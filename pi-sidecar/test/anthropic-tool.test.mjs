import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { runLearningAgent } from '../harness.mjs';
const KEY='local-anthropic-tool-fixture-key';
const encode=events=>events.map(event=>`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join('');
const start=()=>({type:'message_start',message:{id:'fixture',type:'message',role:'assistant',model:'glm-5.3-flash',content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:1,output_tokens:0}}});
const end=reason=>[{type:'message_delta',delta:{stop_reason:reason,stop_sequence:null},usage:{output_tokens:3}},{type:'message_stop'}];
test('Anthropic real SDK round trip uses the registered tool and consumes a synthetic Rust-style receipt',async()=>{
 let requests=0,callbacks=0;
 const server=createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;const body=JSON.parse(raw);requests++;assert.equal(req.url,'/v1/messages?beta=true');assert.deepEqual(body.tools.map(tool=>tool.name),['create_todo']);assert.equal(req.headers['x-api-key'],KEY);res.writeHead(200,{'content-type':'text/event-stream'});
  if(requests===1)res.end(encode([start(),{type:'content_block_start',index:0,content_block:{type:'tool_use',id:'tool_fixture',name:'create_todo',input:{}}},{type:'content_block_delta',index:0,delta:{type:'input_json_delta',partial_json:'{"title":"fixture todo"}'}},{type:'content_block_stop',index:0},...end('tool_use')]));
  else{assert.equal(requests,2);const result=body.messages.flatMap(message=>Array.isArray(message.content)?message.content:[]).find(block=>block.type==='tool_result');assert(result);assert.match(JSON.stringify(result),/todo_fixture/);assert.match(JSON.stringify(result),/persisted/);res.end(encode([start(),{type:'content_block_start',index:0,content_block:{type:'text',text:''}},{type:'content_block_delta',index:0,delta:{type:'text_delta',text:'Fixture receipt consumed.'}},{type:'content_block_stop',index:0},...end('end_turn')]));}
 });await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
 try{const request={protocol:1,operation:'generate',settings:{provider:'custom',protocol:'anthropic-messages',baseUrl:`http://127.0.0.1:${server.address().port}`,model:'glm-5.3-flash'},apiKey:KEY,messages:[{role:'user',content:'Create a fixture todo.'}],learningTools:true,toolPolicy:{mode:'question',allowedTools:['create_todo'],maxCalls:1}};const events=[];const result=await runLearningAgent(request,event=>events.push(event),{requestTool:async(call)=>{callbacks++;assert.equal(call.name,'create_todo');return{result:{persisted:true,todoId:'todo_fixture',sessionId:'session_fixture',todo:{reminderEnabled:false}}};}});assert.equal(result.output,'Fixture receipt consumed.');assert.equal(events.at(-1).toolCalls,1);assert.equal(callbacks,1);assert.equal(requests,2);assert(!JSON.stringify(events).includes(KEY));}
 finally{server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
});
