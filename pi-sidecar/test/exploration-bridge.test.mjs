import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createServer} from 'node:http';
import {SessionManager} from '@earendil-works/pi-coding-agent';
import {runLearningAgent} from '../harness.mjs';
import {LEARNING_PLUGIN} from '../learning-plugin.mjs';
const MARK='【Exploration：当前任务入口】';
const clock={referenceTime:'2026-10-22T09:00:00Z',timeZone:'Etc/UTC'};
const text=message=>typeof message.content==='string'?message.content:(message.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
// The exploration entry is session-scoped: it registers on the opening turn,
// survives follow-ups and same-question retries without re-injection, and
// re-anchors on the current turn only after Pi compaction removed the original.
test('exploration bridge registers once per session and re-anchors only after compaction',async()=>{
  const directory=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'exploration-bridge-')));
  const file=path.join(directory,'transcript.jsonl');
  fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'exploration_session',transcript:'transcript.jsonl',initialized:false}));
  const packets=[];
  const server=createServer(async(req,res)=>{let raw='';for await(const part of req)raw+=part;packets.push(JSON.parse(raw));res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({choices:[{index:0,delta:{content:'OK'},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const rounds=[
    {runId:'opening_run',userEntryId:'opening_user',question:'OPENING_QUESTION'},
    {runId:'followup_run',userEntryId:'followup_user',question:'FOLLOWUP_QUESTION'},
    {runId:'followup_retry_run',userEntryId:'followup_user',question:'FOLLOWUP_QUESTION'},
    {runId:'post_compaction_run',userEntryId:'post_compaction_user',question:'POST_COMPACTION_QUESTION'},
  ];
  try {
    for(const [index,round] of rounds.entries()) {
      if(index===rounds.length-1) {
        const manager=SessionManager.open(file);
        manager.appendCompaction('Synthetic summary drops the opening bridge',manager.getEntries().filter(e=>e.type==='message').at(-1).id,0);
      }
      const exploration={key:'exploration',description:'探索入口的任务位置与来源',value:{scenario:'question-led-exploration',storageDocumentId:'node_exploration_note',materialSourceIds:[]}};
      const frozen=[{key:'current_time',description:'本轮时间',value:{...clock,referenceTime:`2026-10-22T09:00:0${index}Z`}},exploration,{key:'question',description:'仅作为原生用户消息发送',value:round.question}];
      const request={protocol:1,operation:'generate',apiKey:'synthetic-exploration-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture'},timeContext:clock,learningTools:true,learningSkills:[],toolPolicy:{mode:'question',authorization:'off',allowedTools:LEARNING_PLUGIN.tools,maxCalls:6},nativeSession:{directory,learningSessionId:'exploration_session',runId:round.runId,userEntryId:round.userEntryId,authorizedSourceIds:[],protectHistory:false,legacyRecords:[]},messages:[{role:'user',content:JSON.stringify({bridge:frozen,contextBridgeManifest:{version:1,entries:[]}})}]};
      await runLearningAgent(request,()=>{},{requestTool:async call=>{
        if(call.type==='session_sync')return {result:{synced:true}};
        throw new Error('unexpected sidecar tool call '+call.type);
      }});
      const texts=packets[index].messages.map(text);
      assert.equal(texts.filter(value=>value.includes(MARK)).length,1,`round ${index} wire carries exactly one exploration entry`);
      if(index===rounds.length-2) {
        // Before compaction the durable projection holds exactly the opening
        // registration; the follow-up and its retry re-registered nothing.
        const live=SessionManager.open(file).buildSessionProjection().entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.details?.bridgeKey==='exploration');
        assert.deepEqual(live.map(e=>e.sourceEntry.details.userEntryId),['opening_user']);
      }
    }
    // After compaction dropped the opening entry, the next turn re-anchored the
    // session origin exactly once, on its own question.
    const projection=SessionManager.open(file).buildSessionProjection();
    const registered=projection.entries.filter(e=>e.messages.length&&e.sourceEntry.type==='custom_message'&&e.sourceEntry.details?.bridgeKey==='exploration');
    assert.deepEqual(registered.map(e=>e.sourceEntry.details.userEntryId),['post_compaction_user']);
  } finally {server.closeAllConnections();await new Promise(resolve=>server.close(resolve));fs.rmSync(directory,{recursive:true,force:true});}
});
