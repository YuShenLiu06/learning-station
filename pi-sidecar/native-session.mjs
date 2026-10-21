// Application binding around the pinned public Pi SDK. Credentials and tools remain explicit.
import fs from 'node:fs';
import {ContextBridge,decodeBridgeInput} from './context-bridge.mjs';
import {registerMaterialContext} from './material-context.mjs';
import path from 'node:path';
import {createHash, randomUUID} from 'node:crypto';
import {createAssistantMessageEventStream} from '@earendil-works/pi-ai/utils/event-stream';
import {Agent} from '@earendil-works/pi-agent-core';
import {InMemoryCredentialStore} from '@earendil-works/pi-ai';
import {AgentSession, SessionManager, SettingsManager, ModelRuntime, DefaultResourceLoader, getAgentDir, createExtensionRuntime, convertToLlm} from '@earendil-works/pi-coding-agent';
const stable = v=>Array.isArray(v)?v.map(stable):v&&typeof v==='object'?Object.fromEntries(Object.keys(v).sort().map(k=>[k,stable(v[k])])):v;
const sha = value => createHash('sha256').update(value).digest('hex');
const redact=(v,key)=>typeof v==='string'?v.replaceAll(key,'[REDACTED]'):Array.isArray(v)?v.map(x=>redact(x,key)):v&&typeof v==='object'?Object.fromEntries(Object.entries(v).map(([k,x])=>[k,redact(x,key)])):v;
const containsSecret=(v,key)=>typeof v==='string'?v.includes(key):Array.isArray(v)?v.some(x=>containsSecret(x,key)):v&&typeof v==='object'?Object.values(v).some(x=>containsSecret(x,key)):false;
const MAX_FILE = 32 * 1024 * 1024;
export class NativeSessionError extends Error { constructor(code='NATIVE_SESSION_INVALID',cause){ super(code,{cause});this.code=code; } }
function read(file, limit=MAX_FILE) {
  const info=fs.lstatSync(file); if(!info.isFile()||info.isSymbolicLink()||info.size>limit)throw new NativeSessionError();
  return fs.readFileSync(file,'utf8');
}
function atomic(file,value){
  const tmp=`${file}.${randomUUID()}.tmp`;const fd=fs.openSync(tmp,'wx',0o600);
  try{fs.writeFileSync(fd,JSON.stringify(value));if(!process.permission)fs.fsyncSync(fd);}finally{fs.closeSync(fd);}
  fs.renameSync(tmp,file);const dir=fs.openSync(path.dirname(file),'r');try{if(!process.permission)fs.fsyncSync(dir);}finally{fs.closeSync(dir);}
}
function sync(file){const fd=fs.openSync(file,'r');try{if(!process.permission)fs.fsyncSync(fd);}finally{fs.closeSync(fd);}}
function text(message){return typeof message.content==='string'?message.content:(message.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');}
function safeStream(original,secret,checkpoint,beforeRequest){return (model,context,options)=>{
  const stream=createAssistantMessageEventStream();
  (async()=>{try{await checkpoint();const cleanContext={...context,messages:context.messages.map(m=>m.role==='system'?{...m,...(m.sections?{sections:{...m.sections,cwd:'当前课程'}}:{}),...(typeof m.content==='string'?{content:m.content.replace(/<cwd>[\s\S]*?<\/cwd>/gu,'<cwd>当前课程</cwd>')}:{})}:m)};let received=false;for await(const event of await original(model,cleanContext,options)){if(!received&&(event.type==='text_delta'||event.type==='toolcall_start'||event.type==='done')){beforeRequest?.(context);await checkpoint();received=true;}stream.push(redact(event,secret));}stream.end();}
  catch{const error={role:'assistant',content:[],api:model.api,provider:model.provider,model:model.id,usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'error',errorMessage:'Controlled stream failed',timestamp:Date.now()};stream.push({type:'error',reason:'error',error});stream.end();}})();
  return stream;
};}
export function sealedResources(systemPrompt){
  const extensions={extensions:[],errors:[],runtime:createExtensionRuntime()};
  return {getExtensions:()=>extensions,getSkills:()=>({skills:[],diagnostics:[]}),getPrompts:()=>({prompts:[],diagnostics:[]}),getThemes:()=>({themes:[],diagnostics:[]}),getAgentsFiles:()=>({agentsFiles:[]}),getSystemPrompt:()=>systemPrompt,getSystemPromptSource:()=>undefined,getAppendSystemPrompt:()=>[],getAppendSystemPromptSources:()=>[],extendResources:()=>{throw new NativeSessionError('TOOLS_DISABLED');},reload:async()=>{}};
}
function validateInner(spec){
  if(!spec||!path.isAbsolute(spec.directory)||!/^[A-Za-z0-9_-]{1,200}$/u.test(spec.learningSessionId)||!/^[A-Za-z0-9_-]{1,200}$/u.test(spec.runId)||typeof spec.userEntryId!=='string'||!Array.isArray(spec.authorizedSourceIds))throw new NativeSessionError();
  if(fs.realpathSync(spec.directory)!==spec.directory||!fs.lstatSync(spec.directory).isDirectory())throw new NativeSessionError();
  if(spec.projectRoot!==undefined&&(!path.isAbsolute(spec.projectRoot)||fs.realpathSync(spec.projectRoot)!==spec.projectRoot||!fs.lstatSync(spec.projectRoot).isDirectory()))throw new NativeSessionError();
  if(spec.agentDir!==undefined&&!path.isAbsolute(spec.agentDir))throw new NativeSessionError();
  const binding=JSON.parse(read(path.join(spec.directory,'binding.json'),16384));
  if(binding.version!==1||binding.learningSessionId!==spec.learningSessionId||binding.transcript!=='transcript.jsonl')throw new NativeSessionError();
  const file=path.join(spec.directory,binding.transcript);
  if((binding.initialized||fs.existsSync(path.join(spec.directory,'initialization-started.json')))&&!fs.existsSync(file))throw new NativeSessionError();
  if(fs.existsSync(file)){
    // Unlike the SDK's tolerant reader, reject partial JSON lines and duplicate/cross-session IDs.
    const raw=read(file);if(!raw.endsWith('\n'))throw new NativeSessionError();
    const entries=raw.trimEnd().split('\n').map(line=>JSON.parse(line));
    if(entries[0]?.type!=='session'||entries[0].id!==sha(spec.learningSessionId))throw new NativeSessionError();
    const ids=new Set();for(const e of entries.slice(1)){if(!e.id||ids.has(e.id))throw new NativeSessionError();ids.add(e.id);}
  }
  return {file,binding};
}
function validate(spec){try{return validateInner(spec);}catch(error){throw error instanceof NativeSessionError?error:new NativeSessionError('NATIVE_SESSION_INVALID',error);}}
// Pi native messages are the only model transcript. Business Markdown remains independently intact.
export async function openNativeSession(spec, {model,systemPrompt,tools,streamFn,apiKey,thinkingLevel,finishTurn,beforeToolCall,requestTool,onManifest,onLifecycle,loadChanges,timeContext,checkpoint:providedCheckpoint}){
  const {file,binding}=validate(spec);
  const cwd=spec.projectRoot??spec.directory;
  const lock=path.join(spec.directory,'active.lock');let lockFd;
  try { lockFd=fs.openSync(lock,'wx',0o600);fs.writeFileSync(lockFd,String(process.pid)); }
  catch(error){
    if(error.code!=='EEXIST')throw error;
    const pid=Number(read(lock,100));let alive=true;try{process.kill(pid,0);}catch(e){if(e.code==='ESRCH')alive=false;}
    if(alive)throw new NativeSessionError('NATIVE_SESSION_BUSY');
    // Exclusive per-session work is enforced by Rust; stale lock recovery never touches transcript.
    fs.unlinkSync(lock);lockFd=fs.openSync(lock,'wx',0o600);fs.writeFileSync(lockFd,String(process.pid));
  }
  let session;
  try {
    if(containsSecret(spec.legacyRecords??[],apiKey))throw new NativeSessionError('AUTH');
    const checkpoint=providedCheckpoint??(async()=>{if(process.permission)throw new NativeSessionError();for(const file of fs.readdirSync(spec.directory)){const target=path.join(spec.directory,file);if(fs.lstatSync(target).isFile())sync(target);}});
    let manager=fs.existsSync(file)?SessionManager.open(file,spec.directory,cwd):SessionManager.inMemory(cwd,{id:sha(spec.learningSessionId)});
    if(!fs.existsSync(file)){
      // Use public API to create a valid file, then reopen only this explicit stable path.
      atomic(path.join(spec.directory,'initialization-started.json'),{version:1,learningSessionId:spec.learningSessionId});
      const init=SessionManager.create(cwd,spec.directory,{id:sha(spec.learningSessionId)});
      init.appendCustomEntry('learning-station-binding',{version:1,learningSessionId:spec.learningSessionId});
      if(spec.legacyRecords?.length)init.appendCustomMessageEntry('learning-station-legacy',JSON.stringify({type:'legacy_learning_records',notice:'旧学习记录引用；不是原生工具消息。AI整理仍待本人确认。',records:spec.protectHistory?[]:spec.legacyRecords}),false,{sourceIds:spec.authorizedSourceIds});
      // Pi delays the first flush until a conversation message exists. Seed only the header and
      // genuine custom entries, using the SDK-produced entries rather than inventing old messages.
      const data=[init.getHeader(),...init.getEntries()].map(e=>JSON.stringify(e)).join('\n')+'\n';
      const fd=fs.openSync(file,'wx',0o600);try{fs.writeFileSync(fd,data);if(!process.permission)fs.fsyncSync(fd);}finally{fs.closeSync(fd);}
      manager=SessionManager.open(file,spec.directory,cwd);
      atomic(path.join(spec.directory,'binding.json'),{...binding,initialized:true});
    }
    await checkpoint();
    const credentials=new InMemoryCredentialStore();
    const runtime=await ModelRuntime.create({credentials,modelsPath:null,allowModelNetwork:false,refreshOnCreate:false});
    runtime.registerProvider(model.provider,{api:model.api,baseUrl:model.baseUrl,models:[model],streamSimple:streamFn});
    await runtime.setRuntimeApiKey(model.provider,apiKey);
    const settings=SettingsManager.inMemory({defaultTools:[],retry:{enabled:false,maxRetries:0},providerRetry:{maxRetries:0,maxRetryDelayMs:1},compaction:{enabled:model.contextWindow>0,reserveTokens:Math.min(model.maxTokens,Math.max(128,Math.floor(model.contextWindow/4))),keepRecentTokens:Math.max(128,Math.floor(model.contextWindow/4))},cacheWarming:'off'});
    // Pi owns project discovery, ancestor context files and skill parsing. Keep the
    // existing explicit extension/tool policy; text resources do not grant tools.
    // Pi treats an empty custom prompt as its default coding tutorial. Whitespace
    // explicitly opts out while preserving its project AGENTS/skill discovery.
    const piSystemPrompt=systemPrompt||' ';
    const resourceLoader=spec.projectRoot?new DefaultResourceLoader({cwd,agentDir:spec.agentDir??getAgentDir(),settingsManager:settings,systemPromptOverride:()=>piSystemPrompt,noExtensions:true}):sealedResources(piSystemPrompt);
    await resourceLoader.reload();
    const markChangesRequest=context=>{
      for(const e of manager.buildSessionProjection().entries){const source=e.sourceEntry;
        if(!e.messages.length||source.type!=='custom_message'||source.customType!=='learning-station-material'||!context.messages.some(m=>text(m)===source.content))continue;
        if(!manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-material-request'&&r.data?.entryId===source.id))manager.appendCustomEntry('learning-station-material-request',{nativeSessionId:manager.getSessionId(),entryId:source.id});
      }
      for(const e of manager.buildSessionProjection().entries){const source=e.sourceEntry;
        if(!e.messages.length||source.type!=='custom_message'||source.customType!=='learning-station-changes'||source.details?.runId!==spec.runId||!context.messages.some(m=>text(m).includes(source.content)))continue;
        if(!manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-changes-request'&&r.data?.changesEntryId===source.id))manager.appendCustomEntry('learning-station-changes-request',{nativeSessionId:manager.getSessionId(),runId:spec.runId,userEntryId:spec.userEntryId,changesEntryId:source.id});
      }
    };
    const agent=new Agent({sessionId:manager.getSessionId(),initialState:{model,thinkingLevel,messages:manager.buildSessionContext().messages},streamFn:safeStream(streamFn,apiKey,checkpoint,markChangesRequest),convertToLlm,toolExecution:'sequential',finishTurn});
    session=new AgentSession({agent,sessionManager:manager,settingsManager:settings,cwd,resourceLoader,modelRuntime:runtime,baseToolsOverride:{},customTools:tools,initialActiveToolNames:tools.map(t=>t.name),allowedToolNames:tools.map(t=>t.name),usesDefaultTools:false});
    // AgentSession owns its hooks; application preflight remains an additional restrictive layer.
    const nativeBefore=agent.beforeToolCall;agent.beforeToolCall=async(c,s)=>await beforeToolCall?.(c,s)??await nativeBefore?.(c,s);
    agent.finishTurn=finishTurn;
    const expected=tools.map(t=>t.name).sort().join(',');if(agent.state.tools.map(t=>t.name).sort().join(',')!==expected)throw new NativeSessionError('TOOLS_DISABLED');
    const journal=path.join(spec.directory,'run-'+sha(spec.runId)+'.json');
    const old=fs.existsSync(journal)?JSON.parse(read(journal,2*1024*1024)):null;
    if(old&&(old.runId!==spec.runId||old.userEntryId!==spec.userEntryId))throw new NativeSessionError();
    const close=()=>{session?.dispose();try{fs.closeSync(lockFd);}catch{}try{fs.unlinkSync(lock);}catch{}};
    const persist=record=>{atomic(journal,record);sync(file);};
    let record=old??{version:1,runId:spec.runId,userEntryId:spec.userEntryId,status:'pending',userNativeId:null};
    const previousBinding=manager.getEntries().filter(e=>e.type==='custom'&&e.customType==='learning-station-scope').at(-1)?.data;
    const revoked=previousBinding?.sourceIds?.some(id=>!spec.authorizedSourceIds.includes(id));
    // Recover a genuine tool result only from Rust's durable reply. A missing reply is unknown,
    // never a negative receipt and never permission to execute a write again.
    const branch=manager.buildSessionProjection();
    const pending=new Map();
    for(const entry of branch.entries)for(const message of entry.messages){
      if(message.role==='assistant')for(const call of message.content.filter(c=>c.type==='toolCall'))pending.set(call.id,call);
      if(message.role==='toolResult')pending.delete(message.toolCallId);
    }
    for(const call of pending.values()){
      const owner=manager.getEntries().filter(e=>e.type==='custom'&&e.customType==='learning-station-tool'&&e.data?.callId===call.id).at(-1)?.data;
      if(!owner)throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
      const receipt=path.join(spec.directory,'tool-'+sha(JSON.stringify([owner.runId,call.id]))+'.json');
      let reply=fs.existsSync(receipt)?JSON.parse(read(receipt,65536)):null;
      if(call.name==='read'||!reply?.reply){
        if(typeof requestTool!=='function')throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
        const recovered=await requestTool({type:'tool_recover',recoveryRunId:owner.runId,id:call.id,name:call.name,args:call.arguments});
        const deniedRead=call.name==='read'&&['TOOL_SCOPE','CONFLICT','TOOL_NOT_ALLOWED','TRASHED','NOT_FOUND'].includes(recovered?.error?.code);
        if(!(call.name==='read'?recovered?.result||deniedRead:recovered?.result?.persisted))throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
        reply={callId:call.id,name:call.name,argsHash:sha(JSON.stringify(stable(call.arguments))),reply:recovered};
      }
      if(reply.callId!==call.id||reply.name!==call.name||reply.argsHash!==sha(JSON.stringify(stable(call.arguments)))||!reply.reply)throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
      const value=reply.reply.result?{ok:true,result:reply.reply.result}:{ok:false,error:reply.reply.error};
      manager.appendMessage({role:'toolResult',toolCallId:call.id,toolName:call.name,content:[{type:'text',text:JSON.stringify(value)}],isError:!!reply.reply.error,timestamp:Date.now()});
    }
    // A changed scope must not hide an unresolved write and bypass its receipt.
    if(revoked||spec.protectHistory&&(!spec.historyBoundaryKey||previousBinding?.historyBoundaryKey!==spec.historyBoundaryKey)){
      const boundary=manager.appendCustomMessageEntry('learning-station-permission-boundary','资料授权已变化；此前模型上下文已排除，仅使用本轮授权资料。',false,{});
      manager.appendCompaction('应用权限边界：此前内容未重新授权，不作为本轮依据。',boundary,0,{reason:'authorization-change'},true);
    }
    manager.appendCustomEntry('learning-station-scope',{sourceIds:spec.authorizedSourceIds,historyBoundaryKey:spec.historyBoundaryKey});
    agent.state.messages=manager.buildSessionContext().messages;
    // Older application projections carried raw internal metadata and no registry identity.
    // Retire only those custom projections; Pi keeps the actual user/tool/assistant history.
    for(const entry of manager.buildSessionProjection().entries)if(entry.messages.length&&entry.sourceEntry.type==='custom_message'&&['learning-station-material','learning-station-changes','learning-station-turn'].includes(entry.sourceEntry.customType)&&!entry.sourceEntry.details?.bridgeKey)manager.appendContextEdit(entry.sourceEntry.id,null);
    // Old per-turn references are not silently injected into later requests.
    for(const entry of manager.buildSessionProjection().entries){if(entry.sourceEntry.type==='custom_message'&&entry.sourceEntry.customType==='learning-station-turn'&&entry.messages.length)manager.appendContextEdit(entry.sourceEntry.id,null);}
    agent.state.messages=manager.buildSessionContext().messages;
    const bridge=new ContextBridge(manager,{runId:spec.runId,userEntryId:spec.userEntryId});
    const nativeTransform=agent.transformContext;
    agent.transformContext=async(messages,signal)=>bridge.ordered(await nativeTransform?.(messages,signal)??messages);
    let currentTurn;
    const applyContext=async input=>{
      const envelope=decodeBridgeInput(input);
      if(containsSecret(envelope,apiKey))throw new NativeSessionError('AUTH');
      currentTurn=input;
      for(const entry of manager.buildSessionProjection().entries)if(entry.messages.length&&entry.sourceEntry.type==='custom_message'&&['learning-station-turn','learning-station-focus'].includes(entry.sourceEntry.customType))bridge.exclude(entry.sourceEntry.id);
      const clock=envelope.currentTime??(timeContext?{referenceTime:timeContext.referenceTime,timeZone:timeContext.timeZone}:undefined);
      if(clock)bridge.message('current_time','本轮冻结时间与时区','learning-station-turn',clock);
      if(envelope.nativeReferences)bridge.message('references','用户本轮明确提供的引用及必要补充','learning-station-turn',envelope.nativeReferences);
      const inventory=await registerMaterialContext({manager,spec,loadChanges,envelope,bridge,containsSecret,apiKey,NativeSessionError});
      if(envelope.exploration)bridge.message('exploration','探索入口的任务位置与来源','learning-station-turn',envelope.exploration);
      bridge.commit();
      const projected=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.details?.bridgeKey).flatMap(e=>e.messages.map(text)).join('\n');
      inventory.projectedChars=[...projected].length;inventory.projectedBytes=Buffer.byteLength(projected);
      agent.state.messages=manager.buildSessionContext().messages;
      await onManifest?.(inventory);
    };
    let pendingProjection=Promise.resolve();
    const nativePrepare=agent.prepareRequest;
    agent.prepareRequest=async (...args)=>{await pendingProjection;return nativePrepare?.(...args);};
    const unsub=session.subscribe(event=>{
      if(['compaction_start','compaction_end'].includes(event.type)){
        // SDK event listeners are synchronous. Update the projection immediately,
        // and explicitly await its receipt before the next prepared request.
        const projection=event.type==='compaction_end'&&!event.aborted&&currentTurn?applyContext(currentTurn):undefined;
        const lifecycle=onLifecycle?.({type:event.type,reason:event.reason,aborted:event.aborted??false});
        pendingProjection=Promise.all([pendingProjection,projection,lifecycle]).then(()=>{});
        void pendingProjection.catch(()=>{});
      }
    });
    // Public AgentSession listeners run before its persistence. The Agent subscriber below is
    // awaited after the SDK's internal subscriber, so the native entry ID is genuinely durable.
    const userUnsub=agent.subscribe(async event=>{
      if(event.type==='message_end'&&event.message.role==='user'&&!record.userNativeId){
        const entry=manager.getEntries().filter(e=>e.type==='message'&&e.message.role==='user').at(-1);
        if(!entry)throw new NativeSessionError();
        record.userNativeId=entry.id;manager.appendCustomEntry('learning-station-user',{userEntryId:spec.userEntryId,runId:spec.runId,nativeId:entry.id,inputHash:record.inputHash});persist(record);
      }
      if(event.type==='message_end')await checkpoint();
    });
    const userIdentity=nativeId=>{const owner=manager.getEntries().find(e=>e.type==='custom'&&e.customType==='learning-station-user'&&e.data?.nativeId===nativeId)?.data;return owner?{userEntryId:owner.userEntryId,...(owner.runId?{runId:owner.runId}:{})}:{};};
    return {session,agent,manager,applyContext,record,debugRegistrations:()=>bridge.registrations(),
      debugSources:()=>manager.buildSessionProjection().entries.flatMap(entry=>entry.messages.map(message=>({
        value:text(convertToLlm([message])[0]??message),origin:message.role==='custom'?(['learning-station-material','learning-station-turn','learning-station-changes','learning-station-focus'].includes(message.customType)?'bridge':'unknown'):({user:'user',assistant:'model',toolResult:'tool',compactionSummary:'pi',branchSummary:'pi'}[message.role]??'unknown'),
        ...(entry.sourceEntry.details?.bridgeKey?{registrationKey:entry.sourceEntry.details.bridgeKey,description:entry.sourceEntry.details.bridgeDescription,runId:entry.sourceEntry.details.runId,userEntryId:entry.sourceEntry.details.userEntryId}:message.role==='user'?{registrationKey:'question',description:'Pi 原生用户输入',...userIdentity(entry.sourceEntry.id)}:{}),
        name:message.customType??message.role,sourcePath:`native/${entry.sourceEntry.id}/${message.customType??message.role}`,
        ...(message.role==='user'&&entry.sourceEntry.id===record.userNativeId?{registrationKey:'question'}:{}),
      }))),
      async requestTool(call,signal){manager.appendCustomEntry('learning-station-tool',{runId:spec.runId,callId:call.id,argsHash:sha(JSON.stringify(stable(call.args)))});sync(file);await checkpoint();return requestTool(call,signal);},
      async prompt(question){
        if(question.includes(apiKey))throw new NativeSessionError('AUTH');
        const inputHash=sha(question);if(record.inputHash&&record.inputHash!==inputHash)throw new NativeSessionError();record.inputHash=inputHash;
        if(record.status==='complete')return {cached:record.finalContent};
        persist(record);
        // Preserve a previously accepted user once, including after a failed run.
        const prior=manager.getEntries().find(e=>e.type==='custom'&&e.customType==='learning-station-user'&&e.data?.userEntryId===spec.userEntryId);
        if(prior){const entry=manager.getEntry(prior.data.nativeId);if(!entry||entry.type!=='message'||entry.message.role!=='user'||(prior.data.inputHash?prior.data.inputHash!==inputHash:text(entry.message)!==question))throw new NativeSessionError();
          // Compaction or a permission boundary may have removed the original from
          // the effective projection. Restore only this exact saved question as
          // retry context, never the revoked material or another user utterance.
          const accepted=text(entry.message);
          const visible=manager.buildSessionProjection().entries.some(e=>e.sourceEntry.id===entry.id&&e.messages.some(m=>m.role==='user'&&text(m)===accepted));
          const retry=visible?'继续处理上一条已保存用户问题；不得重新执行已完成工具。':`继续处理这条已保存用户问题：\n\n${accepted}\n\n不得重新执行已完成工具。`;
          await session.sendCustomMessage({customType:'learning-station-retry',content:retry,display:false,details:{userEntryId:spec.userEntryId}},{triggerTurn:true});
        }else{
          await session.prompt(question,{expandPromptTemplates:!!spec.projectRoot,source:'rpc'});

        }
        await agent.waitForIdle();await pendingProjection;return {};
      },
      async correctOutput(message){
        await session.sendCustomMessage({customType:'learning-station-output-validation',content:message,display:false,details:{userEntryId:spec.userEntryId}},{triggerTurn:true});
        await agent.waitForIdle();await pendingProjection;await checkpoint();
      },
      async complete(finalContent){await pendingProjection;record.status='complete';record.finalContent=finalContent;persist(record);await checkpoint();},
      close(){unsub();userUnsub();close();},
    };
  }catch(error){session?.dispose();try{fs.closeSync(lockFd);}catch{}try{fs.unlinkSync(lock);}catch{}throw error instanceof NativeSessionError?error:new NativeSessionError('NATIVE_SESSION_INVALID',error);}
}
