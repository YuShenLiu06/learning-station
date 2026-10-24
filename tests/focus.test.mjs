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
globalThis.getComputedStyle=dom.window.getComputedStyle.bind(dom.window);globalThis.ResizeObserver=class{observe(){}unobserve(){}disconnect(){}};HTMLElement.prototype.scrollIntoView=()=>{};Range.prototype.getClientRects=()=>[];Range.prototype.getBoundingClientRect=()=>new DOMRect();
const file=new URL(`./.focus-${process.pid}.mjs`,import.meta.url);
await build({stdin:{contents:`export {App} from './src/App';export {captureFocus,readFocus} from './src/focus';export {sessionMessages} from './src/sessionMessages';export {freezeRequestedContext,readRequestedContext} from './src/requestedContext';export {mergeOutgoing} from './src/outgoingMessages';`,resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
const React=await import('react'),{createRoot}=await import('react-dom/client'),ui=await import(file.href),{mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');
const hash=text=>createHash('sha256').update(text).digest('hex'),clone=v=>structuredClone(v);
const focus=(id='note_a')=>({id:hash(id),courseId:'course_c',documentId:id,sourceDocumentId:id,path:`documents/${id}.md`,type:'markdown',commitHash:'1'.repeat(40),materialTreeOid:'2'.repeat(40),excludedFiles:[],documentVersion:'v1',view:'read',paragraphs:[{start:0,end:4,text:'原文😀'}]});
const tick=async(ms=0)=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});
const wait=async check=>{for(let i=0;i<150;i++){if(check())return;await tick(10);}assert.ok(check(),'UI timeout: '+document.body.textContent);};
const click=async el=>{assert.ok(el);await React.act(async()=>el.dispatchEvent(new MouseEvent('click',{bubbles:true})));};
const button=text=>[...document.querySelectorAll('button')].find(el=>el.textContent.includes(text));

test('focus adapters assemble identity and view metadata only, without visible ranges or selection coordinates',()=>{
 const captured=ui.captureFocus('note_a','原文😀\n\n尾段','v1','read');assert.deepEqual(captured,{documentId:'note_a',expectedBody:'原文😀\n\n尾段',expectedVersion:'v1',view:'read',ranges:[]});
 assert.equal('expectedVersion' in ui.captureFocus('note_a','body',undefined,'read'),false);
 assert.equal(ui.captureFocus('exercise','body','v1','exercise',undefined,'q2').questionId,'q2');
 assert.deepEqual(ui.captureFocus('note_a','body',undefined,'asset','image_asset'),{documentId:'note_a',expectedBody:'body',view:'asset',ranges:[],assetId:'image_asset'});
 const parsed=ui.readFocus(focus());const original=focus();assert.deepEqual(parsed,original);assert.equal(ui.readFocus({...focus(),binary:true,paragraphs:[]}).binary,true);assert.equal(ui.readFocus({...focus(),sessionId:'chat_one'}).sessionId,'chat_one');assert.equal(ui.readFocus({...focus(),repositoryVersion:2}).repositoryVersion,2);original.paragraphs[0].text='later';assert.equal(parsed.paragraphs[0].text,'原文😀');
 for(const bad of [{...focus(),commitHash:'HEAD'},{...focus(),binary:'yes'},{...focus(),sessionId:42},{...focus(),sessionId:''},{...focus(),repositoryVersion:1},{...focus(),repositoryVersion:'2'},{...focus(),paragraphs:[{start:4,end:0,text:'x'}]},{...focus(),asset:{id:'image',mime:'image/png',size:-1}},{...focus(),exercise:{questionId:'q',number:0,answered:false,graded:false}}])assert.equal(ui.readFocus(bad),undefined);
});
test('empty course sources stay empty, frozen Focus wins and citation metadata remains deeply frozen',()=>{
 const citation={documentId:'note_b',expectedVersion:'v1',selection:'文',locator:{startChar:1,endChar:2}};
 const snapshot=focus();const frozen=ui.freezeRequestedContext('','',[citation],[],[],snapshot);citation.locator.startChar=99;snapshot.paragraphs[0].text='later';
 assert.equal(frozen.documentId,'note_a');assert.equal(frozen.injections[0].locator.startChar,1);assert.equal(frozen.focus.paragraphs[0].text,'原文😀');assert.deepEqual(ui.readRequestedContext(frozen),frozen);
 const base={sendId:'send',operationId:'operation',sessionId:'session',sectionId:'old_anchor',documentId:'',detached:false,mode:'explain',content:'question',selection:'',createdAt:'fixture',stage:'saving'};
 assert.equal(ui.mergeOutgoing([], [base], 'session')[0].requestedContext.documentId,'');assert.equal(ui.mergeOutgoing([], [{...base,focus:focus()}], 'session')[0].requestedContext.documentId,'note_a');
});
test('persisted replies restore their own Focus, including image/PDF metadata; old records never borrow current focus',()=>{
 const a=focus(),b={...focus('note_b'),type:'pdf',view:'asset',paragraphs:[],asset:{id:'pdf',mime:'application/pdf',size:20}};
 const mapped=ui.sessionMessages([{id:'a',kind:'assistant',content:'A',data:{focus:a,contextManifest:[]}},{id:'b',kind:'assistant',content:'B',data:{focus:b,contextManifest:[]}},{id:'old',kind:'assistant',content:'Old',data:{contextManifest:[]}}]);
 assert.deepEqual(mapped[0].context.focus,a);assert.deepEqual(mapped[1].context.focus,b);assert.equal(mapped[2].context.focus,undefined);assert.equal('text' in mapped[1].context.focus.asset,false);
});
test('actual React/native path keeps the original Focus through page switch, cancellation, retry and reopening',async()=>{
 const docs=new Map(['note_a','note_b'].map((id,index)=>[id,{id,path:`courses/${id}.md`,metadata:{id,kind:'section',title:index?'B笔记':'A笔记',parentId:'course_c',tags:[],goals:[],links:[]},body:'原文😀\n\n尾段',version:'v1'}]));
 docs.set('exercise_e',{id:'exercise_e',path:'exercises/exercise_e.md',metadata:{id:'exercise_e',kind:'exercise',title:'E练习',parentId:'note_a',originSessionId:'course_session',exerciseEntryId:'exercise_entry'},body:'# E练习',version:'v1'});
 const session={id:'course_session',courseId:'course_c',sectionId:'note_a',version:'s1',entries:[],contextManifest:[]};
 session.entries.push({id:'exercise_entry',kind:'exercise',content:'Q',data:{exercises:[{id:'q1',question:'First?',format:'exact',answers:['one']},{id:'q2',question:'Second?',format:'exact',answers:['two']}]}});
 let proposals=[];const calls=[],runs=new Map();let active='note_a',freezeCount=0,runReads=0,rejectFreeze=true;
 mockWindows('focus-isolated-fixture');mockIPC(async(command,args={})=>{calls.push({command,args:clone(args)});switch(command){
  case 'get_app_state':return {root:'/tmp/focus-fixture',activeDocumentId:active,documents:[...docs.values()].map(d=>({id:d.id,title:d.metadata.title,kind:'section',path:d.path,version:d.version})),model:{configured:true,provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1/mock',model:'fixture',usageGate:''}};
  case 'get_library_tree':return {nodes:[{id:'course_c',title:'C课程',kind:'course',path:'courses/course_c.md',version:'v1'},...[...docs.values()].map(d=>({id:d.id,title:d.metadata.title,kind:d.metadata.kind,parentId:d.metadata.parentId,courseId:'course_c',path:d.path,version:d.version}))],warnings:[]};
  case 'get_ui_preferences':return {fontSize:16,assistantWidth:346,sidebarWidth:240,sidebarCollapsed:false,expandedNodeIds:['course_c','note_a'],toolAuthorizationTier:'off'};
  case 'save_ui_preferences':return args;case 'get_ui_draft':return args.key==='section:note_b'?{key:args.key,data:{body:'仍未保存的本机草稿'},version:'draft',updatedAt:'fixture'}:null;case 'save_ui_draft':return {key:args.key,data:args.data,version:'draft',updatedAt:'fixture'};
  case 'edit_proposals':return proposals.filter(p=>p.courseId===args.courseId).map(p=>({id:p.id,stamp:'1'}));
  case 'edit_proposal':return clone(proposals.find(p=>p.id===args.id));
  case 'list_course_files':return [];
  case 'decide_edit_proposal':proposals=proposals.filter(p=>p.id!==args.id);return {persisted:false,status:'rejected'};
  case 'record_ui_metric':return null;case 'list_todos':case 'list_notifications':return [];
  case 'open_document':active=args.id;return clone(docs.get(args.id));case 'start_session':return {session:clone(session)};case 'get_session':return clone(session);
  case 'get_exercise_document':return {session:clone(session)};
  case 'load_session_context':return {text:'',manifest:[],warnings:[],truncated:false};
  case 'freeze_focus':{freezeCount++;if(rejectFreeze){rejectFreeze=false;throw {code:'CONTEXT_CONFLICT',message:'Synthetic preflight rejection'};}if(args.request.input.documentId==='exercise_e')return {...focus('exercise_e'),sourceDocumentId:'note_a',type:'exercise',view:args.request.input.view,paragraphs:[],exercise:{questionId:args.request.input.questionId||'q1',number:2,answered:false,graded:false}};assert.equal(args.request.input.documentId,'note_a');assert.equal(args.request.input.view,'read');return focus();}
  case 'append_entry':if(!session.entries.some(e=>e.data?.clientMessageId===args.entry.data.clientMessageId))session.entries.push({...clone(args.entry),id:'saved_user_'+session.entries.filter(e=>e.kind==='user').length});return clone(session);
  case 'start_generation':{const id='run_'+hash(args.requestId).slice(0,32);const run={id,sessionId:session.id,userEntryId:args.userEntryId,status:runs.size?'succeeded':'running',content:'A的回复',contextSnapshotVersion:1,contextManifest:[],focus:clone(args.focus)};runs.set(id,run);if(run.status==='succeeded')session.entries.push({id:'reply_'+id,kind:'assistant',content:run.content,data:{runId:id,focus:clone(args.focus),contextManifest:[]}});return {};}
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
  await click([...document.querySelectorAll('.tree-row')].find(el=>el.title==='E练习'));await wait(()=>document.querySelectorAll('.exercise-card').length===2);
  await React.act(async()=>document.querySelectorAll('.exercise-card textarea')[1].focus());
  async function sendAgain(){const box=document.querySelector('[aria-label="向学习助手提问"]');await React.act(async()=>{Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(box,'当前题');box.dispatchEvent(new Event('input',{bubbles:true}));});await click(document.querySelector('[aria-label="发送问题"]'));await wait(()=>!document.querySelector('.generating'));}
  await sendAgain();let frozenCall=calls.filter(c=>c.command==='freeze_focus').at(-1).args.request.input;assert.equal(frozenCall.questionId,'q2');assert.equal(frozenCall.view,'exercise');
  await click(button('编辑 Markdown'));await wait(()=>document.querySelector('[aria-label="编辑小节正文"]'));
  await sendAgain();frozenCall=calls.filter(c=>c.command==='freeze_focus').at(-1).args.request.input;assert.equal(frozenCall.view,'edit');assert.equal(frozenCall.questionId,'q2');
  await click([...document.querySelectorAll('.tree-row')].find(el=>el.title==='B笔记'));
  await click(button('编辑'));await wait(()=>document.querySelector('.document-editor:not([hidden]) [aria-label="编辑小节正文"]'));
  // Real App routing: a proposal arriving while CodeMirror is open is reachable
  // from the course sidebar, including two proposals for the same document.
  assert.equal(document.querySelector('[aria-label="文件变更审批"]'),null);
  proposals=[{id:'first',courseId:'course_c',path:'documents/note_b.md',original:'原文😀\n\n尾段',content:'# First proposal',status:'pending'}, {id:'second',courseId:'course_c',path:'documents/note_b.md',original:'原文😀\n\n尾段',content:'# Second proposal',status:'pending'}];
  await wait(()=>document.querySelectorAll('.course-sidebar-panel .review-file-link').length===2);
  assert.match(document.querySelector('[aria-label="文件变更审批"]').textContent,/文件变更审批 · 2/);
  assert.equal(document.querySelectorAll('.assistant-panel .review-file-link').length,0);
  for(const row of document.querySelectorAll('.review-file-link')){assert.ok(row.classList.contains('tree-row'));assert.ok(row.querySelector('.tree-title'));assert.match(row.getAttribute('aria-label'),/修改待确认/);assert.equal(row.querySelector('small'),null);assert.equal(row.querySelectorAll('svg').length,1);}
  const groupToggle=document.querySelector('[aria-label="文件变更审批"] .course-group-heading');
  await click(groupToggle);assert.equal(groupToggle.getAttribute('aria-expanded'),'false');
  assert.equal(groupToggle.nextElementSibling.hidden,true);await click(groupToggle);
  const countBefore=calls.length;
  await click(document.querySelectorAll('.course-sidebar-panel .review-file-link')[1]);
  assert.equal(document.querySelector('.document-editor:not([hidden])'),null);
  assert.match(document.querySelector('.inline-edit-review').textContent,/Second proposal/);
  assert.equal(button('接受').disabled,true,'unsaved draft prevents acceptance');
  assert.match(document.querySelector('[aria-label="文件变更审批"]').textContent,/B笔记/);
  assert.equal(calls.slice(countBefore).some(c=>['open_document','save_document','decide_edit_proposal'].includes(c.command)),false,'locating the current document neither reloads nor saves/accepts it');
  assert.match(document.querySelector('.toolbar-hint').textContent,/审阅修改/);
  await click([...document.querySelectorAll('.tree-row')].find(el=>el.title==='A笔记'));
  await wait(()=>document.querySelector('.document-title-row h1')?.textContent==='A笔记');
  const beforeCross=calls.filter(c=>c.command==='open_document').length;
  await click(document.querySelectorAll('.course-sidebar-panel .review-file-link')[1]);
  assert.equal(document.querySelector('.document-title-row h1').textContent,'B笔记');
  assert.match(document.querySelector('.inline-edit-review').textContent,/Second proposal/);
  assert.equal(calls.filter(c=>c.command==='open_document').length,beforeCross+1,'cross-document review verifies once and reuses the result');
  assert.equal(button('接受').disabled,true,'cross-document navigation retains the existing local draft');
  await click(button('拒绝'));assert.equal(calls.filter(c=>c.command==='decide_edit_proposal').at(-1).args.id,'second');
  await click(button('拒绝'));assert.equal(document.querySelector('[aria-label="文件变更审批"]'),null);
  await click(button('编辑'));assert.match(document.querySelector('.document-editor:not([hidden])').textContent,/仍未保存的本机草稿/);
  assert.equal(calls.slice(countBefore).some(c=>c.command==='save_document'),false);


 }finally{await React.act(async()=>root.unmount());clearMocks();}
});
test.after(async()=>{clearMocks();dom.window.close();await unlink(file).catch(()=>{});});
