import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { runLearningAgent } from '../harness.mjs';
import { safeResponseDiagnostic } from '../response-diagnostics.mjs';
const KEY='fixture-response-key-not-real';
const input=(baseUrl,protocol='openai-completions')=>({protocol:1,operation:'test',settings:{provider:'custom',protocol,baseUrl,model:'glm-5.3-flash'},apiKey:KEY,messages:[]});
const frame=(delta={},reason=null)=>`data: ${JSON.stringify({choices:[{index:0,delta,finish_reason:reason}]})}\n\n`;
async function fixture(handler,action){const server=createServer(handler);await new Promise(r=>server.listen(0,'127.0.0.1',r));try{return await action(`http://127.0.0.1:${server.address().port}`);}finally{server.closeAllConnections();await new Promise(r=>server.close(r));}}
async function payload(req){let raw='';for await(const part of req)raw+=part;return JSON.parse(raw);}
const cases=[
 ['HTML page','text/html','<!doctype html><html>synthetic gateway page</html>','MODEL_ENDPOINT_PAGE'],
 ['JSON instead of SSE','application/json',JSON.stringify({choices:[{message:{content:'OK'},finish_reason:'stop'}]}),'MODEL_FORMAT'],
 ['thinking only','text/event-stream',frame({reasoning_content:'synthetic thinking'},'stop')+'data: [DONE]\n\n','MODEL_THINKING_ONLY'],
 ['thinking reaches length','text/event-stream',frame({reasoning_content:'synthetic thinking'},'length')+'data: [DONE]\n\n','MODEL_LENGTH'],
 ['short partial reaches length','text/event-stream',frame({content:'abc'},'length')+'data: [DONE]\n\n','MODEL_LENGTH'],
 ['short partial loses finish at EOF','text/event-stream',frame({content:'abc'}),'MODEL_INCOMPLETE'],
 ['empty completed answer','text/event-stream',frame({},'stop')+'data: [DONE]\n\n','MODEL_EMPTY'],
 ['DONE alone is not an answer','text/event-stream','data: [DONE]\n\n','MODEL_INCOMPLETE'],
];
for(const [name,mime,body,code] of cases)test(`response diagnostics distinguish ${name} without settled success`,async()=>{
 let requests=0;
 await fixture(async(req,res)=>{requests++;const sent=await payload(req);assert.equal(sent.max_tokens,256);assert.equal(sent.model,'glm-5.3-flash');assert.equal(req.url,'/chat/completions');res.writeHead(200,{'content-type':mime});res.end(body);},async(base)=>{
  const events=[];let observed;
  await assert.rejects(runLearningAgent(input(base),e=>events.push(e)),error=>{observed=error.diagnostic;return error.code===code;});
  assert.equal(observed.httpStatus,200);assert.equal(observed.requestPath,'/chat/completions');assert.equal(observed.maxTokens,256);
  assert(!events.some(e=>e.type==='done'));assert(!JSON.stringify({events,observed}).includes(KEY));
  if(body.includes('abc'))assert.equal(events.filter(e=>e.type==='delta').map(e=>e.delta).join(''),'abc');
 });assert.equal(requests,1);
});
test('mixed reasoning/content, usage-only trailer and DONE complete without leaking reasoning',async()=>{
 await fixture(async(req,res)=>{await payload(req);res.writeHead(200,{'content-type':'text/event-stream'});const body=frame({reasoning_content:'never display reasoning'})+frame({content:'OK'},'stop')+'data: {"choices":[],"usage":{"completion_tokens":25,"completion_tokens_details":{"reasoning_tokens":23}}}\n\ndata: [DONE]\n\n';for(const byte of Buffer.from(body))res.write(Buffer.of(byte));res.end();},async(base)=>{const events=[];const result=await runLearningAgent(input(base+'/v1'),e=>events.push(e));assert.equal(result.output,'OK');const done=events.at(-1);assert.equal(done.diagnostic.finishReason,'stop');assert.equal(done.diagnostic.sawDone,true);assert.equal(done.diagnostic.usageReasoningTokens,23);assert(!JSON.stringify(events).includes('never display reasoning'));});
});
test('OpenAI semantic stop plus clean EOF is valid without relaxing missing-finish checks',async()=>{
 await fixture(async(req,res)=>{await payload(req);res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({content:'OK'},'stop'));},async(base)=>{const events=[];await runLearningAgent(input(base),e=>events.push(e));assert.equal(events.at(-1).type,'done');assert.equal(events.at(-1).diagnostic.sawDone,false);});
});
test('aborted short partial preserves usable text while redacting a partial credential suffix',async()=>{
 await fixture(async(req,res)=>{await payload(req);res.writeHead(200,{'content-type':'text/event-stream'});res.end(frame({content:'abc '+KEY.slice(0,12)}));},async(base)=>{const events=[];await assert.rejects(runLearningAgent(input(base),e=>events.push(e)),e=>e.code==='MODEL_INCOMPLETE');const text=events.filter(e=>e.type==='delta').map(e=>e.delta).join('');assert.equal(text,'abc [REDACTED]');assert(!JSON.stringify(events).includes(KEY.slice(0,12)));});
});
test('diagnostics accept only fixed metadata fields and never arbitrary headers or body',()=>{
 const value=safeResponseDiagnostic({operation:'test',protocol:'openai-completions',httpStatus:200,finishReason:KEY,requestPath:KEY,content:KEY,headers:{authorization:KEY},rawError:KEY,textChars:3,thinkingChars:-5,maxTokens:Number.MAX_SAFE_INTEGER});
 assert.deepEqual(value,{version:1,operation:'test',protocol:'openai-completions',httpStatus:200,textChars:3});assert(!JSON.stringify(value).includes(KEY));
});
function anthropic(text='OK',finish='end_turn',complete=true){const events=[{type:'message_start',message:{id:'fixture',type:'message',role:'assistant',model:'glm-5.3-flash',content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:1,output_tokens:0}}},{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text}},{type:'content_block_stop',index:0},{type:'message_delta',delta:{stop_reason:finish,stop_sequence:null},usage:{output_tokens:2}},...(complete?[{type:'message_stop'}]:[])];return events.map(event=>`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join('');}
test('real Pi Anthropic SDK uses exact configured proxy target and API-key headers once',async()=>{
 let requests=0;await fixture(async(req,res)=>{requests++;const body=await payload(req);assert.equal(req.url,'/proxy/v1/messages?beta=true');assert.equal(req.headers['x-api-key'],KEY);assert.equal(req.headers.authorization,undefined);assert.equal(req.headers['anthropic-version'],'2023-06-01');assert.equal(body.model,'glm-5.3-flash');assert.equal(body.stream,true);assert.equal(body.tools,undefined);res.writeHead(200,{'content-type':'text/event-stream'});res.end(anthropic());},async(base)=>{const events=[];const result=await runLearningAgent(input(base+'/proxy','anthropic-messages'),e=>events.push(e));assert.equal(result.output,'OK');assert.equal(events.at(-1).diagnostic.messageStop,true);assert.equal(events.at(-1).diagnostic.finishReason,'end_turn');});assert.equal(requests,1);
});
for(const [finish,complete,code] of [['max_tokens',true,'MODEL_LENGTH'],['end_turn',false,'MODEL_INCOMPLETE']])test(`Anthropic ${finish}/${complete} cannot falsely complete`,async()=>{
 await fixture(async(req,res)=>{await payload(req);res.writeHead(200,{'content-type':'text/event-stream'});res.end(anthropic('abc',finish,complete));},async(base)=>{const events=[];await assert.rejects(runLearningAgent(input(base,'anthropic-messages'),e=>events.push(e)),e=>e.code===code);assert.equal(events.filter(e=>e.type==='delta').map(e=>e.delta).join(''),'abc');assert(!events.some(e=>e.type==='done'));});
});
test('Anthropic disallows OAuth-shaped secrets and duplicate /v1 without a request',async()=>{for(const [path,key]of[['/v1',KEY],['','sk-ant-oat-fixture-only']]){let requests=0;await fixture((req,res)=>{requests++;res.end();},async(base)=>{const value=input(base+path,'anthropic-messages');value.apiKey=key;await assert.rejects(runLearningAgent(value,()=>{}),e=>e.code==='INVALID_MODEL_SETTINGS');});assert.equal(requests,0);}});
