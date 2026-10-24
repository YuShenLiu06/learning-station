import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mergeOutgoing, reconcileOutgoing, retryOperation, stableRunId } from '../src/outgoingMessages.ts';
import { foldToolActivities, historicalTools, readToolActivity, toolState } from '../src/toolActivities.ts';
import { sessionMessages } from '../src/sessionMessages.ts';
import { modelSelectionError } from '../src/modelPolicy.ts';
import { clampAssistantWidth, validAssistantWidth } from '../src/assistantWidth.ts';
import { feishuRequestId, feishuTestResult } from '../src/feishuPolicy.ts';

const outgoing = {sendId:'send_a',operationId:'op_a',sessionId:'session_a',sectionId:'section_a',detached:false,mode:'explain',content:'same text',selection:'',createdAt:'now',stage:'saving'};
const saved = {id:'entry_a',clientMessageId:'send_a',role:'user',content:'same text',createdAt:'now'};
const tool = {id:'activity_a',toolCallId:'call_a',runId:'run_a',sessionId:'session_a',name:'create_todo',status:'running',title:'unsafe title',summary:'secret raw arguments',startedAt:'2026-10-16T00:00:00Z',updatedAt:'2026-10-16T00:00:00Z',persisted:false};
const object = {kind:'todo',id:'todo_a',sessionId:'session_a',sectionId:'section_a'};

test('pending messages merge only by stable receipt IDs, preserving same-text sends and session isolation', () => {
  assert.equal(mergeOutgoing([], [outgoing], 'session_a')[0].outgoing.stage,'saving');
  assert.equal(mergeOutgoing([saved], [outgoing], 'session_a').length,1);
  assert.equal(mergeOutgoing([{...saved,clientMessageId:undefined}], [{...outgoing,userEntryId:'entry_a'}], 'session_a').length,1);
  assert.equal(mergeOutgoing([{...saved,clientMessageId:'different'}], [outgoing], 'session_a').length,2);
  assert.equal(mergeOutgoing([], [outgoing], 'session_b').length,0);
  assert.equal(reconcileOutgoing([outgoing],[saved],'session_a')[0].userEntryId,'entry_a');
  assert.equal(reconcileOutgoing([outgoing],[saved],'session_b')[0].userEntryId,undefined);
});
test('unknown sends keep their operation identity; only confirmed generation failures create a new run', async () => {
  assert.equal(retryOperation({...outgoing,failureStage:'save'},'new'),'op_a');
  assert.equal(retryOperation({...outgoing,failureStage:'unknown'},'new'),'op_a');
  assert.equal(retryOperation({...outgoing,failureStage:'generation'},'new'),'new');
  assert.equal(await stableRunId('中文-operation'),'run_'+createHash('sha256').update('中文-operation').digest('hex').slice(0,32));
});
test('tool success requires a real persisted object, while query success does not claim a write', () => {
  const parsed=readToolActivity({...tool,status:'succeeded'});
  assert.equal(toolState(parsed),'unconfirmed');
  const receipt=readToolActivity({...tool,status:'succeeded',persisted:true,object});
  assert.equal(toolState(receipt),'succeeded');
  const query=readToolActivity({...tool,name:'query_related_knowledge',status:'succeeded'});
  assert.equal(toolState(query),'succeeded');assert.equal(query.persisted,false);assert.match(query.summary,/未写入/);
  const loaded=readToolActivity({...tool,name:'load_learning_skill',status:'succeeded'});
  assert.equal(loaded.title,'加载学习指导');assert.equal(toolState(loaded),'succeeded');assert.match(loaded.summary,/未写入/);
  assert.equal(readToolActivity({...tool,name:'list_learning_skills',status:'succeeded'}).title,'查看学习指导目录');
  assert.equal(toolState(readToolActivity(tool)),'unconfirmed');
  assert.equal(toolState(readToolActivity(tool),'run_a'),'running');
});
test('tool cards whitelist metadata, collapse history, and never turn activity snapshots into learning prose', () => {
  const parsed=readToolActivity({...tool,status:'succeeded',persisted:true,object,args:{apiKey:'secret'},path:'/private/config'});
  assert.equal(parsed.title,'创建待办');assert.doesNotMatch(JSON.stringify(parsed),/secret|private|unsafe/);
  assert.equal(readToolActivity({...tool,id:'../../file'}),null);
  assert.equal(readToolActivity({...tool,name:'unregistered_tool'}),null);
  assert.equal(readToolActivity({...tool,object:{...object,id:'../file'}}).object,undefined);
  const terminal={...tool,status:'succeeded',persisted:true,object,updatedAt:'2026-10-16T00:00:01Z'};
  const entries=[tool,terminal,{...tool,updatedAt:'2026-10-16T00:00:02Z'},{...terminal,id:'other',sessionId:'session_b'}].map((value,i)=>({id:String(i),kind:'note',content:'not assistant prose',data:{origin:'pi-tool-activity',toolActivity:value}}));
  const history=historicalTools(entries,'session_a');assert.equal(history.length,1);assert.equal(history[0].status,'succeeded');
  assert.equal(foldToolActivities(entries.map(x=>x.data.toolActivity),'session_b').length,1);
  assert.equal(sessionMessages(entries).length,1, 'unassigned runs keep one clearly labelled receipt position each');
});
test('hand-entered model IDs validate UTF-8 limits and fixed GLM binding without touching connection fields', () => {
  const current={configured:true,provider:'custom',model:'old',baseUrl:'https://example.test/v1',protocol:'openai-completions'};
  const before={...current};assert.equal(modelSelectionError('new',current),'');assert.deepEqual(current,before);
  assert.match(modelSelectionError('',current),/模型/);assert.match(modelSelectionError('bad\nmodel',current),/模型/);
  assert.match(modelSelectionError('文'.repeat(86),current),/模型/);
  assert.match(modelSelectionError('new',{...current,configured:false}),/保存/);
  assert.match(modelSelectionError('other',{...current,provider:'glm-pi'}),/仅支持/);
  assert.equal(modelSelectionError('glm-5.3-flash',{...current,provider:'glm-pi'}),'');
});
test('assistant width remains within persisted and available-space bounds', () => {
  assert.equal(validAssistantWidth(280),true);assert.equal(validAssistantWidth(960),true);assert.equal(validAssistantWidth(346.5),false);
  assert.equal(clampAssistantWidth(100),280);assert.equal(clampAssistantWidth(1200),960);assert.equal(clampAssistantWidth(800,520),520);
});
test('Feishu never calls missing receipts success and reuses unknown request IDs for explicit checks', () => {
  const unknown=feishuTestResult({status:'sent',messageId:null,reason:'no receipt'});
  assert.equal(unknown.status,'unknown');assert.equal(feishuRequestId('original',unknown,'new'),'original');
  const sent=feishuTestResult({status:'sent',messageId:'msg_1',reason:'receipt'});
  assert.equal(sent.status,'sent');assert.equal(feishuRequestId('original',sent,'new'),'new');
});

test('tools stay with their saved question across failure, cancellation, retry and history clipping', async () => {
  const { toolsByMessage } = await import('../src/toolActivities.ts');
  const messages=[{id:'u1',role:'user'},{id:'u2',role:'user'},{id:'a2',role:'assistant',runId:'run_2'}];
  const calls=foldToolActivities([
    {...tool,id:'first',runId:'run_1',userEntryId:'u1',status:'failed',sequence:0},
    {...tool,id:'cancelled',runId:'run_1',userEntryId:'u1',sequence:1},
    {...tool,id:'retry_b',runId:'retry',userEntryId:'u1',sequence:1},
    {...tool,id:'retry_a',runId:'retry',userEntryId:'u1',sequence:0},
    {...tool,id:'second',runId:'run_2',userEntryId:'u2',status:'succeeded'},
  ],'session_a');
  const grouped=toolsByMessage(messages,calls);
  assert.deepEqual(grouped.get('u1').map(x=>x.id),['first','cancelled','retry_a','retry_b']);
  assert.deepEqual(grouped.get('u2').map(x=>x.id),['second']);
  assert.equal(grouped.has('a2'),false);
  const window=toolsByMessage(messages.slice(-1),calls);
  assert.deepEqual([...window.keys()],['a2']);
  assert.deepEqual(window.get('a2').map(x=>x.id),['second']);
  assert.equal(toolState(calls[1]),'unconfirmed');
  assert.equal(toolState(calls[2],'retry'),'running');
});

test('unbound legacy receipts remain at their recorded position without guessing their question', async () => {
  const { toolsByMessage } = await import('../src/toolActivities.ts');
  const entry=(id,kind,data={})=>({id,kind,content:'fixture',createdAt:'same',data});
  const entries=[entry('u1','user'),entry('receipt','note',{origin:'pi-tool-activity',toolActivity:tool}),entry('u2','user'),entry('terminal','note',{origin:'pi-tool-activity',toolActivity:{...tool,status:'failed'}})];
  const messages=sessionMessages(entries);
  assert.deepEqual(messages.map(x=>x.id),['u1','receipt','u2']);
  assert.equal(messages[1].toolRunId,'run_a');
  assert.equal(messages[1].content,'');
  assert.deepEqual([...toolsByMessage(messages,historicalTools(entries,'session_a')).keys()],['receipt']);
  entries.push(entry('answer','assistant',{runId:'run_a'}));
  assert.deepEqual(sessionMessages(entries).map(x=>x.id),['u1','u2','answer']);
  assert.deepEqual([...toolsByMessage(sessionMessages(entries),historicalTools(entries,'session_a')).keys()],['answer']);
});


test('an old run snapshot cannot erase attribution enriched from the durable history', () => {
  const calls=foldToolActivities([{...tool,userEntryId:'u1',sequence:2},tool],'session_a');
  assert.equal(calls.length,1);
  assert.equal(calls[0].userEntryId,'u1');
  assert.equal(calls[0].sequence,2);
});
