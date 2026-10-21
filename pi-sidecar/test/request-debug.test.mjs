import test from 'node:test';
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {createControlledFetch,runLearningAgent} from '../harness.mjs';
import {snapshotRequest,redactDebug,DEBUG_BODY_LIMIT} from '../request-debug.mjs';
const secret='synthetic-debug-key-ONLY';
const sse=text=>`data: ${JSON.stringify({choices:[{index:0,delta:{content:text},finish_reason:'stop'}]})}\n\ndata: [DONE]\n\n`;
async function endpoint(fn) {
  const packets=[];const server=createServer(async(req,res)=>{let raw='';for await(const part of req)raw+=part;packets.push({body:JSON.parse(raw),raw,headers:req.headers});await fn(req,res,packets.length);});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  return {url:`http://127.0.0.1:${server.address().port}`,packets,close:()=>new Promise(resolve=>{server.closeAllConnections();server.close(resolve);})};
}
test('final Request, not estimated context, matches endpoint; off neither clones nor collects; failures remain visible',async()=>{
  const server=await endpoint((_req,res,n)=>{res.writeHead(n===3?503:200,{'content-type':'text/event-stream'});res.end(sse('OK'));});
  try{
    const events=[];let enabled=false;let clones=0;const original=Request.prototype.clone;
    Request.prototype.clone=function(){clones++;return original.call(this);};
    try{
      const send=createControlledFetch({endpoint:server.url,apiKey:secret,fail:()=>{},debug:{phase:'initial',gate:async()=>enabled?7:null,emit:async e=>events.push(e)}});
      const payload={model:'fixture-model',system:'system',messages:[{role:'user',content:'Focus full\nChanges diff\ncitation supplement'}],tools:[{type:'function',function:{name:'fixture',parameters:{type:'object'}}}],max_tokens:91,stream:true,temperature:1};
      for(let i=0;i<3;i++){
        const req=new Request(server.url+'/chat/completions',{method:'POST',headers:{authorization:`Bearer ${secret}`,'content-type':'application/json'},body:JSON.stringify(payload)});
        if(i===2)await assert.rejects(send(req));else await (await send(req)).text();
        if(i===0){assert.equal(clones,0);assert.deepEqual(events,[]);enabled=true;}
      }
      assert.equal(clones,2);const records=events.filter(e=>e.type==='debug_request');assert.equal(records.length,2);
      assert.deepEqual(records[0].body,server.packets[1].body);assert.equal(records[0].headers.authorization,'[REDACTED]');
      assert.deepEqual(server.packets[0].raw,server.packets[1].raw);assert.equal(server.packets.length,3);
      assert.equal(events.at(-1).status,'HTTP 503');assert.doesNotMatch(JSON.stringify(events),new RegExp(secret));
    }finally{Request.prototype.clone=original;}
  }finally{await server.close();}
});
test('recursive redaction covers nested JSON, repeated secrets, headers, free text, URL and limits without altering original',async()=>{
  const source={api_key:'fixture-api',password:'fixture-password',accessToken:'fixture-token',messages:[{content:JSON.stringify({token:'fixture-inner',echo:'fixture-inner fixture-password fixture-api fixture-token '+secret})},{content:'Authorization: Bearer fixture-bearer password=fixture-pass sk-syntheticlongkey123'}],max_tokens:2048};
  const before=JSON.stringify(source);const safe=redactDebug(source,secret);assert.equal(JSON.stringify(source),before);
  assert.doesNotMatch(JSON.stringify(safe),/fixture-api|fixture-password|fixture-token|fixture-inner|fixture-bearer|fixture-pass|syntheticlongkey123|synthetic-debug-key/);
  assert.equal(safe.max_tokens,2048);
  const request=new Request('http://localhost/',{method:'POST',headers:{authorization:'Bearer '+secret,cookie:'session=fixture-cookie'},body:JSON.stringify({text:'x'.repeat(DEBUG_BODY_LIMIT+1)})});
  const record=await snapshotRequest(request,secret);assert.equal(record.complete,false);assert.equal(record.body,null);assert.match(record.omitted,/整个请求体/);assert.equal(record.headers.cookie,'[REDACTED]');
  assert.ok((await request.text()).length>DEBUG_BODY_LIMIT);
});
test('real Pi provider sends every tool turn and labels continuation; no extra model request',async()=>{
  const server=await endpoint((_req,res,n)=>{
    res.writeHead(200,{'content-type':'text/event-stream'});
    res.end(n===1?`data: ${JSON.stringify({choices:[{index:0,delta:{tool_calls:[{index:0,id:'fixture-call',type:'function',function:{name:'query_related_knowledge',arguments:'{"query":"fixture"}'}}]},finish_reason:'tool_calls'}]})}\n\ndata: [DONE]\n\n`:sse('OK'));
  });
  try{
    const events=[];const request={protocol:1,debugProtocol:true,operation:'generate',apiKey:secret,settings:{provider:'custom',protocol:'openai-completions',baseUrl:server.url,model:'fixture'},messages:[{role:'user',content:'fixture question'}],learningTools:true,toolPolicy:{mode:'explain',authorization:'off',allowedTools:['query_related_knowledge'],maxCalls:6}};
    const result=await runLearningAgent(request,async e=>events.push(e),{requestTool:async call=>call.type==='debug_probe'?{result:{epoch:1}}:{result:{items:[]}}});
    assert.equal(result.output,'OK');assert.equal(server.packets.length,2);
    const records=events.filter(e=>e.type==='debug_request');assert.equal(records.length,2);
    for(let i=0;i<2;i++)assert.deepEqual(records[i].body,server.packets[i].body);
    assert.equal(records[0].phase,'initial');assert.equal(records[1].phase,'tool_continuation');
    assert.equal(records[1].body.messages.at(-1).role,'tool');assert.ok(records[0].body.tools.length);
  }finally{await server.close();}
});
test('Anthropic actual system/tools/parameters captured, auth redacted',async()=>{
  const server=await endpoint((_req,res)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: fixture\n\n');});
  try{
    const events=[];const send=createControlledFetch({endpoint:server.url,apiKey:secret,protocol:'anthropic-messages',fail:()=>{},debug:{phase:'initial',gate:async()=>1,emit:async e=>events.push(e)}});
    const body={model:'fixture',system:[{type:'text',text:'system'}],messages:[{role:'user',content:'hello'}],tools:[{name:'fixture',input_schema:{type:'object'}}],max_tokens:9,stream:true};
    await(await send(server.url+'/v1/messages?beta=true',{method:'POST',headers:{'x-api-key':secret,'content-type':'application/json'},body:JSON.stringify(body)})).text();
    assert.deepEqual(events[0].body,server.packets[0].body);assert.equal(events[0].headers['x-api-key'],'[REDACTED]');
  }finally{await server.close();}
});
