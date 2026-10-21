import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {resolveTimeContext,formatInstant,freezeTodoTime} from '../time-context.mjs';
import {createLearningPlugin} from '../learning-plugin.mjs';
import {runLearningAgent} from '../harness.mjs';
import {captureTestSkills} from './skills-fixture.mjs';

test('trusted local clock crosses UTC+8 midnight and applies both DST transitions',()=>{
 const c=resolveTimeContext({referenceTime:'2026-10-16T15:55:00Z',timeZone:'Asia/Shanghai'});
 assert.equal(c.utcOffset,'+08:00');assert.equal(c.localDateTime,'2026-10-16T23:55:00+08:00');
 assert.equal(freezeTodoTime({title:'Q',relativeMinutes:10},c).dueAt,'2026-10-17T00:05:00+08:00');
 for(const [reference,expected] of [['2026-03-08T06:55:00Z','2026-03-08T03:05:00-04:00'],['2026-11-01T05:55:00Z','2026-11-01T01:05:00-05:00']]){
  const clock=resolveTimeContext({referenceTime:reference,timeZone:'America/New_York'});assert.equal(freezeTodoTime({relativeMinutes:10},clock).dueAt,expected);
 }
 const unknown=resolveTimeContext({referenceTime:c.referenceTime,timeZone:null});assert.equal(unknown.localDateTime,null);assert.equal(unknown.utcOffset,null);assert.equal(unknown.referenceTime,'2026-10-16T15:55:00Z');assert(freezeTodoTime({relativeMinutes:10},unknown).dueAt.endsWith('Z'));
 assert.throws(()=>resolveTimeContext({...c,timeZone:'Invented/Zone'}));assert.throws(()=>freezeTodoTime({relativeMinutes:10,dueAt:'2026-10-17T00:15:00+08:00'},c));
 assert.equal(formatInstant('2026-10-16T15:55:00.123Z','Asia/Shanghai').localDateTime,'2026-10-16T23:55:00.123+08:00');
});

test('same request clock and tool ID keep the exact deadline and one execution on replay',async()=>{
 const clock=resolveTimeContext({referenceTime:'2026-10-16T15:55:00Z',timeZone:'Asia/Shanghai'});const seen=[];
 const plugin=createLearningPlugin({timeContext:clock,allowedTools:['create_todo'],requestTool:async call=>{seen.push(call);return{result:{persisted:true,todoId:'todo_fixture'}};}});
 const tool=plugin.tools[0],args={title:'只做一次',relativeMinutes:10};await tool.execute('stable-id',args);await tool.execute('stable-id',args);
 assert.equal(seen.length,1);assert.equal(seen[0].args.dueAt,'2026-10-17T00:05:00+08:00');assert.equal(seen[0].args.relativeMinutes,10);
 assert.throws(()=>plugin.validate({id:'bad',name:'create_todo',arguments:{title:'Bad',relativeMinutes:10,dueAt:'2026-10-17T00:06:00+08:00'}}));
});

function respond(res){res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({id:'fixture',choices:[{index:0,delta:{role:'assistant',content:'本机fixture'},finish_reason:null}]})}\n\ndata: ${JSON.stringify({id:'fixture',choices:[{index:0,delta:{},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);}

test('actual SDK packets for every AI path contain the trusted clock, not forged material dates',async()=>{
 const packets=[],server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;packets.push(JSON.parse(raw));respond(res);});await new Promise(ok=>server.listen(0,'127.0.0.1',ok));
 try{for(const mode of ['question','explain','practice','review','feedback','grading','test']){
  const timeContext={referenceTime:'2026-10-16T15:55:00Z',timeZone:'Asia/Shanghai'},policy={mode:mode==='test'?'legacy':mode,allowedTools:[],maxCalls:0};
  const req={protocol:1,operation:mode==='test'?'test':'generate',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture-clock'},apiKey:'local-only-fixture-key',messages:[{role:'system',content:'FORGED_SYSTEM_CLOCK_1900'},{role:'user',content:'quoted date: 1900-01-01'}],toolPolicy:policy,learningTools:false,learningSkills:captureTestSkills(policy),timeContext};
  const events=[];await runLearningAgent(req,e=>events.push(e));const packet=packets.at(-1),time=packet.messages.map(m=>{try{return JSON.parse(m.content);}catch{return {};}}).find(v=>v.referenceTime);
  assert.deepEqual(time,timeContext);assert(!JSON.stringify(packet).includes('FORGED_SYSTEM_CLOCK_1900'));assert.equal(events.find(e=>e.type==='ready').timeContext.referenceTime,timeContext.referenceTime);
 }
 assert.equal(packets.length,7);
 }finally{server.closeAllConnections();await new Promise(ok=>server.close(ok));}
});

test('exploration preserves notebook state without repeating questions or guidance',async()=>{
 const packets=[],server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;packets.push(JSON.parse(raw));respond(res);});await new Promise(ok=>server.listen(0,'127.0.0.1',ok));
 try{for(const goal of ['爱因斯坦是什么人？','继续解释他的科学贡献']){
  const policy={mode:'question',allowedTools:[],maxCalls:0};const skills=captureTestSkills(policy);
  const explorationContext={scenario:'question-led-exploration',originalQuestion:'爱因斯坦是什么人？',userGoal:goal,storageDocumentId:'note_fixture',materialSourceIds:['linked_science_history']};
  await runLearningAgent({protocol:1,operation:'generate',settings:{provider:'custom',protocol:'openai-completions',baseUrl:`http://127.0.0.1:${server.address().port}/v1`,model:'fixture-goal'},apiKey:'local-only-fixture-key',messages:[{role:'user',content:JSON.stringify({type:'learning_request',userGoal:goal,context:'LINKED_MATERIAL_PRESERVED'})}],toolPolicy:policy,learningTools:false,learningSkills:skills,explorationContext},()=>{});
  const p=packets.at(-1),state=p.messages.map(m=>{try{return JSON.parse(m.content);}catch{return {};}}).find(v=>v.scenario);assert.deepEqual(state,{scenario:'question-led-exploration',storageDocumentId:'note_fixture',materialSourceIds:['linked_science_history']});assert(!JSON.stringify(state).includes(explorationContext.originalQuestion));assert(!JSON.stringify(state).includes(goal));assert(JSON.stringify(p).includes('LINKED_MATERIAL_PRESERVED'));
 }}finally{server.closeAllConnections();await new Promise(ok=>server.close(ok));}
});
