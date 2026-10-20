// Application binding around the pinned public Pi SDK. Credentials and tools remain explicit.
import fs from 'node:fs';
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
  (async()=>{try{beforeRequest?.(context);await checkpoint();const cleanContext={...context,messages:context.messages.map(m=>m.role==='system'?{...m,...(m.sections?{sections:{...m.sections,cwd:'学习站受控学习空间（没有通用文件或命令权限）'}}:{}),...(typeof m.content==='string'?{content:m.content.replace(/<cwd>[\s\S]*?<\/cwd>/gu,'<cwd>学习站受控学习空间（没有通用文件或命令权限）</cwd>')}:{})}:m)};for await(const event of await original(model,cleanContext,options))stream.push(redact(event,secret));stream.end();}
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
export async function openNativeSession(spec, {model,systemPrompt,tools,streamFn,apiKey,thinkingLevel,finishTurn,beforeToolCall,requestTool,onManifest,onLifecycle,loadChanges,checkpoint:providedCheckpoint}){
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
    const resourceLoader=spec.projectRoot?new DefaultResourceLoader({cwd,agentDir:spec.agentDir??getAgentDir(),settingsManager:settings,systemPromptOverride:()=>systemPrompt,noExtensions:true}):sealedResources(systemPrompt);
    await resourceLoader.reload();
    const markChangesRequest=context=>{
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
      if(!reply?.reply){
        if(typeof requestTool!=='function')throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
        const recovered=await requestTool({type:'tool_recover',recoveryRunId:owner.runId,id:call.id,name:call.name,args:call.arguments});
        if(!recovered?.result?.persisted)throw new NativeSessionError('NATIVE_TOOL_UNKNOWN');
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
    // Old per-turn references are not silently injected into later requests.
    for(const entry of manager.buildSessionProjection().entries){if(entry.sourceEntry.type==='custom_message'&&entry.sourceEntry.customType==='learning-station-turn'&&entry.messages.length)manager.appendContextEdit(entry.sourceEntry.id,null);}
    agent.state.messages=manager.buildSessionContext().messages;
    const materialBaseline=id=>{
      const active=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.customType==='learning-station-material'&&e.sourceEntry.details?.sourceId===id);
      let activeHash;let chainReady=false;
      for(const entry of active){
        let value;try{value=JSON.parse(text(entry.messages[0]));}catch{chainReady=false;break;}
        for(const frame of value.frames??[]){if(frame.kind==='full'){chainReady=sha(frame.text)===frame.textHash;activeHash=frame.textHash;}
        else if(frame.kind==='diff'){chainReady=chainReady&&frame.baseHash===activeHash;activeHash=frame.textHash;}else chainReady=false;}
      }
      return {active,last:chainReady?active.at(-1)?.sourceEntry.details:undefined};
    };
    let currentSources=[];let currentTurn;
    const applyContext=async envelope=>{
      if(containsSecret(envelope,apiKey))throw new NativeSessionError('AUTH');
      currentSources=envelope.nativeMaterials??[];currentTurn=envelope;
      const inventory=structuredClone(envelope.contextBridgeManifest??{version:1,entries:[]});
      const metadataOnly=envelope.metadataOnlySourceIds??[];
      if(!Array.isArray(metadataOnly)||metadataOnly.some(id=>!spec.authorizedSourceIds.includes(id)))throw new NativeSessionError();
      for(const entry of manager.buildSessionProjection().entries)if(entry.messages.length&&entry.sourceEntry.type==='custom_message'&&entry.sourceEntry.customType==='learning-station-material'&&metadataOnly.includes(entry.sourceEntry.details?.sourceId))manager.appendContextEdit(entry.sourceEntry.id,null);
      inventory.entries=inventory.entries.filter(e=>e.kind!=='history');
      if(loadChanges&&envelope.focus){
        const focus=envelope.focus;
        if(focus.asset&&!spec.authorizedSourceIds.includes(focus.asset.id))throw new NativeSessionError();
        // A Git commit alone is never an accepted checkpoint. Reconcile the
        // original saved question against its actual native user entry, even
        // after cancellation, a lost reply, or a new run for the same question.
        const received=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.customType==='learning-station-changes'&&e.sourceEntry.details?.courseId===focus.courseId).map(e=>e.sourceEntry).filter(e=>manager.getEntries().some(r=>r.type==='custom'&&r.customType==='learning-station-changes-request'&&r.data?.changesEntryId===e.id&&r.data.runId===e.details.runId&&r.data.userEntryId===e.details.userEntryId&&r.data.nativeSessionId===manager.getSessionId())&&manager.getEntries().some(u=>u.type==='custom'&&u.customType==='learning-station-user'&&u.data?.userEntryId===e.details.userEntryId&&manager.getEntry(u.data.nativeId)?.message?.role==='user')).at(-1);
        const body=materialBaseline(focus.asset?.id??focus.documentId).last;
        const focusBaseCommitOid=body?.range?.complete&&body.path===focus.path?body.gitCommitOid??null:null;
        const changes=await loadChanges({baseCommitOid:received?.details.checkpointCommitOid??null,focusBaseCommitOid});
        if(changes?.complete!==true||changes.checkpoint?.courseId!==focus.courseId||changes.checkpoint.checkpointCommitOid!==focus.commitHash||changes.checkpoint.materialTreeOid!==focus.materialTreeOid||changes.baseCommitOid!==(received?.details.checkpointCommitOid??null)||!Array.isArray(changes.changes))throw new NativeSessionError();
        if(Buffer.byteLength(JSON.stringify(changes))>512*1024)throw new NativeSessionError('LIMIT_EXCEEDED');
        if(containsSecret(changes,apiKey))throw new NativeSessionError('AUTH');
        if((changes.focused?.textUnavailable||focus.binary||['image','pdf','exercise'].includes(focus.type))&&(typeof changes.focused?.text==='string'||focus.paragraphs?.some(p=>p.text?.length)))throw new NativeSessionError('CONTEXT_CONFLICT');
        currentSources=typeof changes.focused?.text==='string'?[changes.focused]:[];
        for(const source of currentSources)if(sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
        if(currentSources.length+changes.changes.length+(changes.checkpoint.excludedFiles?.length??0)+inventory.entries.filter(e=>e.kind!=='material').length>256)throw new NativeSessionError('LIMIT_EXCEEDED');
        inventory.entries=inventory.entries.filter(e=>e.kind!=='material');
        for(const source of currentSources)inventory.entries.push({id:source.id,kind:'material',sourceId:source.id,path:source.path,version:source.version,includedChars:[...source.text].length,range:source.range});
        if(!currentSources.length&&!changes.focused?.textUnavailable)inventory.entries.push({id:`focus:${focus.asset?.id??focus.documentId}`,kind:'material',status:'added',sourceId:focus.asset?.id??focus.documentId,path:focus.path,version:focus.documentVersion,includedChars:0,reason:'focus_metadata_only'});
        if(changes.focused?.textUnavailable)inventory.entries.push({id:changes.focused.id,kind:'material',status:'retained',sourceId:changes.focused.id,version:focus.documentVersion??focus.commitHash,includedChars:0,reason:`text_unavailable:${changes.focused.textUnavailable}`,fixedVersion:{courseId:focus.courseId,checkpointCommitOid:focus.commitHash,path:changes.focused.path,blobOid:changes.focused.gitBlobOid}});
        for(const change of changes.changes)inventory.entries.push({id:`change:${change.path}`,kind:'material',status:change.status==='A'?'added':'updated',version:focus.materialTreeOid,includedChars:JSON.stringify(change).length,path:change.path,fileStatus:change.status,added:change.added,deleted:change.deleted,binary:change.binary,fixedVersion:change.fixedVersion,reason:change.patchUnavailable?'git_patch_non_utf8':change.binary?'git_binary_status':change.patch?'git_complete_diff':'git_name_status_numstat'});
        for(const [index,excluded] of (changes.checkpoint.excludedFiles??[]).entries())inventory.entries.push({id:`excluded:${index}:${excluded.assetId}`,kind:'material',status:'retained',...(excluded.assetId?{sourceId:excluded.assetId}:{}),version:focus.commitHash,includedChars:0,reason:`excluded:${excluded.reason}`,fileName:excluded.fileName});
        if(changes.changes.length||changes.exclusionsChanged){
          const notification={type:'material_changes',checkpoint:changes.checkpoint,baseCommitOid:changes.baseCommitOid,initial:changes.initial,changes:changes.changes,complete:true};
          manager.appendCustomMessageEntry('learning-station-changes',JSON.stringify(notification),false,{courseId:focus.courseId,checkpointCommitOid:focus.commitHash,materialTreeOid:focus.materialTreeOid,runId:spec.runId,userEntryId:spec.userEntryId});
        }
      }
      for(const source of currentSources){
        if(typeof source.text!=='string'||sha(source.text)!==source.textHash||!spec.authorizedSourceIds.includes(source.id))throw new NativeSessionError();
        const {active,last}=materialBaseline(source.id);
        let status=manager.getEntries().some(e=>e.type==='custom_message'&&e.customType==='learning-station-material'&&e.details?.sourceId===source.id)?'reloaded':'added',frames=[{kind:'full',version:source.version,textHash:source.textHash,text:source.text}];
        if(last&&source.range?.complete&&last?.range?.complete&&(!source.path||last.path===source.path)&&last.textHash===source.textHash){status='retained';frames=[];}
        else if(source.range?.complete&&last?.range?.complete&&(!source.path||last.path===source.path)&&active.length<8){
          if(source.gitChange&&last.gitCommitOid===source.gitChange.baseCommitOid&&last.gitBlobOid===source.gitChange.baseBlobOid){frames=[{kind:'diff',version:source.version,baseHash:last.textHash,textHash:source.textHash,unifiedDiff:source.gitChange.unifiedDiff}];status='updated';}
          else {const at=source.frames?.findIndex(f=>f.textHash===last.textHash)??-1;if(at>=0&&source.frames.length>at+1){frames=source.frames.slice(at+1);status='updated';}}
        }
        if(frames.length){
          if(frames[0].kind==='full')for(const entry of active)manager.appendContextEdit(entry.sourceEntry.id,null);
          manager.appendCustomMessageEntry('learning-station-material',JSON.stringify({type:'automatic_material_projection',id:source.id,...(source.path?{path:source.path}:{}),version:source.version,range:source.range,...(source.bodyStartChar?{bodyStartChar:source.bodyStartChar}:{}),frames}),false,{sourceId:source.id,...(source.path?{path:source.path}:{}),textHash:source.textHash,version:source.version,range:source.range,...(source.gitCommitOid?{gitCommitOid:source.gitCommitOid,gitBlobOid:source.gitBlobOid}:{})});
        }
        const item=inventory.entries.find(e=>e.sourceId===source.id&&e.kind==='material');if(item){item.status=status;item.reason=status==='retained'?'native_projection_contains_baseline':status==='updated'?'native_projection_verified_diff':'native_baseline_reload';}
      }
      for(const entry of manager.buildSessionProjection().entries){if(entry.sourceEntry.type==='custom_message'&&entry.sourceEntry.customType==='learning-station-turn'&&entry.messages.length)manager.appendContextEdit(entry.sourceEntry.id,null);}
      const projectedMaterial=manager.buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&['learning-station-material','learning-station-changes'].includes(e.sourceEntry.customType)).flatMap(e=>e.messages.map(text)).join('\n')+(envelope.nativeReferences??'')+(loadChanges?'':(envelope.focus?.paragraphs??[]).map(p=>p.text??'').join(''));
      inventory.projectedChars=[...projectedMaterial].length;inventory.projectedBytes=Buffer.byteLength(projectedMaterial);
      const turn={...envelope};delete turn.nativeMaterials;delete turn.learningHistory;delete turn.historyManifest;delete turn.context;delete turn.question;
      // The fixed full/diff frames above own the automatic Focus body. Keep the
      // frozen source untouched; repeating visible text here bypasses that chain.
      if(loadChanges&&currentSources.length&&turn.focus?.paragraphs){turn.focus={...turn.focus,paragraphs:turn.focus.paragraphs.map(({text,...location})=>location)};for(const item of inventory.entries)if(item.kind==='turn'&&(item.id==='focus'||item.id.startsWith('focus:'))){item.includedChars=0;item.reason='focus_location';}}
      turn.context=envelope.nativeReferences??'';delete turn.nativeReferences;turn.contextBridgeManifest=inventory;
      manager.appendCustomMessageEntry('learning-station-turn',JSON.stringify(turn),false,{runId:spec.runId});
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
        record.userNativeId=entry.id;manager.appendCustomEntry('learning-station-user',{userEntryId:spec.userEntryId,nativeId:entry.id,inputHash:record.inputHash});persist(record);
      }
      if(event.type==='message_end')await checkpoint();
    });
    return {session,agent,manager,applyContext,record,
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
