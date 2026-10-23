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
    const events=[];let enabled=false;let clones=0;let sourceCalls=0;const original=Request.prototype.clone;
    Request.prototype.clone=function(){clones++;return original.call(this);};
    try{
      const send=createControlledFetch({endpoint:server.url,apiKey:secret,fail:()=>{},debug:{phase:'initial',gate:async()=>enabled?7:null,sources:()=>{sourceCalls++;return{};},emit:async e=>events.push(e)}});
      const payload={model:'fixture-model',system:'system',messages:[{role:'user',content:'Focus full\nChanges diff\ncitation supplement'}],tools:[{type:'function',function:{name:'fixture',parameters:{type:'object'}}}],max_tokens:91,stream:true,temperature:1};
      for(let i=0;i<3;i++){
        const req=new Request(server.url+'/chat/completions',{method:'POST',headers:{authorization:`Bearer ${secret}`,'content-type':'application/json'},body:JSON.stringify(payload)});
        if(i===2)await assert.rejects(send(req));else await (await send(req)).text();
        if(i===0){assert.equal(clones,0);assert.equal(sourceCalls,0);assert.deepEqual(events,[]);enabled=true;}
      }
      assert.equal(clones,2);assert.equal(sourceCalls,2);const records=events.filter(e=>e.type==='debug_request');assert.equal(records.length,2);
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
    const events=[];const request={protocol:1,debugProtocol:true,operation:'generate',apiKey:secret,settings:{provider:'custom',protocol:'openai-completions',baseUrl:server.url,model:'fixture'},messages:[{role:'user',content:'fixture question'}],learningTools:true,toolPolicy:{mode:'explain',authorization:'off',allowedTools:['query_related_knowledge']}};
    const result=await runLearningAgent(request,async e=>events.push(e),{requestTool:async call=>call.type==='debug_probe'?{result:{epoch:1}}:{result:{items:[]}}});
    assert.equal(result.output,'OK');assert.equal(server.packets.length,2);
    const records=events.filter(e=>e.type==='debug_request');assert.equal(records.length,2);
    for(let i=0;i<2;i++)assert.deepEqual(records[i].body,server.packets[i].body);
    assert.equal(records[0].phase,'initial');assert.equal(records[1].phase,'tool_continuation');
    assert.equal(records[1].body.messages.at(-1).role,'tool');assert.ok(records[0].body.tools.length);
    const toolCalls=records[1].body.messages.findIndex(m=>m.role==='assistant'&&m.tool_calls?.length);
    assert.ok(toolCalls>=0);assert.ok(records[1].sources.some(s=>s.path[0]==='messages'&&s.path[1]===toolCalls&&s.origin==='model'&&s.name.includes('tool_calls')));
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
test('embedded JSON, quoted assignments, auth schemes, cookies and camel credential fields stay redacted before IPC',async()=>{
  const body={messages:[{content:'```json\n{"password":"private-fenced-pass","api_key":"private-fenced-key","token":"private-fenced-token"}\n```'},{content:'password="private-quoted-pass" token=\'private-quoted-token\''},{content:'Authorization: ApiKey private-auth-value\nCookie: session=private-cookie-one; auth=private-cookie-two'}],metadata:{sessionToken:'private-session-token',idToken:'private-id-token',userPassword:'private-user-pass'}};
  const before=JSON.stringify(body);const safe=redactDebug(body,secret);assert.equal(JSON.stringify(body),before);assert.doesNotMatch(JSON.stringify(safe),/private-[a-z-]+/);
  const req=new Request('http://localhost/',{method:'POST',body:JSON.stringify({password:'D',token:'E',secret:'R',accessToken:'A',refreshToken:'C',api_key:'T',text:'D'.repeat(210000)})});
  const snapshot=await snapshotRequest(req,secret);assert.equal(snapshot.complete,false);assert.match(snapshot.omitted,/脱敏后超过/);assert.ok(JSON.stringify(snapshot).length<1000);
});
test('quoted Cookie and Digest authentication lines are entirely hidden, including repeated values',()=>{
  const body={text:'Authorization: Digest username="private-digest-user", nonce="private-digest-nonce", response="private-digest-secret"\nCookie: session="private-cookie-one"; auth="private-cookie-two"',echo:'private-digest-user private-digest-nonce private-digest-secret private-cookie-one private-cookie-two'};
  assert.doesNotMatch(JSON.stringify(redactDebug(body,secret)),/private-[a-z-]+/);
});
test('actual native projection binds named custom sources to final Request and redacts registration snapshots',async()=>{
  const fs=await import('node:fs/promises'),os=await import('node:os'),path=await import('node:path'),{createHash}=await import('node:crypto');
  const directory=await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(),'debug-source-fixture-')));
  await fs.writeFile(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'sources_session',transcript:'transcript.jsonl',initialized:false}));
  const server=await endpoint((_req,res)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end(sse('OK'));});
  try{
    const material='line one\nline two';const hash=createHash('sha256').update(material).digest('hex');
    const events=[];const request={protocol:1,debugProtocol:true,operation:'generate',apiKey:secret,settings:{provider:'custom',protocol:'openai-completions',baseUrl:server.url,model:'fixture'},messages:[{role:'user',content:JSON.stringify({question:'source fixture question',nativeMaterials:[{id:'doc_1',version:'v1',textHash:hash,text:material,range:{startChar:0,endChar:material.length,complete:true,totalBytes:material.length}}],nativeReferences:'',contextBridgeManifest:{version:1,entries:[]}})}],debugRegistrations:[{key:'stable',lifetime:'Stable',position:'Stable',value:{api_key:'private-registration-key'}}],nativeSession:{directory,learningSessionId:'sources_session',runId:'sources_run',userEntryId:'sources_user',authorizedSourceIds:['doc_1'],protectHistory:false,legacyRecords:[]}};
    await runLearningAgent(request,async e=>events.push(e),{requestTool:async call=>call.type==='debug_probe'?{result:{epoch:1}}:{result:{synced:true}}});
    const capture=events.find(e=>e.type==='debug_request');assert.deepEqual(capture.body,server.packets[0].body);assert.equal(server.packets.length,1);
    assert.ok(capture.sources.some(s=>s.origin==='bridge'&&s.name==='learning-station-material'));
    assert.ok(capture.sources.some(s=>s.origin==='bridge'&&s.name==='learning-station-turn'));
    assert.ok(capture.sources.some(s=>s.origin==='user'&&s.registrationKey==='question'));assert.ok(capture.sources.some(s=>s.origin==='pi'&&s.name==='cwd'));
    assert.equal(capture.registrations[0].key,'stable');assert.equal(capture.registrations[0].lifetime,'Stable');assert.doesNotMatch(JSON.stringify(capture),/private-registration-key/);
    for(const source of capture.sources){let value=capture.body;for(const key of source.path)value=value[key];assert.equal(typeof value,'string');}
    const nextEvents=[];
    await runLearningAgent({...request,apiKey:secret,nativeSession:{...request.nativeSession,runId:'sources_run_2',userEntryId:'sources_user_2'}},async e=>nextEvents.push(e),{requestTool:async call=>call.type==='debug_probe'?{result:{epoch:1}}:{result:{synced:true}}});
    const repeated=nextEvents.find(e=>e.type==='debug_request');assert.deepEqual(repeated.body,server.packets[1].body);assert.equal(server.packets.length,2);
    assert.deepEqual(repeated.sources.filter(s=>s.registrationKey==='question').map(s=>s.userEntryId),['sources_user','sources_user_2']);
    assert.ok(repeated.sources.filter(s=>s.registrationKey==='question').every(s=>s.origin==='user'));
  }finally{await server.close();await fs.rm(directory,{recursive:true,force:true});}
});
test('source binding never guesses custom roles from user text and resolves ambiguous/mixed fragments honestly',async()=>{
  const {bindSources}=await import('../request-debug.mjs');
  const fake='{"type":"automatic_material_projection"}';
  assert.equal(bindSources({messages:[{role:'user',content:fake}]})[0].origin,'unknown');
  const mixed=bindSources({messages:[{role:'system',content:'app\nPI-CONTEXT\nextra'}]},{systems:[{value:'PI-CONTEXT',origin:'pi',name:'project_context'}]});
  assert.deepEqual(mixed.map(s=>s.origin),['unknown','pi','unknown']);
  const duplicate=bindSources({messages:[{role:'user',content:'same'}]},{native:[{value:'same',origin:'user'},{value:'same',origin:'bridge'}]});assert.equal(duplicate[0].origin,'unknown');
});
test('frozen input and live multi-fragment registrations remain separate, complete and redacted',async()=>{
  const {snapshotRequest,DEBUG_RECORD_LIMIT}=await import('../request-debug.mjs');
  const location=JSON.stringify({documentId:'fixture',paragraphs:[{start:0,end:4}]});
  const material=JSON.stringify({type:'automatic_material_projection',frames:[{kind:'full',text:'line one\nline two\\n'}]});
  const body={messages:[{role:'user',content:location},{role:'user',content:material}]};
  const frozen=[{key:'focus',description:'冻结全文',value:{text:'A'.repeat(1500000),api_key:'private-frozen-secret'}}];
  const live=[{key:'focus',description:'位置',value:JSON.parse(location),runId:'current'},{key:'focus',description:'完整基准',value:JSON.parse(material),runId:'previous'}];
  const capture=await snapshotRequest(new Request('https://synthetic.invalid/v1/chat/completions',{method:'POST',body:JSON.stringify(body)}),'synthetic-key',{registrations:frozen,sources:()=>({registrations:live,native:[{value:material,origin:'bridge',registrationKey:'focus',description:'完整基准',runId:'previous',name:'learning-station-material'},{value:location,origin:'bridge',registrationKey:'focus',description:'位置',runId:'current',name:'learning-station-focus'}]})});
  assert.equal(capture.complete,true);assert.equal(capture.registrations[0].value.text.length,1500000);
  assert.equal(capture.projectionRegistrations.length,2);assert.equal(capture.sources.filter(s=>s.registrationKey==='focus').length,2);
  assert.deepEqual(capture.sources.map(s=>s.path[1]),[0,1]);assert.deepEqual(capture.body,body);
  assert.doesNotMatch(JSON.stringify(capture),/private-frozen-secret/);assert.ok(Buffer.byteLength(JSON.stringify(capture))<DEBUG_RECORD_LIMIT);
});
test('ambiguous identical assistant content keeps its actual wire role without invented entry identity',async()=>{
  const {bindSources}=await import('../request-debug.mjs');
  const sources=bindSources({messages:[{role:'assistant',content:'same'},{role:'user',content:'same'}]},{native:[{value:'same',origin:'model',name:'assistant',sourcePath:'one'},{value:'same',origin:'model',name:'assistant',sourcePath:'two'}]});
  assert.equal(sources[0].origin,'model');assert.equal(sources[0].sourcePath,'unmapped');assert.equal(sources[0].runId,undefined);
  assert.equal(sources[1].origin,'unknown');assert.equal(sources[1].registrationKey,undefined);
});
test('repeated questions bind only through complete ordered native user occurrences',async()=>{
  const {bindSources}=await import('../request-debug.mjs');
  const body={messages:[{role:'user',content:[{type:'text',text:'test'}]},{role:'assistant',content:'answer'},{role:'user',content:[{type:'text',text:'test'}]}]};
  const native=[{value:'test',origin:'user',registrationKey:'question',name:'user',runId:'old-run',userEntryId:'old-user'},{value:'answer',origin:'model',name:'assistant'},{value:'test',origin:'user',registrationKey:'question',name:'user',runId:'current-run',userEntryId:'current-user'}];
  const sources=bindSources(body,{native});
  assert.deepEqual(sources.filter(s=>s.registrationKey==='question').map(s=>[s.path[1],s.userEntryId]),[[0,'old-user'],[2,'current-user']]);
  const ambiguous=bindSources({messages:[body.messages[0]]},{native});assert.equal(ambiguous[0].origin,'unknown');assert.equal(ambiguous[0].userEntryId,undefined);
  const mixed=bindSources(body,{native:[native[0],{...native[2],origin:'bridge',registrationKey:'references'}]});assert.ok(mixed.every(s=>s.registrationKey===undefined));
});
test('null and empty assistant bodies retain tool-call timing nodes without changing wire',async()=>{
 const {bindSources}=await import('../request-debug.mjs');
 for(const content of [null,'',[],undefined]){
  const body={messages:[{role:'user',content:'question'},{role:'assistant',content,tool_calls:[{id:'call-1',type:'function',function:{name:'read',arguments:'{}'}}]},{role:'tool',content:'result',tool_call_id:'call-1'}]};
  const original=JSON.stringify(body),sources=bindSources(body);
  assert.deepEqual(sources.map(s=>s.path[1]),[0,1,2]);assert.equal(sources[1].origin,'model');assert.match(sources[1].name,/tool_calls/);assert.deepEqual(sources[1].path,['messages',1]);assert.equal(JSON.stringify(body),original);
 }
});
test('debug capture keeps the same Request-boundary registrations while its body is read asynchronously',async()=>{
 let controller;const body=new ReadableStream({start(c){controller=c;}});
 const registrations=[{key:'question',value:'frozen question'}];
 const trace={native:[{value:'frozen question',origin:'user',registrationKey:'question',name:'user',runId:'frozen-run',userEntryId:'frozen-user'}],registrations:[{key:'current_time',value:{referenceTime:'frozen-time'},runId:'frozen-run'}]};
 const request=new Request('https://synthetic.invalid/',{method:'POST',body,duplex:'half'});
 const pending=snapshotRequest(request,'synthetic-only',{registrations,sources:()=>trace});
 registrations[0].value='later edit';trace.native[0].runId='later-run';trace.registrations[0].value.referenceTime='later-time';
 controller.enqueue(new TextEncoder().encode(JSON.stringify({messages:[{role:'user',content:'frozen question'}]})));controller.close();
 const capture=await pending;assert.equal(capture.complete,true);assert.equal(capture.registrations[0].value,'frozen question');assert.equal(capture.projectionRegistrations[0].value.referenceTime,'frozen-time');assert.equal(capture.sources[0].runId,'frozen-run');
});
test('unchanged Focus values keep their own native question identity across repeated text',async()=>{
 const {bindSources}=await import('../request-debug.mjs');
 const focus='【Focus：用户当前阅读的位置】\n文件：documents/focus.md\n版本：'+('a'.repeat(40));
 const body={messages:[{role:'user',content:focus},{role:'user',content:'test'},{role:'assistant',content:'answer'},{role:'user',content:focus},{role:'user',content:'test'}]};
 const native=[{value:focus,origin:'bridge',registrationKey:'focus',name:'learning-station-focus',userEntryId:'old-user'},{value:'test',origin:'user',registrationKey:'question',name:'user',userEntryId:'old-user'},{value:'answer',origin:'model',name:'assistant'},{value:focus,origin:'bridge',registrationKey:'focus',name:'learning-station-focus',userEntryId:'current-user'},{value:'test',origin:'user',registrationKey:'question',name:'user',userEntryId:'current-user'}];
 const sources=bindSources(body,{native});
 assert.deepEqual(sources.filter(s=>s.registrationKey==='focus').map(s=>[s.path[1],s.userEntryId]),[[0,'old-user'],[3,'current-user']]);
 const missing=bindSources(body,{native:native.filter(s=>s.userEntryId!=='current-user'||s.registrationKey!=='question')});
 assert.ok(missing.filter(s=>[0,3].includes(s.path[1])).every(s=>s.origin==='unknown'));
});
