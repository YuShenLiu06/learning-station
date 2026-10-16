import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createModel, resolveModelLimits, runLearningAgent, MODEL_ID} from '../harness.mjs';
const KEY='fixture-model-limits-not-real';
const settings=(provider='custom', protocol='openai-completions')=>({provider,protocol,baseUrl:'http://127.0.0.1:1',model:provider==='custom'?'fixture-model':MODEL_ID});
const frame=(delta={},finish=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:finish}]})}\n\n`;
async function fixture(action, respond) {
 let requests=0;const server=createServer(async(req,res)=>{requests++;let raw='';for await(const part of req)raw+=part;const payload=JSON.parse(raw);await respond(req,res,payload);});
 await new Promise(r=>server.listen(0,'127.0.0.1',r));
 try {await action(`http://127.0.0.1:${server.address().port}`);return requests;}finally{server.closeAllConnections();await new Promise(r=>server.close(r));}
}
const anthropic=()=>[{type:'message_start',message:{id:'fixture',type:'message',role:'assistant',model:'fixture-model',content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:1,output_tokens:0}}},{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:'OK'}},{type:'content_block_stop',index:0},{type:'message_delta',delta:{stop_reason:'end_turn',stop_sequence:null},usage:{output_tokens:1}},{type:'message_stop'}].map(e=>`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`).join('');
test('legacy budgets stay compatible without advertising guessed context capacity',()=>{
 for(const [provider,max,cap] of [['custom',4096,256],['glm-pi',32768,2048]]) {
  assert.deepEqual(resolveModelLimits(settings(provider)),{maxTokens:max,contextWindow:0});
  assert.equal(createModel(settings(provider),'test').maxTokens,cap);
  assert.deepEqual(resolveModelLimits({...settings(provider),maxOutputTokens:1,contextWindow:null},'test'),{maxTokens:1,contextWindow:0});
 }
 assert.deepEqual(resolveModelLimits({...settings(),maxOutputTokens:131072,contextWindow:132096}),{maxTokens:131072,contextWindow:132096});
});
test('invalid integer, range and input-reserve limits fail before stream or request',async()=>{
 for(const patch of [{maxOutputTokens:0},{maxOutputTokens:131073},{maxOutputTokens:1.5},{maxOutputTokens:'8192'},{contextWindow:1023},{contextWindow:2000001},{contextWindow:5.5},{maxOutputTokens:8192,contextWindow:9215}]) {
  let streamed=0;
  await assert.rejects(runLearningAgent({protocol:1,operation:'test',settings:{...settings(),...patch},apiKey:KEY,messages:[]},()=>{},{streamFn:()=>{streamed++;}}),e=>e.code==='INVALID_MODEL_LIMITS');
  assert.equal(streamed,0);
 }
});
for(const [provider,protocol,operation,max,expected] of [
 ['custom','openai-completions','generate',12288,12288],['custom','anthropic-messages','generate',8192,8192],
 ['glm-pi','openai-completions','generate',6144,6144],['custom','openai-completions','test',8192,256],
 ['custom','anthropic-messages','test',64,64],['glm-pi','openai-completions','test',8192,2048],
 ['glm-pi','openai-completions','test',64,64],
]) test(`real pinned Pi wire preserves ${provider}/${protocol}/${operation} budget ${max} -> ${expected}`,async()=>{
 const events=[];
 const count=await fixture(async(base)=>{
  const result=await runLearningAgent({protocol:1,operation,settings:{...settings(provider,protocol),baseUrl:base,maxOutputTokens:max,contextWindow:null},apiKey:KEY,messages:[{role:'user',content:'fixture only'}]},e=>events.push(e));
  assert.equal(result.output,'OK');assert.equal(events.at(-1).diagnostic.maxTokens,expected);
 },async(req,res,body)=>{
  assert.equal(body.max_tokens,expected);assert.equal(body.max_completion_tokens,undefined);
  assert.equal(body.model,provider==='custom'?'fixture-model':MODEL_ID);assert.equal(body.stream,true);
  assert.equal(req.url,protocol==='anthropic-messages'?'/v1/messages?beta=true':'/chat/completions');
  assert.equal(body.tools,undefined);res.writeHead(200,{'content-type':'text/event-stream'});
  res.end(protocol==='anthropic-messages'?anthropic():frame({content:'OK'},'stop')+'data: [DONE]\n\n');
 });assert.equal(count,1);assert(!JSON.stringify(events).includes(KEY));
});
test('configured length stop with reasoning-only output remains length, not network truncation or success',async()=>{
 const count=await fixture(async(base)=>{
  const events=[];
  await assert.rejects(runLearningAgent({protocol:1,operation:'generate',settings:{...settings(),baseUrl:base,maxOutputTokens:7777,contextWindow:null},apiKey:KEY,messages:[{role:'user',content:'fixture only'}]},e=>events.push(e)),e=>e.code==='MODEL_LENGTH'&&e.diagnostic.maxTokens===7777&&e.diagnostic.finishReason==='length'&&e.diagnostic.textChars===0&&e.diagnostic.thinkingChars>0);
  assert(!events.some(e=>e.type==='done'));
 },async(req,res,body)=>{assert.equal(body.max_tokens,7777);res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({reasoning_content:'fixture reasoning'},'length')+'data: [DONE]\n\n');});assert.equal(count,1);
});
