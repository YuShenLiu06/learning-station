import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {createServer} from 'node:http';
import {SessionManager} from '@earendil-works/pi-coding-agent';
import {runLearningAgent} from '../harness.mjs';
import {LEARNING_PLUGIN} from '../learning-plugin.mjs';
import {openNativeSession} from '../native-session.mjs';
import {streamSimple} from '@earendil-works/pi-ai/api/openai-completions';

const hash=text=>createHash('sha256').update(text).digest('hex');
const text=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
const bodies=['test','test','test 测试 diff','test 测试 diff\n测试正文'];
const questions=['test','我们这个是在测试当前学习环境是否正常','test','。'];
const part=(key,value)=>({key,description:`合成回归：${key}`,value});

test('actual multi-turn HTTP wire keeps each bridge with its native question, including repeated text, tools, retry and compaction',{timeout:30000},async()=>{
  const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'context-chronology-')));
  fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'chronology',transcript:'transcript.jsonl',initialized:false}));
  const packets=[];let stage=0;
  const server=createServer(async(req,res)=>{
    let raw='';for await(const chunk of req)raw+=chunk;const packet=JSON.parse(raw);packets.push(packet);
    const toolsNeeded=stage===1&&packet.messages.at(-1).role!=='tool';
    const delta=toolsNeeded?{tool_calls:[
      {index:0,id:'read_fixture',type:'function',function:{name:'read',arguments:JSON.stringify({path:'missing.md'})}},
      {index:1,id:'skills_fixture',type:'function',function:{name:'list_learning_skills',arguments:'{}'}},
    ]}:{content:`answer-${stage}`};
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end(`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:toolsNeeded?'tool_calls':'stop'}]})}\n\ndata: [DONE]\n\n`);
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const clock=i=>({referenceTime:`2026-10-21T10:00:0${i}Z`,timeZone:'Asia/Shanghai'});
  const freeze=i=>[
    part('current_time',clock(i)),part('references',`QUOTE_${i}`),
    part('materials',[{id:'doc',path:'courses/doc.md',version:hash(bodies[i]),text:bodies[i],textHash:hash(bodies[i]),range:{startChar:0,endChar:[...bodies[i]].length,complete:true,totalBytes:Buffer.byteLength(bodies[i])},frames:[{kind:'full',version:hash(bodies[i]),textHash:hash(bodies[i]),text:bodies[i]}]}]),
    part('exploration',{scenario:'question-led-exploration',storageDocumentId:'doc',materialSourceIds:[]}),part('question',questions[i]),
  ];
  const captures=[];
  const run=async(i,runId=`run_${i}`)=>{
    const bridge=freeze(i);
    await runLearningAgent({protocol:1,operation:'generate',apiKey:'synthetic-chronology-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture'},timeContext:clock(i),learningTools:true,learningSkills:[],toolPolicy:{mode:'question',authorization:'read',allowedTools:[...LEARNING_PLUGIN.tools,'read'],maxCalls:6},debugProtocol:true,nativeSession:{directory,learningSessionId:'chronology',runId,userEntryId:`user_${i}`,authorizedSourceIds:['doc'],protectHistory:false,legacyRecords:[]},messages:[{role:'user',content:JSON.stringify({bridge,contextBridgeManifest:{version:1,entries:[]}})}]},event=>{if(event.type==='debug_request')captures.push(event);},{requestTool:async call=>{
      if(call.type==='session_sync')return {result:{synced:true}};
      if(call.type==='debug_probe')return {result:{epoch:process.env.LS_DBG_NULL?null:1}};
      if(call.name==='read')return {error:{code:'NOT_FOUND',message:'Synthetic missing workspace file'}};
      if(call.name==='list_learning_skills')return {result:{skills:[]}};
      assert.fail(`Unexpected synthetic request: ${call.type}/${call.name}`);
    }});
  };
  const label=m=>{
    const value=text(m);let parsed;try{parsed=JSON.parse(value);}catch{}
    if(value.startsWith('本轮冻结时间与时区\nreferenceTime: '))return `time:${value.split('\n')[1].slice(-3,-1)}`;
    if(parsed?.type==='automatic_material_projection')return `focus:${parsed.frames[0].text}`;
    if(parsed?.scenario)return 'exploration';
    if(value.startsWith('以下为当前用户正在看的内容'))return 'focus:'+value.split('\n').slice(2).join('\n');
    if(value.startsWith('【Exploration：'))return 'exploration';
    if(value.startsWith('用户本轮明确提供的引用及必要补充\n'))return 'user:'+value.split('\n').slice(1).join('\n');
    if(m.role==='assistant'&&m.tool_calls?.length)return 'assistant:tools';
    if(m.role==='tool')return `tool:${parsed?.ok===false?'NOT_FOUND':'skills'}`;
    return `${m.role}:${value}`;
  };
  // Exploration is session-scoped: only the opening turn registers it.
  const turn=i=>[`time:0${i}`,`user:QUOTE_${i}`,...(i===1?[]:[`focus:${bodies[i]}`]),...(i===0?['exploration']:[]),`user:${questions[i]}`];
  const history=[];
  try {
    for(stage=0;stage<4;stage++){
      const start=packets.length;await run(stage);
      const actual=packets[start].messages.filter(m=>m.role!=='system').map(label);
      if(process.env.LS_CONTEXT_WIRE_EVIDENCE){fs.mkdirSync(process.env.LS_CONTEXT_WIRE_EVIDENCE,{recursive:true});fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'chronology-wire.json'),JSON.stringify(packets,null,2));}
      assert.deepEqual(actual,[...history,...turn(stage)],`round ${stage}: historical bridge must remain before its own question`);
      history.push(...turn(stage));
      if(stage===1){
        const continuation=packets[start+1].messages.filter(m=>m.role!=='system').map(label);
        assert.deepEqual(continuation,[...history,'assistant:tools','tool:NOT_FOUND','tool:skills']);
        history.push('assistant:tools','tool:NOT_FOUND','tool:skills');
        // A click-to-finish run with a tool round captures every actual send,
        // each under its own id, instead of keeping only the latest request.
        const round=captures.slice(-2);
        assert.equal(round.length,2,'both sends of the tool run are captured');
        assert.notEqual(round[0].id,round[1].id);
        assert.equal(round[0].phase,'initial');
        assert.equal(round[1].phase,'tool_continuation');
        assert.deepEqual(round[1].body,packets[start+1]);
      }
      history.push(`assistant:answer-${stage}`);
    }
    // Same saved question, new run: no text matching and no duplicate historical quote/body.
    stage=4;await run(3,'retry_3');
    const retry=packets.at(-1).messages.filter(m=>m.role!=='system').map(label);
    assert.deepEqual(retry.slice(0,history.length),history);
    assert.equal(retry.length,history.length+1);
    assert.match(retry.at(-1),/继续处理上一条已保存用户问题/u);
    const native=SessionManager.open(path.join(directory,'transcript.jsonl'));
    const users=native.getEntries().filter(e=>e.type==='message'&&e.message.role==='user');
    assert.deepEqual(users.map(e=>text(e.message)),questions);
    // Pi legitimately drops the full baseline during compaction. Do not revive old quotes.
    native.appendCompaction('Synthetic summary contains no exact body',users.at(-1).id,100);
    stage=5;await run(3,'retry_after_compaction');
    const compacted=packets.at(-1).messages.filter(m=>m.role!=='system').map(label);
    const at=compacted.indexOf('user:。');
    // Compaction dropped the opening registration, so this turn re-anchors it.
    assert.deepEqual(compacted.slice(at-4,at),[...turn(3).slice(0,-1),'exploration']);
    assert(!compacted.includes('user:QUOTE_0'));
    assert(!compacted.includes('focus:test'));
    assert.equal(compacted.filter(v=>v==='user:。').length,1);
    if(process.env.LS_CONTEXT_WIRE_EVIDENCE)fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'chronology-wire.json'),JSON.stringify(packets,null,2));
  } finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));fs.rmSync(directory,{recursive:true,force:true});}
});

test('frozen input and registration receipts gate the actual provider; failed registration sends no partial request',{timeout:30000},async()=>{
  const packets=[];
  const server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;packets.push(JSON.parse(raw));res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'Frozen answer'},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  try {
    for(const failed of [false,true]){
      const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'context-freeze-barrier-')));
      fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'freeze_barrier',transcript:'transcript.jsonl',initialized:false}));
      const checkpoint='1'.repeat(40),tree='2'.repeat(40),body='FROZEN_BODY',before=packets.length;
      const entered=Promise.withResolvers(),release=Promise.withResolvers(),receipt=Promise.withResolvers(),releaseReceipt=Promise.withResolvers();
      const input={currentTime:{referenceTime:'2026-10-21T10:00:00Z',timeZone:'Asia/Shanghai'},nativeReferences:'FROZEN_SELECTION',focus:{courseId:'course',documentId:'doc',path:'documents/doc.md',type:'markdown',commitHash:checkpoint,materialTreeOid:tree,paragraphs:[{start:0,end:body.length,text:body}]},contextBridgeManifest:{version:1,entries:[]}};
      const native=await openNativeSession({directory,learningSessionId:'freeze_barrier',runId:'frozen_run',userEntryId:'frozen_user',authorizedSourceIds:['doc'],protectHistory:false,legacyRecords:[]},{
        model:{provider:'learning-custom',id:'fixture',name:'fixture',api:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,reasoning:false,input:['text'],contextWindow:0,maxTokens:1024,cost:{input:0,output:0,cacheRead:0,cacheWrite:0}},systemPrompt:'Synthetic course',tools:[],streamFn:(m,c,o)=>streamSimple(m,c,{...o,apiKey:'synthetic-freeze-key',maxRetries:0}),apiKey:'synthetic-freeze-key',thinkingLevel:'off',
        loadChanges:async()=>{entered.resolve();await release.promise;if(failed)throw Error('Synthetic registration failure');return {complete:true,checkpoint:{courseId:'course',checkpointCommitOid:checkpoint,materialTreeOid:tree,excludedFiles:[]},baseCommitOid:null,stat:'',focusPath:'documents/doc.md',changes:[],focused:{id:'doc',path:'documents/doc.md',version:hash(body),text:body,textHash:hash(body),range:{startChar:0,endChar:body.length,complete:true,totalBytes:body.length}}};},
        onManifest:async()=>{receipt.resolve();await releaseReceipt.promise;},
      });
      try {
        const registration=native.applyContext(input);await entered.promise;
        input.currentTime.referenceTime='UI_CHANGED_AFTER_CLICK';input.nativeReferences='UI_CHANGED_SELECTION';input.focus.documentId='UI_CHANGED_FOCUS';input.focus.paragraphs[0].text='UI_CHANGED_BODY';
        const answer=native.prompt('FROZEN_QUESTION');
        await new Promise(resolve=>setImmediate(resolve));
        assert.equal(packets.length,before,'provider waits while a bridge provider is paused');
        release.resolve();
        if(failed){
          const results=await Promise.allSettled([registration,answer]);assert(results.every(r=>r.status==='rejected'));
          assert.equal(packets.length,before,'registration failure cannot send a half-registered request');
          assert.equal(native.manager.buildSessionProjection().messages.filter(m=>m.role==='custom'&&m.details?.bridgeKey).length,0);
        } else {
          await receipt.promise;await new Promise(resolve=>setImmediate(resolve));
          assert.equal(packets.length,before,'provider also waits for the registration receipt');
          releaseReceipt.resolve();await Promise.all([registration,answer]);
          assert.equal(packets.length,before+1,'exactly one provider request after the complete registration receipt');
          const packet=packets.at(-1),wire=JSON.stringify(packet);
          assert(wire.includes('FROZEN_BODY')&&wire.includes('FROZEN_SELECTION')&&wire.includes('FROZEN_QUESTION'));
          assert(!wire.includes('UI_CHANGED_'));
          assert.equal(text(packet.messages.at(-1)),'FROZEN_QUESTION');
        }
      } finally {release.resolve();releaseReceipt.resolve();native.close();fs.rmSync(directory,{recursive:true,force:true});}
    }
  } finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
});
