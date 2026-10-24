import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {JSDOM} from 'jsdom';
import {unlink} from 'node:fs/promises';

test('React tool cards distinguish running, persisted success, handler error and unconfirmed history without inferred saves',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/'});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
 const filename=new URL(`./.tool-cards-${process.pid}.mjs`,import.meta.url);await build({entryPoints:['src/ToolActivityCards.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic'});
 const React=await import('react');const {createRoot}=await import('react-dom/client');const {ToolActivityCards}=await import(filename.href);const root=createRoot(document.getElementById('root'));const opened=[];
 const base={id:'tool_safe',toolCallId:'call_safe',runId:'run_safe',sessionId:'session_safe',name:'create_todo',title:'创建待办',summary:'已收到本机保存回执。',startedAt:'2026-10-16T00:00:00Z',updatedAt:'2026-10-16T00:00:01Z',status:'running',persisted:false};
 const render=async(item,liveRunId)=>React.act(async()=>root.render(React.createElement(ToolActivityCards,{items:[item],liveRunId,onOpen:value=>opened.push(value)})));
 try{
  await render(base,'run_safe');assert.ok(document.querySelector('[aria-label="工具调用"] [aria-label="创建待办 · 运行中"]'));assert.match(document.body.textContent,/正在调用本机工具/);assert.equal(document.querySelector('.tool-activity button'),null);
  const object={kind:'todo',id:'todo_safe',sessionId:'session_safe',sectionId:'section_safe'};await render({...base,status:'succeeded',persisted:true,object});assert.ok(document.querySelector('[aria-label="创建待办 · 成功"]'));await React.act(async()=>document.querySelector('.tool-activity button').click());assert.deepEqual(opened,[object]);assert.equal(document.querySelectorAll('.tool-activity').length,1);
  await render({...base,status:'failed',errorCode:'CONFLICT'});assert.ok(document.querySelector('[aria-label="创建待办 · 失败"]'));assert.match(document.body.textContent,/保存版本发生变化/);assert.equal(document.querySelector('.tool-activity button'),null);
  await render({...base,name:'write',title:'受控写入',status:'waiting',summary:'AI 修改待确认，建议已保留，正文未修改。'},'run_safe');assert.ok(document.querySelector('[aria-label="受控写入 · 待审阅"]'));assert.equal(document.querySelector('.spin'),null);assert.match(document.body.textContent,/正文未修改/);
  await render({...base,status:'unknown'},'run_safe');assert.ok(document.querySelector('[aria-label="创建待办 · 结果未确认"]'));assert.equal(document.querySelector('.spin'),null);
  await render(base);assert.ok(document.querySelector('[aria-label="创建待办 · 结果未确认"]'));assert.match(document.body.textContent,/不会自动重跑/);assert.equal(opened.length,1);
  const exerciseObject={kind:'exercise',id:'exercise_safe',sessionId:'session_safe',sectionId:'section_safe'};await render({...base,name:'generate_exercises',title:'生成练习',status:'succeeded',persisted:true,object:exerciseObject});assert.equal(document.querySelector('.tool-activity button').textContent,'打开练习');await React.act(async()=>document.querySelector('.tool-activity button').click());assert.deepEqual(opened.at(-1),exerciseObject);
 }finally{await React.act(async()=>root.unmount());dom.window.close();await unlink(filename).catch(()=>{});}
});

test('authorized workspace tool cards render fixed titles without faking unconfirmed saves',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/'});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
 const filename=new URL(`./.tool-cards-auth-${process.pid}.mjs`,import.meta.url);await build({entryPoints:['src/ToolActivityCards.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic'});
 const React=await import('react');const {createRoot}=await import('react-dom/client');const {ToolActivityCards}=await import(filename.href);const {readToolActivity,toolState}=await import(await build({entryPoints:['src/toolActivities.ts'],outfile:new URL(`./.tool-activities-auth-${process.pid}.mjs`,import.meta.url).pathname,bundle:true,packages:'external',platform:'node',format:'esm'}).then(async()=>new URL(`./.tool-activities-auth-${process.pid}.mjs`,import.meta.url).href));
 const root=createRoot(document.getElementById('root'));const opened=[];
 const base={id:'tool_read_1',toolCallId:'call_read_1',runId:'run_auth',sessionId:'session_auth',startedAt:'2026-10-17T00:00:00Z',updatedAt:'2026-10-17T00:00:01Z'};
 const read=readToolActivity({...base,name:'read',status:'succeeded',persisted:false});
 assert.equal(read.title,'受控读取');
 assert.equal(read.summary,'受控读取已完成，未写入内容。');
 assert.equal(toolState(read,'run_auth'),'succeeded');
 const written=readToolActivity({...base,id:'tool_write_1',toolCallId:'call_write_1',name:'write',status:'succeeded',persisted:true});
 assert.equal(written.title,'受控写入');
 assert.equal(written.summary,'已收到本机保存回执。');
 assert.equal(toolState(written,'run_auth'),'succeeded');
 const unfinished=readToolActivity({...base,id:'tool_write_2',toolCallId:'call_write_2',name:'write',status:'succeeded',persisted:false});
 assert.equal(toolState(unfinished,'run_auth'),'unconfirmed');
 const unknown=readToolActivity({...base,name:'shell'});
 assert.equal(unknown,null,'unregistered tool names are dropped');
 try{
  await React.act(async()=>root.render(React.createElement(ToolActivityCards,{items:[read,written],liveRunId:'run_auth',onOpen:value=>opened.push(value)})));
  assert.ok(document.querySelector('[aria-label="工具调用"] [aria-label="受控读取 · 成功"]'));
  assert.ok(document.querySelector('[aria-label="受控写入 · 成功"]'));
  assert.match(document.body.textContent,/受控读取已完成/);
  assert.equal(document.querySelectorAll('.tool-activity button').length,0,'workspace tools open no library object');
 }finally{await React.act(async()=>root.unmount());dom.window.close();await unlink(filename).catch(()=>{});await unlink(new URL(`./.tool-activities-auth-${process.pid}.mjs`,import.meta.url).pathname).catch(()=>{});}
});

test('failed exercise cards pass only whitelisted field diagnostics; raw values, URLs, overlong text and scope failures fall back',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/'});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});
 const cardsUrl=new URL(`./.tool-cards-diag-${process.pid}.mjs`,import.meta.url);await build({entryPoints:['src/ToolActivityCards.tsx'],outfile:cardsUrl.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic'});
 const activitiesUrl=new URL(`./.tool-activities-diag-${process.pid}.mjs`,import.meta.url);await build({entryPoints:['src/toolActivities.ts'],outfile:activitiesUrl.pathname,bundle:true,packages:'external',platform:'node',format:'esm'});
 const React=await import('react');const {createRoot}=await import('react-dom/client');const {ToolActivityCards}=await import(cardsUrl.href);const {readToolActivity}=await import(activitiesUrl.href);const root=createRoot(document.getElementById('root'));
 const base={id:'tool_gen_1',toolCallId:'call_gen_1',runId:'run_gen',sessionId:'session_gen',name:'generate_exercises',startedAt:'2026-10-17T00:00:00Z',updatedAt:'2026-10-17T00:00:01Z',status:'failed',persisted:false,errorCode:'INVALID_MODEL_OUTPUT'};
 const generic='工具未完成，未确认保存成功。';
 // Exact backend vocabulary (exercises.rs field_error, generation.rs practice) passes through unchanged.
 const kept=[
  '练习校验失败：exercises[1].question 不能为空或只有空白；收到的内容已保留。',
  '练习校验失败：exercises[5].answers[10] 共262144个字符，最多10000个字符；收到的内容已保留。',
  '练习校验失败：schema 必须是对象且不能包含未声明字段；收到的内容已保留。',
  '练习校验失败：exercises[2].sourceIds 含本轮授权来源清单以外的编号；收到的内容已保留。',
  '练习校验失败：exercises[3].format 必须是exact或self；收到的内容已保留。',
  '练习校验失败：exercises[100] 必须是对象且不能包含未声明字段；收到的内容已保留。',
  '模型没有返回严格练习 JSON；请仅输出exercises与summary组成的JSON对象，不带代码围栏或说明；收到的内容已保留。'
 ];
 kept.forEach((summary,index)=>{const card=readToolActivity({...base,id:`tool_ok_${index}`,toolCallId:`call_ok_${index}`,summary});assert.ok(card);assert.equal(card.summary,summary);assert.equal(card.errorCode,'INVALID_MODEL_OUTPUT');});
 // Suspected raw values, URLs, unknown paths, markup and overlong text fall back to the fixed copy.
 const blocked=[
  '练习校验失败：exercises[1].question PRIVATE_RAW_VALUE；收到的内容已保留。',
  '练习校验失败：exercises[1].question https://evil.example/payload；收到的内容已保留。',
  '练习校验失败：user.name 必须是文本；收到的内容已保留。',
  `练习校验失败：exercises[1].question ${'超'.repeat(120)}；收到的内容已保留。`,
  '练习校验失败：exercises[1].question 不能为空或只有空白；收到的内容已保留。<script>alert(1)</script>',
  '练习校验失败：exercises[1000] 必须是对象且不能包含未声明字段；收到的内容已保留。',
  '练习校验失败：exercises[1].answers[100] 必须是文本；收到的内容已保留。',
  ' 练习校验失败：exercises[1].question 不能为空或只有空白；收到的内容已保留。',
  '练习校验失败：exercises[1].question 不能包含空字符\u0000；收到的内容已保留。',
  '练习校验失败：exercises[１] 必须是文本；收到的内容已保留。',
  '工具未确认完成；不会将模型文字当作保存成功。'
 ];
 blocked.forEach((summary,index)=>{const card=readToolActivity({...base,id:`tool_bad_${index}`,toolCallId:`call_bad_${index}`,summary});assert.ok(card);assert.equal(card.summary,generic);});
 // Authorization and other failures keep the generic copy even with diagnostic-shaped summary text.
 const scoped=readToolActivity({...base,id:'tool_scope',toolCallId:'call_scope',errorCode:'TOOL_SCOPE',summary:kept[0]});
 assert.equal(scoped.summary,generic);
 assert.equal(readToolActivity({...base,id:'tool_conflict',toolCallId:'call_conflict',errorCode:'CONFLICT'}).summary,generic);
 try{
  const allowed=readToolActivity({...base,summary:kept[0]});const blockedCard=readToolActivity({...base,id:'tool_bad_0',toolCallId:'call_bad_0',summary:blocked[0]});
  await React.act(async()=>root.render(React.createElement(ToolActivityCards,{items:[allowed,blockedCard],liveRunId:'run_gen',onOpen:()=>{}})));
  assert.match(document.body.textContent,/exercises\[1\]\.question 不能为空或只有空白/);
  assert.ok(!document.body.textContent.includes('PRIVATE_RAW_VALUE'),'raw model values never render');
  assert.equal(document.querySelectorAll('script').length,0,'whitelisted summary renders as plain text');
 }finally{await React.act(async()=>root.unmount());dom.window.close();await unlink(cardsUrl.pathname).catch(()=>{});await unlink(activitiesUrl.pathname).catch(()=>{});}
});
