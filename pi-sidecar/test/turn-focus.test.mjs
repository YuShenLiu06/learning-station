import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {SessionManager} from '@earendil-works/pi-coding-agent';
import {LEARNING_PLUGIN} from '../learning-plugin.mjs';
import {runLearningAgent} from '../harness.mjs';
const text=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');

test('A → B → A → no Focus: actual payload retires references and keeps one focus across tools, retry and reopen',async()=>{
  const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'turn-focus-')));
  fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'focus',transcript:'transcript.jsonl',initialized:false}));
  const packets=[],captures=[];let round=0;
  const server=createServer(async(req,res)=>{
    let raw='';for await(const chunk of req)raw+=chunk;const packet=JSON.parse(raw);packets.push(packet);
    const tool=round===0&&!packet.messages.some(m=>m.role==='tool');
    const delta=tool?{tool_calls:[{index:0,id:'skills_0',type:'function',function:{name:'list_learning_skills',arguments:'{}'}}]}:{content:'回答 '+round};
    res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:tool?'tool_calls':'stop'}]})}\n\ndata: [DONE]\n\n`);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const body='两份不同文件的相同正文',hash=createHash('sha256').update(body).digest('hex');
  const run=async(id,runId='run_'+round)=>{
    const focus=id?{courseId:'course',documentId:id,sourceDocumentId:id,path:`documents/${id}.md`,type:'markdown',documentVersion:hash,commitHash:'',materialTreeOid:'',paragraphs:[]}:undefined;
    await runLearningAgent({protocol:1,operation:'generate',apiKey:'synthetic-focus-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture'},learningTools:true,learningSkills:[],toolPolicy:{mode:'question',authorization:'off',allowedTools:[...LEARNING_PLUGIN.tools]},debugProtocol:true,nativeSession:{directory,learningSessionId:'focus',runId,userEntryId:'user_'+round,authorizedSourceIds:['A','B'],protectHistory:false,legacyRecords:[]},messages:[{role:'user',content:JSON.stringify({question:'问题 '+round,focus,nativeReferences:round===0?'ONCE_ONLY_CITATION':'',nativeMaterials:id?[{id,path:focus.path,text:body,textHash:hash,version:hash,range:{startChar:0,endChar:[...body].length,complete:true,totalBytes:Buffer.byteLength(body)}}]:[],contextBridgeManifest:{version:1,entries:[]}})}]},e=>{if(e.type==='debug_request')captures.push(e);},{requestTool:async call=>{
      if(call.type==='session_sync')return {result:{synced:true}};
      if(call.type==='debug_probe')return {result:{epoch:1}};
      if(call.name==='list_learning_skills')return {result:{skills:[]}};
      assert.fail(JSON.stringify(call));
    }});
  };
  try {
    for(const id of ['A','B','A',null]){
      const start=packets.length;await run(id);
      for(const packet of packets.slice(start)){
        const values=packet.messages.map(text),focus=values.filter(v=>v.startsWith('【Focus：当前文件】'));
        assert.equal(focus.length,id?1:0);
        if(id)assert(focus[0].includes(`documents/${id}.md`));
        assert.equal(values.filter(v=>v.includes('ONCE_ONLY_CITATION')).length,round===0?1:0);
        for(let previous=0;previous<round;previous++)assert(values.includes('问题 '+previous),'legitimate native question history retained');
      }
      const current=captures.at(-1).sources.filter(s=>s.registrationKey==='focus');
      assert.equal(current.length,id?1:0,'debug maps one actual Focus registration');
      if(round===0){assert.equal(packets.length-start,2);assert.deepEqual(packets[start+1].messages.slice(0,packets[start].messages.length),packets[start].messages);}
      round++;
    }
    round=2;await run('A','retry_A');
    assert.equal(packets.at(-1).messages.map(text).filter(v=>v==='问题 2').length,1);
    assert.equal(packets.at(-1).messages.map(text).filter(v=>v.startsWith('【Focus：当前文件】')).length,1);
    const manager=SessionManager.open(path.join(directory,'transcript.jsonl'));
    const users=manager.getEntries().filter(e=>e.type==='message'&&e.message.role==='user');
    assert.equal(users.length,4);
    manager.appendCompaction('Synthetic compacted summary',users.at(-1).id,100);
    round=4;await run('B');
    const values=packets.at(-1).messages.map(text);
    assert.equal(values.filter(v=>v.startsWith('【Focus：当前文件】')).length,1);
    assert(values.some(v=>v.includes(body)),'lost baseline reloads without a second Focus');
    assert(manager.getEntries().some(e=>e.type==='custom_message'&&e.content.includes('ONCE_ONLY_CITATION')),'durable original reference is preserved');
  } finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));fs.rmSync(directory,{recursive:true,force:true});}
});

test('legacy Focus retirement preserves received diff offsets and ignores unsent candidates',async()=>{
  const {ContextBridge}=await import('../context-bridge.mjs');
  const {retireTurnContext,receivedMaterialFrames}=await import('../material-context.mjs');
  const manager=SessionManager.inMemory(),patch='@@ -1 +1 @@\n-旧\n+新😀\n',prefix='【Focus：当前文件】\n文件：documents/A.md\n';
  const hash=v=>createHash('sha256').update(v).digest('hex');
  const id=manager.appendCustomMessageEntry('learning-station-focus',prefix+patch,false,{bridgeKey:'focus',runId:'old_run',userEntryId:'old_user',sourceId:'A',path:'documents/A.md',materialFrames:[{kind:'diff',baseHash:'old',textHash:'new',patchHash:hash(patch),start:prefix.length,end:prefix.length+patch.length}]});
  manager.appendCustomEntry('learning-station-material-request',{nativeSessionId:manager.getSessionId(),entryId:id});
  const unsent=manager.appendCustomMessageEntry('learning-station-focus','【Focus：当前文件】\nNEVER_SENT',false,{bridgeKey:'focus'});
  retireTurnContext(manager,new ContextBridge(manager,{runId:'new_run',userEntryId:'new_user'}));
  const visible=manager.buildSessionProjection().entries.filter(e=>e.messages.length).map(e=>e.sourceEntry);
  assert(!visible.some(e=>e.id===id||e.id===unsent));
  const material=visible.find(e=>e.details?.materialFrames);
  assert.equal(receivedMaterialFrames(material)[0].unifiedDiff,patch);
  assert.equal(material.details.bridgeKey,'materials');
  assert.equal(material.details.userEntryId,'old_user');
  assert(manager.getEntries().some(e=>e.data?.entryId===material.id));
  assert(manager.getEntry(id),'original durable record is never rewritten');
});
