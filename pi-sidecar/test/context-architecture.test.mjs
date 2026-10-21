import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {createAssistantMessageEventStream} from '@earendil-works/pi-ai/utils/event-stream';
import {openNativeSession} from '../native-session.mjs';
import {SessionManager} from '@earendil-works/pi-coding-agent';
import {runLearningAgent} from '../harness.mjs';
import {LEARNING_PLUGIN} from '../learning-plugin.mjs';
import {decodeBridgeInput} from '../context-bridge.mjs';
const hash=text=>createHash('sha256').update(text).digest('hex');
const content=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
const first='FIRST_FOCUS_BODY',second='SECOND_FOCUS_BODY',quote='EXACT_QUOTE\nconst literal = "\\n";';
const cp1='1'.repeat(40),cp2='2'.repeat(40),tree='3'.repeat(40);
const clock={referenceTime:'2026-10-21T09:00:00Z',timeZone:'Etc/UTC'};
const entry=(key,description,value)=>({key,description,value});
function source(body,commit,base) {
  return {id:'doc_1',path:'documents/doc_1.md',version:hash(body),textHash:hash(body),text:body,range:{startChar:0,endChar:body.length,complete:true,totalBytes:body.length},gitCommitOid:commit,gitBlobOid:hash(body),...(base?{gitChange:{baseCommitOid:cp1,baseBlobOid:hash(first),unifiedDiff:`@@ -1 +1 @@\n-${first}\n+${second}\n`}}:{})};
}
test('final SDK wire allows only registered supplements in declared order and one native question',async()=>{
  const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'context-architecture-')));
  fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'architecture_session',transcript:'transcript.jsonl',initialized:false}));
  const packets=[],captures=[],inventories=[];
  const server=createServer(async(req,res)=>{let raw='';for await(const part of req)raw+=part;packets.push(JSON.parse(raw));res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'OK'},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try {
    // Seed an older application preamble through Pi itself, then inspect the new final HTTP wire.
    const model={provider:'learning-custom',id:'fixture',api:'openai-completions',baseUrl:'http://127.0.0.1:1/v1',contextWindow:0,maxTokens:1024};
    const seed=await openNativeSession({directory,learningSessionId:'architecture_session',runId:'seed_run',userEntryId:'seed_user',authorizedSourceIds:['doc_1'],protectHistory:false,legacyRecords:[]},{model,systemPrompt:'SUPERSEDED_APPLICATION_RULES',tools:[],apiKey:'synthetic-architecture-key',thinkingLevel:'off',streamFn:()=>{
      const stream=createAssistantMessageEventStream();queueMicrotask(()=>{stream.push({type:'done',reason:'stop',message:{role:'assistant',content:[{type:'text',text:'Earlier genuine answer'}],api:model.api,provider:model.provider,model:model.id,usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:Date.now()}});stream.end();});return stream;
    }});
    seed.manager.appendCustomMessageEntry('learning-station-material',JSON.stringify({type:'automatic_material_projection',frames:[{kind:'full',text:'OLD_INTERNAL_METADATA_MUST_NOT_SURVIVE'}]}),false,{sourceId:'doc_1'});
    seed.manager.appendCustomMessageEntry('learning-station-changes',JSON.stringify({checkpoint:{internal:'OLD_INTERNAL_CHECKPOINT_MUST_NOT_SURVIVE'}}),false,{});
    await seed.prompt('PREVIOUS_NATIVE_QUESTION');await seed.complete('Earlier genuine answer');seed.close();
    for(let round=0;round<4;round++) {
      if(round===3) {
        const manager=SessionManager.open(path.join(directory,'transcript.jsonl'));
        manager.appendCompaction('Synthetic summary cannot prove a full body',manager.getEntries().filter(e=>e.type==='message').at(-1).id,100);
      }
      const body=round===0?first:second,commit=round===0?cp1:cp2,question=`QUESTION_ONLY_ONCE_${round}`;
      const focus={courseId:'course_1',documentId:'doc_1',sourceDocumentId:'doc_1',path:'documents/doc_1.md',type:'markdown',documentVersion:hash(body),commitHash:commit,materialTreeOid:tree,view:'read',paragraphs:[{startChar:0,endChar:body.length,text:body}],uiState:'DO_NOT_SEND_UI_STATE',metadata:{permission:'DO_NOT_SEND_PERMISSION',stats:'DO_NOT_SEND_STATS'}};
      const frozen=[entry('current_time','本轮时间',clock),entry('references','本轮引用',round===0?quote:''),entry('focus','聚焦材料',focus),entry('question','仅作为原生用户消息发送',question)];
      const request={protocol:1,operation:'generate',apiKey:'synthetic-architecture-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture'},timeContext:clock,learningTools:true,learningSkills:[],toolPolicy:{mode:'question',authorization:'off',allowedTools:LEARNING_PLUGIN.tools,maxCalls:6},debugProtocol:true,debugRegistrations:frozen,nativeSession:{directory,learningSessionId:'architecture_session',runId:`round_${round}`,userEntryId:`user_${round}`,authorizedSourceIds:['doc_1'],protectHistory:false,legacyRecords:[]},materialChanges:true,messages:[{role:'user',content:JSON.stringify({bridge:frozen,contextBridgeManifest:{version:1,entries:[]}})}]};
      await runLearningAgent(request,async event=>{if(event.type==='debug_request')captures.push(event);if(event.type==='context_manifest')inventories.push(event.manifest);},{requestTool:async call=>{
        if(call.type==='debug_probe')return {result:{epoch:1}};
        if(call.type==='session_sync')return {result:{synced:true}};
        assert.equal(call.type,'material_changes');
        assert.equal(call.focusBaseCommitOid,round===0||round===3?null:round===1?cp1:cp2);
        return {result:{complete:true,checkpoint:{courseId:'course_1',checkpointCommitOid:commit,materialTreeOid:tree,excludedFiles:[]},baseCommitOid:call.baseCommitOid,exclusionsChanged:round===0,focused:source(body,commit,round>0),changes:round===2?[]:[{path:focus.path,status:'M',patch:'MUST_NOT_DUPLICATE_FOCUS_PATCH'},{path:'documents/other.md',status:'M',added:1,deleted:1,patch:'MUST_NOT_SEND_NONFOCUS_PATCH'}]}};
      }});
      const packet=packets[round],texts=packet.messages.map(content),serialized=JSON.stringify(packet);
      assert.equal(texts.filter(text=>text===question).length,1);
      if(round<3)assert(texts.includes('PREVIOUS_NATIVE_QUESTION'));
      for(const forbidden of ['DO_NOT_SEND_','automaticProjection','turnState','materialRole','projectionVersion','contextBridgeManifest','MUST_NOT_SEND_','MUST_NOT_DUPLICATE_','学习指导（','工具授权档位','跨学科学习助手','SUPERSEDED_APPLICATION_RULES','expert coding assistant','Pi documentation','OLD_INTERNAL_'])assert(!serialized.includes(forbidden),forbidden);
      const capture=captures[round];assert.deepEqual(capture.body,packet);
      const application=capture.sources.filter(s=>s.origin==='bridge');
      assert(application.every(s=>s.registrationKey&&s.description&&s.runId));
      const keys=application.map(s=>s.registrationKey),priority=['current_time','references','focus','material_changes'];
      assert(keys.every(key=>priority.includes(key)),'no undeclared application key');
      for(const source of capture.sources.filter(s=>s.origin==='unknown')){const role=source.path[0]==='messages'?packet.messages[source.path[1]].role:'system';if(['assistant','tool'].includes(role))continue;let value=packet;for(const at of source.path)value=value[at];if(source.start!==undefined)value=value.slice(source.start,source.end);assert.equal(value.trim(),'','no unregistered application text at the final wire boundary');}
      assert.deepEqual(keys,[...keys].sort((a,b)=>priority.indexOf(a)-priority.indexOf(b)));
      assert.equal(keys.filter(key=>key==='current_time').length,1);
      assert.equal(texts.filter(text=>text.includes('EXACT_QUOTE')).length,round===0?1:0);
      if(round===0)assert(texts.includes(quote),'reference preserves real newlines and literal backslash-n');
      const values=application.map(s=>{let value=packet;for(const at of s.path)value=value[at];return s.registrationKey==='references'?value:JSON.parse(value);});
      const material=values.filter(v=>v.type==='automatic_material_projection');
      if(round===0||round===3)assert.equal(material[0].frames[0].kind,'full');
      if(round===1)assert.equal(material.at(-1).frames[0].kind,'diff');
      if(round===2)assert.equal(material.length,2);
      assert.equal(serialized.split(first).length-1,round===0?1:round===3?0:2); // full baseline plus diff removal line
      const location=values.find(v=>v.documentId==='doc_1');assert(location);assert(location.paragraphs.every(p=>p.text===undefined));
      const notifications=values.filter(v=>Array.isArray(v.changes));assert(notifications.every(v=>v.changes.every(c=>c.path!=='documents/doc_1.md'&&c.patch===undefined)));
      assert.deepEqual(packet.tools.map(t=>t.function.name).sort(),[...LEARNING_PLUGIN.tools].sort());
      assert(packet.tools.every(t=>t.function.description.length<180));
    }
    if(process.env.LS_CONTEXT_WIRE_EVIDENCE) {
      fs.mkdirSync(process.env.LS_CONTEXT_WIRE_EVIDENCE,{recursive:true});
      fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'requests-after.json'),JSON.stringify(packets,null,2));
      fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'source-map.json'),JSON.stringify(captures.map(c=>({sources:c.sources,registrations:c.registrations})),null,2));
    }
  } finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));fs.rmSync(directory,{recursive:true,force:true});}
});
test('conflicting registrations fail before projection and private bridge requires a native mapping',async()=>{
  assert.throws(()=>decodeBridgeInput({bridge:[entry('references','明确引用','first'),entry('references','明确引用','second')]}),e=>e.code==='CONTEXT_CONFLICT');
  await assert.rejects(runLearningAgent({protocol:1,operation:'generate',apiKey:'synthetic-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1:1/v1',model:'fixture'},messages:[{role:'user',content:JSON.stringify({bridge:[entry('question','用户问题','never send internal envelope')]})}]},()=>{}),e=>e.code==='NATIVE_SESSION_INVALID');
});

test('the application bundle includes the bridge and material provider modules',()=>{
 const config=JSON.parse(fs.readFileSync(new URL('../../src-tauri/tauri.conf.json',import.meta.url)));
 for(const file of ['context-bridge.mjs','material-context.mjs'])assert.equal(config.bundle.resources[`../pi-sidecar/${file}`],`pi-sidecar/${file}`);
});
