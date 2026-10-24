import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {JSDOM} from 'jsdom';
import {unlink} from 'node:fs/promises';
test('raw course tree keeps nested files, filters with ancestors and reveals only selected course path',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/'});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true,HTMLElement:dom.window.HTMLElement,Event:dom.window.Event,MouseEvent:dom.window.MouseEvent});
 const {mockIPC,clearMocks}=await import('@tauri-apps/api/mocks'); const calls=[];
 mockIPC(async(command,args)=>{calls.push({command,args});if(command==='list_course_files')return [{id:'dir',name:'demo',path:'demo',directory:true},{id:'raw',name:'example.html',path:'demo/example.html',parentId:'dir',directory:false},{id:'note',name:'note.md',path:'note.md',directory:false,documentId:'note-doc'}];if(command==='reveal_course_file')return null;throw Error(command);});
 const file=new URL(`./.course-sidebar-${process.pid}.mjs`,import.meta.url);
 await build({entryPoints:['src/CourseFiles.tsx'],outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
 const React=await import('react'),{createRoot}=await import('react-dom/client'),{CourseFiles}=await import(file.href);const root=createRoot(document.getElementById('root'));let expanded=[];let fallback;
 const render=(filter='')=>root.render(React.createElement(CourseFiles,{courseId:'course-a',nodes:[],activeId:'',expandedIds:expanded,onExpanded:ids=>{expanded=ids;render();},onOpen:()=>assert.fail('raw file must not open as business document'),filter,renderDocument:id=>React.createElement('span',null,'Registered '+id),renderRemaining:ids=>{fallback=ids;return null;}}));
 try{
  await React.act(async()=>{render();});
  assert.ok(document.body.textContent.includes('demo'));assert.ok(!document.body.textContent.includes('example.html'));assert.deepEqual(fallback,['note-doc']);
  await React.act(async()=>render('example'));assert.ok(document.body.textContent.includes('example.html'));assert.ok(document.body.textContent.includes('demo'));
  const raw=document.querySelector('[title="demo/example.html"]');await React.act(async()=>raw.click());
  assert.match(document.querySelector('[role="dialog"]').textContent,/暂不支持/);assert.equal(document.querySelector('iframe'),null);
  await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='在 Finder 中显示').click());
  assert.deepEqual(calls.find(c=>c.command==='reveal_course_file').args,{courseId:'course-a',path:'demo/example.html'});
  await React.act(async()=>document.querySelector('[aria-label="关闭文件信息"]').click());assert.equal(document.querySelector('[role="dialog"]'),null);
 }finally{await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(file);}
});
