import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { unlink } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { JSDOM } from 'jsdom';
const dom = new JSDOM('<div id="root"></div>', {url:'http://127.0.0.1/',pretendToBeVisual:true});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true,requestAnimationFrame:dom.window.requestAnimationFrame.bind(dom.window),cancelAnimationFrame:dom.window.cancelAnimationFrame.bind(dom.window)});
Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
for(const name of ['Window','Range','HTMLElement','HTMLInputElement','HTMLSelectElement','HTMLTextAreaElement','Element','Node','Event','MouseEvent','KeyboardEvent','MutationObserver','DOMRect'])globalThis[name]=dom.window[name];
globalThis.getComputedStyle=dom.window.getComputedStyle.bind(dom.window);globalThis.ResizeObserver=class{observe(){}unobserve(){}disconnect(){}};Range.prototype.getClientRects=()=>[];Range.prototype.getBoundingClientRect=()=>new DOMRect();
const file=new URL(`./.focus-${process.pid}.mjs`,import.meta.url);
await build({stdin:{contents:`export {App} from './src/App';export {captureFocus,readFocus} from './src/focus';export {sessionMessages} from './src/sessionMessages';`,resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
const React=await import('react'),{createRoot}=await import('react-dom/client'),ui=await import(file.href),{mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');
const hash=text=>createHash('sha256').update(text).digest('hex'),clone=v=>structuredClone(v);
const focus=(id='note_a')=>({id:hash(id),courseId:'course_c',documentId:id,sourceDocumentId:id,path:`documents/${id}.md`,type:'markdown',commitHash:'1'.repeat(40),materialTreeOid:'2'.repeat(40),excludedFiles:[],documentVersion:'v1',view:'read',paragraphs:[{start:0,end:4,text:'原文😀'}]});
const tick=async(ms=0)=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});
const wait=async check=>{for(let i=0;i<150;i++){if(check())return;await tick(10);}assert.ok(check(),'UI timeout: '+document.body.textContent);};
const click=async el=>{assert.ok(el);await React.act(async()=>el.dispatchEvent(new MouseEvent('click',{bubbles:true})));};
const button=text=>[...document.querySelectorAll('button')].find(el=>el.textContent.includes(text));

test('view adapters capture visible raw paragraphs, active question and metadata without sharing mutable state',()=>{
 const root=document.createElement('div');document.body.append(root);root.innerHTML='<div data-focus-start="0" data-focus-end="4"></div><div data-focus-start="6" data-focus-end="8"></div><article data-focus-question="q1"></article><article data-focus-question="q2"><textarea></textarea></article>';
 root.getBoundingClientRect=()=>({top:0,bottom:200,height:200});
 const blocks=root.querySelectorAll('[data-focus-start]');blocks[0].getBoundingClientRect=()=>({top:20,bottom:70,height:50});blocks[1].getBoundingClientRect=()=>({top:250,bottom:300,height:50});
 const captured=ui.captureFocus('note_a','原文😀\n\n尾段','v1','read',root);assert.deepEqual(captured.ranges,[{start:0,end:4}]);assert.equal(captured.documentId,'note_a');
 root.querySelector('textarea').focus();assert.equal(ui.captureFocus('exercise','body','v1','exercise',root).questionId,'q2');
 assert.deepEqual(ui.captureFocus('note_a','body','v1','asset',root,undefined,'image_asset').ranges,[]);
 const parsed=ui.readFocus(focus());const original=focus();assert.deepEqual(parsed,original);original.paragraphs[0].text='later';assert.equal(parsed.paragraphs[0].text,'原文😀');
 for(const bad of [{...focus(),commitHash:'HEAD'},{...focus(),paragraphs:[{start:4,end:0,text:'x'}]},{...focus(),asset:{id:'image',mime:'image/png',size:-1}},{...focus(),exercise:{questionId:'q',number:0,answered:false,graded:false}}])assert.equal(ui.readFocus(bad),undefined);
 root.remove();
});
test('persisted replies restore their own Focus, including image/PDF metadata; old records never borrow current focus',()=>{
 const a=focus(),b={...focus('note_b'),type:'pdf',view:'asset',paragraphs:[],asset:{id:'pdf',mime:'application/pdf',size:20}};
 const mapped=ui.sessionMessages([{id:'a',kind:'assistant',content:'A',data:{focus:a,contextManifest:[]}},{id:'b',kind:'assistant',content:'B',data:{focus:b,contextManifest:[]}},{id:'old',kind:'assistant',content:'Old',data:{contextManifest:[]}}]);
 assert.deepEqual(mapped[0].context.focus,a);assert.deepEqual(mapped[1].context.focus,b);assert.equal(mapped[2].context.focus,undefined);assert.equal('text' in mapped[1].context.focus.asset,false);
});
test('actual React/native path keeps the original Focus through page switch, cancellation, retry and reopening',async()=>{
 const docs=new Map(['note_a','note_b'].map((id,index)=>[id,{id,path:`courses/${id}.md`,metadata:{id,kind:'section',title:index?'B笔记':'A笔记',parentId:'course_c',tags:[],goals:[],links:[]},body:'原文😀\n\n尾段',version:'v1'}]));
 const session={id:'course_session',courseId:'course_c',sectionId:'note_a',version:'s1',entries:[],contextManifest:[]};
 const calls=[],runs=new Map();let active='note_a',freezeCount=0,runReads=0,rejectFreeze=true;
 mockWindows('focus-isolated-fixture');mockIPC(async(command,args={})=>{calls.push({command,args:clone(args)});switch(command){
  case 'get_app_state':return {root:'/tmp/focus-fixture',activeDocumentId:active,documents:[...docs.values()].map(d=>({id:d.id,title:d.metadata.title,kind:'section',path:d.path,version:d.version})),model:{configured:true,provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1/mock',model:'fixture',usageGate:''}};
  case 'get_library_tree':return {nodes:[{id:'course_c',title:'C课程',kind:'course',path:'courses/course_c.md',version:'v1'},...[...docs.values()].map(d=>({id:d.id,title:d.metadata.title,kind:'section',parentId:'course_c',courseId:'course_c',path:d.path,version:d.version}))],warnings:[]};
  case 'get_ui_preferences':return {fontSize:16,assistantWidth:346,sidebarWidth:240,sidebarCollapsed:false,expandedNodeIds:['course_c'],toolAuthorizationTier:'off'};
  case 'save_ui_preferences':return args;case 'get_ui_draft':return null;case 'save_ui_draft':return {key:args.key,data:args.data,version:'draft',updatedAt:'fixture'};
  case 'record_ui_metric':return null;case 'list_todos':case 'list_notifications':return [];
  case 'open_document':active=args.id;return clone(docs.get(args.id));case 'start_session':return {session:clone(session)};case 'get_session':return clone(session);
  case 'load_session_context':return {text:'',manifest:[],warnings:[],truncated:false};
  case 'freeze_focus':{freezeCount++;if(rejectFreeze){rejectFreeze=false;throw {code:'CONTEXT_CONFLICT',message:'Synthetic preflight rejection'};}assert.equal(args.request.input.documentId,'note_a');assert.equal(args.request.input.view,'read');return focus();}
  case 'append_entry':if(!session.entries.some(e=>e.data?.clientMessageId===args.entry.data.clientMessageId))session.entries.push({...clone(args.entry),id:'saved_user'});return clone(session);
  case 'start_generation':{const id='run_'+hash(args.requestId).slice(0,32);const run={id,sessionId:session.id,userEntryId:'saved_user',status:runs.size?'succeeded':'running',content:'A的回复',contextSnapshotVersion:1,contextManifest:[],focus:clone(args.focus)};runs.set(id,run);if(run.status==='succeeded')session.entries.push({id:'reply',kind:'assistant',content:run.content,data:{runId:id,focus:clone(args.focus),contextManifest:[]}});return {};}
  case 'get_run':runReads++;return clone(runs.get(args.runId));
  case 'cancel_generation':{const run=runs.get(args.runId);run.status='cancelled';run.error={code:'CANCELLED',message:'Synthetic cancellation'};return clone(run);}
  default:throw Error('Unexpected IPC '+command);
 }},{shouldMockEvents:true});
 const root=createRoot(document.getElementById('root'));
 try{
  await React.act(async()=>root.render(React.createElement(ui.App)));await wait(()=>document.querySelector('[aria-label="向学习助手提问"]')?.disabled===false);
  const textarea=document.querySelector('[aria-label="向学习助手提问"]');await React.act(async()=>{Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(textarea,'解释A');textarea.dispatchEvent(new Event('input',{bubbles:true}));});
  await click(document.querySelector('[aria-label="发送问题"]'));await wait(()=>button('重试保存并生成'));assert.equal(document.querySelector('[aria-label="向学习助手提问"]').value,'解释A','preflight rejection retains the draft');await click(button('重试保存并生成'));await wait(()=>calls.some(c=>c.command==='start_generation'));
  await click([...document.querySelectorAll('.tree-row')].find(el=>el.title==='B笔记'));await wait(()=>document.querySelector('.document-title-row h1')?.textContent==='B笔记');
  const before=runReads;await wait(()=>runReads>before);assert.equal(document.querySelector('.context-sources .source-details')?.textContent.includes('documents/note_a.md')||false,false,'late A progress must not replace B preview');
  await click(document.querySelector('[aria-label="停止生成"]'));await wait(()=>button('重试生成')&&!document.querySelector('.generating'));
  docs.get('note_a').body='A changed after cancellation';docs.get('note_a').version='v2';
  await click(button('重试生成'));await wait(()=>calls.filter(c=>c.command==='start_generation').length===2&&!document.querySelector('.generating'));
  const starts=calls.filter(c=>c.command==='start_generation');assert.equal(freezeCount,2);assert.deepEqual(starts[1].args.focus,starts[0].args.focus);assert.equal(starts[1].args.documentId,'note_a');assert.equal(session.entries.filter(e=>e.kind==='user').length,1);
  assert.deepEqual(session.entries.find(e=>e.kind==='user').data.requestedContext.focus,focus());
  await React.act(async()=>root.render(null));await React.act(async()=>root.render(React.createElement(ui.App)));await wait(()=>document.querySelector('.message.assistant .context-sources'));
  assert.match(document.querySelector('.message.assistant .context-sources').textContent,/documents\/note_a.md/);assert.equal(document.querySelector('.document-title-row h1').textContent,'B笔记');
 }finally{await React.act(async()=>root.unmount());clearMocks();}
});
test.after(async()=>{clearMocks();dom.window.close();await unlink(file).catch(()=>{});});
