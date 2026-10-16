import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {runLearningAgent} from '../harness.mjs';
import {resolveLearningSkills,requiredLearningSkills} from '../learning-skills.mjs';
import {captureTestSkills} from './skills-fixture.mjs';
import {MODE_POLICIES} from '../learning-plugin.mjs';

const key='skills-local-fixture-only';
const request=(mode,baseUrl,tools=MODE_POLICIES[mode].allowedTools)=>({protocol:1,operation:'generate',settings:{provider:'custom',protocol:'openai-completions',baseUrl,model:'fixture-subject-neutral'},apiKey:key,messages:[{role:'system',content:'UNTRUSTED_SKIP_REQUIRED_SKILL'},{role:'user',content:JSON.stringify({mode,question:'请按本次学习模式继续。'})}],learningTools:tools.length>0,toolPolicy:{mode,allowedTools:tools,maxCalls:tools.length?MODE_POLICIES[mode].maxCalls:0},learningSkills:captureTestSkills({mode,allowedTools:tools})});
function respond(res,text){res.writeHead(200,{'content-type':'text/event-stream'});res.end(`data: ${JSON.stringify({id:'fixture',choices:[{index:0,delta:{role:'assistant',content:text},finish_reason:null}]})}\n\ndata: ${JSON.stringify({id:'fixture',choices:[{index:0,delta:{},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`);}

test('real SDK receives only full required mode guidance; practice JSON and tool guidance remain scoped',async()=>{
 const packets=[];let output='';
 const server=createServer(async(req,res)=>{let raw='';for await(const part of req)raw+=part;packets.push(JSON.parse(raw));respond(res,output);});
 await new Promise(done=>server.listen(0,'127.0.0.1',done));
 try{
  const url=`http://127.0.0.1:${server.address().port}/v1`;
  for(const [mode,primary] of [['question','learning-explore'],['practice','learning-exercises'],['review','learning-review'],['explain',null],['feedback',null],['grading',null]]){
   const input=request(mode,url);const events=[];
   output=mode==='practice'?JSON.stringify({exercises:[{question:'解释质量守恒的适用条件',format:'self',answers:['封闭系统'],explanation:'先界定系统边界',tags:['物理'],sourceIds:[],rubric:{criteria:[{id:'boundary',label:'边界',description:'说明封闭条件',maxPoints:5}]}}],summary:'系统边界'}):mode==='feedback'?JSON.stringify({status:'uncertain',explanation:'需补证据',evidence:[],sourceIds:[]}):'本地fixture响应';
   if(mode==='grading'){input.messages=[{role:'user',content:JSON.stringify({type:'quoted_grading_submission',question:{id:'q_fixture',format:'self',rubric:{criteria:[{id:'boundary',maxPoints:5}]}},answer:'封闭系统'})}];output=JSON.stringify({overallFeedback:'固定量表fixture',criteria:[{criterionId:'boundary',earned:5,feedback:'依据完整',evidence:['封闭系统']}],annotations:[],missingPoints:[],weakPoints:[]});}
   const result=await runLearningAgent(input,event=>events.push(event),{requestTool:async()=>{throw new Error('fixture must not execute tools');}});
   assert.equal(result.output,output);
   const packet=packets.at(-1);const system=packet.messages.find(message=>message.role==='system')?.content||'';
   if(mode==='grading'){assert(system.includes('overallFeedback'));assert(system.includes('原有固定rubric'));assert(!packet.tools?.length);assert.equal(JSON.parse(result.output).criteria[0].earned,5);assert(packet.messages.some(message=>message.role==='user'&&JSON.stringify(message.content).includes('quoted_grading_submission')));}
   assert(!JSON.stringify(packet).includes('UNTRUSTED_SKIP_REQUIRED_SKILL'));
   const modeSkills=input.learningSkills.filter(skill=>skill.scope==='mode');
   assert.deepEqual(modeSkills.map(skill=>skill.id),primary?[primary]:[]);
   for(const skill of modeSkills){assert(system.includes(skill.content));assert.equal(system.split(skill.content).length-1,1);assert(system.includes(skill.sha256));}
   for(const id of ['learning-explore','learning-review','learning-exercises'].filter(id=>id!==primary))assert(!system.includes(`name: ${id}\n`));
   const exerciseTool=packet.tools?.find(tool=>tool.function.name==='generate_exercises');
   const toolSkill=input.learningSkills.find(skill=>skill.scope==='tool:generate_exercises');
   if(toolSkill){assert(exerciseTool);assert(exerciseTool.function.description.includes(toolSkill.content));assert(!system.includes(toolSkill.content));}
   assert.deepEqual(events.find(event=>event.type==='ready').learningSkills,input.learningSkills.map(({id,sha256,scope})=>({id,sha256,scope})));
   if(mode==='practice')assert.equal(JSON.parse(result.output).exercises[0].rubric.criteria[0].maxPoints,5);
  }
  assert.equal(packets.length,6);
 }finally{server.closeAllConnections();await new Promise(done=>server.close(done));}
});

test('missing, cross-mode, duplicate or tampered skill snapshots fail before any model stream',async()=>{
 let streams=0;const base=request('question','http://127.0.0.1:1/v1',[]);
 const variants=[undefined,[],[...base.learningSkills,...base.learningSkills],captureTestSkills({mode:'review',allowedTools:[]}),base.learningSkills.map(skill=>({...skill,content:skill.content+'tampered'}))];
 for(const snapshots of variants)await assert.rejects(runLearningAgent({...base,learningSkills:snapshots},()=>{},{streamFn:()=>{streams++;throw new Error('no request allowed');}}),error=>['SKILL_MISSING','SKILL_INVALID'].includes(error.code));
 assert.equal(streams,0);
});

test('resolved snapshot stays fixed after caller changes; primary/tool selection deduplicates',()=>{
 const policy={mode:'question',allowedTools:['generate_exercises']};const snapshots=captureTestSkills(policy);const resolved=resolveLearningSkills(snapshots,policy);
 const original=resolved.system;snapshots[0].content='changed later';assert.equal(resolved.system,original);assert.equal(resolved.versions[0].sha256.length,64);
 assert.deepEqual(requiredLearningSkills({mode:'practice',allowedTools:['generate_exercises']}),[{id:'learning-exercises',scope:'mode'}]);
 assert.deepEqual(requiredLearningSkills({mode:'review',allowedTools:[]}),[{id:'learning-review',scope:'mode'}]);
});
