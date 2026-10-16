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
  await render(base);assert.ok(document.querySelector('[aria-label="创建待办 · 结果未确认"]'));assert.match(document.body.textContent,/不会自动重跑/);assert.equal(opened.length,1);
  const exerciseObject={kind:'exercise',id:'exercise_safe',sessionId:'session_safe',sectionId:'section_safe'};await render({...base,name:'generate_exercises',title:'生成练习',status:'succeeded',persisted:true,object:exerciseObject});assert.equal(document.querySelector('.tool-activity button').textContent,'打开练习');await React.act(async()=>document.querySelector('.tool-activity button').click());assert.deepEqual(opened.at(-1),exerciseObject);
 }finally{await React.act(async()=>root.unmount());dom.window.close();await unlink(filename).catch(()=>{});}
});
