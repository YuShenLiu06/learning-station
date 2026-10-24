import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {JSDOM} from 'jsdom';
import {unlink} from 'node:fs/promises';
test('raw course tree keeps nested files, filters with ancestors and reveals only selected course path',async()=>{
 const dom=new JSDOM('<div id="root"></div>',{url:'http://localhost/'});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true,HTMLElement:dom.window.HTMLElement,Event:dom.window.Event,MouseEvent:dom.window.MouseEvent});
 const {mockIPC,clearMocks}=await import('@tauri-apps/api/mocks'); const calls=[]; let external=false;
 mockIPC(async(command,args)=>{calls.push({command,args});if(command==='list_course_files')return [{id:'dir',name:'demo',path:'demo',directory:true},{id:'raw',name:'example.html',path:'demo/example.html',parentId:'dir',directory:false},{id:'documents-dir',name:'documents',path:'documents',directory:true},{id:'note',name:'note.md',path:'documents/note.md',parentId:'documents-dir',directory:false,documentId:'note-doc'},...(external?[{id:'new',name:'new.md',path:'documents/new.md',parentId:'documents-dir',directory:false,documentId:'new-doc'}]:[])];if(command==='reveal_course_file')return null;throw Error(command);});
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
  const added={...note,id:'new-doc',title:'新增笔记',path:'new.md'};let opened;let navigate;let noteTitle=note.title;const noop=()=>{};
  function SidebarFixture(){const[nodes,setNodes]=React.useState([course,note]);const[active,setActive]=React.useState('note-doc');const[expanded,setExpanded]=React.useState([]);navigate=setActive;return React.createElement(WorkspaceSidebar,{nodes,activeId:active,onRefreshNodes:async()=>{setNodes([course,{...note,title:noteTitle},...(external?[added]:[])]);},onToggleCollapsed:noop,sidebarWidth:480,onSidebarWidth:noop,collapsed:false,expandedIds:expanded,onExpanded:setExpanded,onOpen:id=>{opened=id;setActive(id);},onNew:noop,onImport:noop,onLearning:noop,onExplorations:noop,onHistory:noop,onReview:noop,onTasks:noop,onTrash:noop,onTrashNode:noop,onSettings:noop,onInject:noop,injectedIds:[],onRemoveInjection:noop,canInject:false,view:'lesson',loading:false,dirtyIds:[],taskCount:0,modelReady:false,blocked:false});}
  await React.act(async()=>root.render(React.createElement(SidebarFixture)));
  assert.equal(document.querySelector('[title="documents"]').getAttribute('aria-expanded'),'true','opening current document reveals its physical parent');
  const search=document.querySelector('[aria-label="查找学习文件"]');
  async function filter(value){await React.act(async()=>{Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype,'value').set.call(search,value);search.dispatchEvent(new Event('input',{bubbles:true}));});}
  await filter('note.md');assert.ok(document.querySelector('[title="课堂笔记"]'),'physical filename match must retain differently titled business row');
  await React.act(async()=>document.querySelector('[title="课堂笔记"]').click());assert.equal(opened,'note-doc');
  await filter('');external=true;await React.act(async()=>document.querySelector('[aria-label="刷新课程文件"]').click());
  assert.ok(document.querySelector('[title="新增笔记"]'),'refresh publishes newly discovered business nodes');
  await React.act(async()=>document.querySelector('[title="新增笔记"]').click());assert.equal(opened,'new-doc');
  const fileTree=document.querySelector('[role="tree"][aria-label="课程中的实际文件"]');
  const key=async(key)=>React.act(async()=>document.activeElement.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key,bubbles:true,cancelable:true})));
  const noteRow=()=>document.querySelector(`[title="${noteTitle}"]`);const addedRow=()=>document.querySelector('[title="新增笔记"]');
  await React.act(async()=>noteRow().focus());await key('ArrowDown');assert.equal(document.activeElement,addedRow(),'ArrowDown crosses registered-document component boundaries');
  await key('ArrowUp');assert.equal(document.activeElement,noteRow());await key('End');assert.equal(document.activeElement,addedRow());await key('Home');assert.equal(document.activeElement,fileTree.querySelector('.tree-row'));
  assert.equal(fileTree.querySelectorAll('.tree-row[tabindex="0"]').length,1,'the combined file tree has one tab stop');
  await React.act(async()=>document.querySelector('[title="documents"]').click());assert.equal(noteRow(),null,'user can collapse the active physical directory');
  await React.act(async()=>navigate('note-doc'));assert.ok(noteRow(),'source navigation into another document reveals the physical parent');
  await React.act(async()=>document.querySelector('[title="documents"]').click());await React.act(async()=>navigate('course-a'));await React.act(async()=>navigate('note-doc'));assert.ok(noteRow(),'returning from a logical-only document also reveals the physical parent');
  external=false;await React.act(async()=>document.querySelector('[aria-label="刷新课程文件"]').click());assert.equal(addedRow(),null,'removed external document cannot survive via renderRemaining');
  noteTitle='外部改名';await React.act(async()=>window.dispatchEvent(new Event('focus')));assert.ok(noteRow(),'window focus refreshes changed document metadata');assert.equal(document.querySelector('[title="课堂笔记"]'),null);
  const settled=calls.filter(c=>c.command==='list_course_files').length;await React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,20));});assert.equal(calls.filter(c=>c.command==='list_course_files').length,settled,'node reconciliation must settle without an effect loop');


 }finally{await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(file);}
});
