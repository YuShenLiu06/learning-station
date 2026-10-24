import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {JSDOM} from 'jsdom';
import {unlink} from 'node:fs/promises';
test('raw course tree keeps nested files, filters with ancestors and reveals only selected course path',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/'});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true,HTMLElement:dom.window.HTMLElement,Event:dom.window.Event,MouseEvent:dom.window.MouseEvent});
 const {mockIPC,clearMocks}=await import('@tauri-apps/api/mocks'); const calls=[]; let external=false;
 mockIPC(async(command,args)=>{calls.push({command,args});if(command==='list_course_files')return [{id:'dir',name:'demo',path:'demo',directory:true},{id:'raw',name:'example.html',path:'demo/example.html',parentId:'dir',directory:false},{id:'note',name:'note.md',path:'note.md',directory:false,documentId:'note-doc'},...(external?[{id:'new',name:'new.md',path:'new.md',directory:false,documentId:'new-doc'}]:[])];if(command==='reveal_course_file')return null;throw Error(command);});
 const file=new URL(`./.course-sidebar-${process.pid}.mjs`,import.meta.url);
 await build({stdin:{contents:"export {CourseFiles} from './src/CourseFiles';export {WorkspaceSidebar} from './src/WorkspaceSidebar';",resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
 const React=await import('react'),{createRoot}=await import('react-dom/client'),{CourseFiles,WorkspaceSidebar}=await import(file.href);const root=createRoot(document.getElementById('root'));let expanded=[];let fallback;
 const render=(filter='')=>root.render(React.createElement(CourseFiles,{courseId:'course-a',nodes:[],activeId:'',expandedIds:expanded,onExpanded:ids=>{expanded=ids;render();},onOpen:id=>assert.equal(id,'note-doc'),filter,renderDocument:id=>React.createElement('span',null,'Registered '+id),renderRemaining:ids=>{fallback=ids;return null;}}));
 try{
  await React.act(async()=>{render();});
  assert.ok(document.body.textContent.includes('demo'));assert.ok(!document.body.textContent.includes('example.html'));assert.deepEqual(fallback,['note-doc']);
  await React.act(async()=>render('example'));assert.ok(document.body.textContent.includes('example.html'));assert.ok(document.body.textContent.includes('demo'));
  const raw=document.querySelector('[title="demo/example.html"]');await React.act(async()=>raw.click());
  assert.match(document.querySelector('[role="dialog"]').textContent,/暂不支持/);assert.equal(document.querySelector('iframe'),null);
  await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='在 Finder 中显示').click());
  assert.deepEqual(calls.find(c=>c.command==='reveal_course_file').args,{courseId:'course-a',path:'demo/example.html'});
  await React.act(async()=>document.querySelector('[aria-label="关闭文件信息"]').click());assert.equal(document.querySelector('[role="dialog"]'),null);
  const course={id:'course-a',title:'课程',kind:'course',path:'',version:'v1'};
  const note={id:'note-doc',title:'课堂笔记',kind:'section',parentId:'course-a',path:'note.md',version:'v1'};
  const added={...note,id:'new-doc',title:'新增笔记',path:'new.md'};let opened;const noop=()=>{};
  function SidebarFixture(){const[nodes,setNodes]=React.useState([course,note]);return React.createElement(WorkspaceSidebar,{nodes,activeId:'note-doc',onRefreshNodes:async()=>{setNodes([course,note,added]);},onToggleCollapsed:noop,sidebarWidth:480,onSidebarWidth:noop,collapsed:false,expandedIds:[],onExpanded:noop,onOpen:id=>{opened=id;},onNew:noop,onImport:noop,onLearning:noop,onExplorations:noop,onHistory:noop,onReview:noop,onTasks:noop,onTrash:noop,onTrashNode:noop,onSettings:noop,onInject:noop,injectedIds:[],onRemoveInjection:noop,canInject:false,view:'lesson',loading:false,dirtyIds:[],taskCount:0,modelReady:false,blocked:false});}
  await React.act(async()=>root.render(React.createElement(SidebarFixture)));
  const search=document.querySelector('[aria-label="查找学习文件"]');
  async function filter(value){await React.act(async()=>{Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype,'value').set.call(search,value);search.dispatchEvent(new Event('input',{bubbles:true}));});}
  await filter('note.md');assert.ok(document.querySelector('[title="课堂笔记"]'),'physical filename match must retain differently titled business row');
  await React.act(async()=>document.querySelector('[title="课堂笔记"]').click());assert.equal(opened,'note-doc');
  await filter('');external=true;await React.act(async()=>document.querySelector('[aria-label="刷新课程文件"]').click());
  assert.ok(document.querySelector('[title="新增笔记"]'),'refresh publishes newly discovered business nodes');
  await React.act(async()=>document.querySelector('[title="新增笔记"]').click());assert.equal(opened,'new-doc');

 }finally{await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(file);}
});
