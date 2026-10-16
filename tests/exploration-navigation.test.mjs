import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { isExplorationDocument } from '../src/explorationPolicy.ts';

const folderId = 'node_' + createHash('sha256').update('learning-station-exploration-folder-v1').digest('hex').slice(0,32);
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

test('exploration classification uses stable ancestry, including section notes and cycles', () => {
 const nodes = [{id:folderId,kind:'folder'},{id:'nested',kind:'folder',parentId:folderId},{id:'old-note',kind:'section',parentId:'nested'},{id:'legacy',kind:'exploration'},{id:'ordinary',kind:'section'},{id:'cycle',kind:'section',parentId:'cycle'},{id:'same-title',title:'探索',kind:'folder'}];
 assert.equal(isExplorationDocument(nodes,'old-note',folderId),true);
 assert.equal(isExplorationDocument(nodes,'legacy',folderId),true);
 assert.equal(isExplorationDocument(nodes,'ordinary',folderId),false);
 assert.equal(isExplorationDocument(nodes,'cycle',folderId),false);
 assert.equal(isExplorationDocument(nodes,'same-title',folderId),false);
});

test('actual React exploration entry preserves old data and rejects late navigation acknowledgements (mock IPC)', async t => {
 const dom = new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/',pretendToBeVisual:true});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true});
 Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
 for(const name of ['Window','Range','HTMLElement','HTMLInputElement','HTMLTextAreaElement','HTMLSelectElement','Element','Node','Event','MouseEvent','KeyboardEvent','MutationObserver','DOMRect']) globalThis[name]=dom.window[name];
 globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window);globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window);globalThis.getComputedStyle=dom.window.getComputedStyle.bind(dom.window);globalThis.ResizeObserver=class{observe(){}unobserve(){}disconnect(){}};
 dom.window.Range.prototype.getClientRects=()=>[];dom.window.Range.prototype.getBoundingClientRect=()=>new dom.window.DOMRect();
 const {mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');
 const filename=new URL(`./.exploration-navigation-${process.pid}.mjs`,import.meta.url);
 await build({entryPoints:['src/App.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
 const React=await import('react'),{createRoot}=await import('react-dom/client'),{emit}=await import('@tauri-apps/api/event'),{App}=await import(filename.href);
 const tick=async(ms=0)=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});
 const wait=async(check)=>{for(let i=0;i<70;i++){if(check())return;await tick(10);}assert.ok(check(),'UI timeout: '+document.body.textContent);};
 const click=async(el)=>{assert.ok(el,'control exists');await React.act(async()=>{el.dispatchEvent(new MouseEvent('click',{bubbles:true}));});};
 const input=async(value)=>{const el=document.querySelector('[aria-label="想探索的问题"]');await React.act(async()=>{Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(el,value);el.dispatchEvent(new Event('input',{bubbles:true}));});};
 const nav=label=>document.querySelector(`[aria-label="${label}"]`);
 const button=(text,scope=document)=>[...scope.querySelectorAll('button')].find(el=>el.textContent.includes(text));
 const entry=()=>document.querySelector('[aria-label="想探索的问题"]');
 const atEntry=()=>document.querySelector('.exploration-records')?.hidden===false;
 let root, host, serial=0;
 function fixture(options={}) {
  const suffix=String(++serial),oldId='old-note-'+suffix,ordinaryId='ordinary-'+suffix;
  const clone=x=>structuredClone(x),commands=[],drafts=new Map(),docs=new Map(),sessions=new Map();
  const doc=(id,kind,title,parentId=null)=>({id,path:`nodes/${id}.md`,version:'v1',metadata:{id,kind,title,parentId,order:0,goals:[],tags:[],links:[],version:1},body:'# '+title,content:''});
  docs.set(folderId,doc(folderId,'folder','探索'));docs.set(oldId,doc(oldId,options.kind||'section','旧问题 '+suffix,options.kind==='exploration'?null:folderId));docs.set(ordinaryId,doc(ordinaryId,'section','普通材料 '+suffix));
  const oldSession={id:'session_'+oldId,sectionId:oldId,title:'旧问题 '+suffix,version:'s1',entries:[{id:'old-user-'+suffix,kind:'user',content:'旧问题原文 '+suffix,createdAt:'2026-10-16T00:00:00Z',relatedIds:[],data:{}}],contextManifest:[]};sessions.set(oldSession.id,oldSession);
  const legacy={key:'exploration-launch',data:{question:'红黑树',requestId:'legacy-original-request'},version:'legacy-v1',updatedAt:'2026-10-16T00:00:00Z'};drafts.set(legacy.key,clone(legacy));
  const nodes=()=>[...docs.values()].map(d=>({id:d.id,kind:d.metadata.kind,title:d.metadata.title,parentId:d.metadata.parentId,path:d.path,version:d.version,order:0}));
  const h={oldId,ordinaryId,oldSession,commands,drafts,docs,sessions,nodes,legacy,bootstrapGate:null,treeGate:null,historyGate:null,createGate:null,draftGate:null,saveGate:null,failDraftRead:false,createStarted:false};
  mockWindows('exploration-entry-mock-'+suffix);
  mockIPC(async(command,args={})=>{
   commands.push({command,args:clone(args)});
   switch(command){
    case 'get_ui_preferences': return {fontSize:16,assistantWidth:346,sidebarWidth:240,sidebarCollapsed:false,expandedNodeIds:[folderId]};
    case 'save_ui_preferences': return {fontSize:16,assistantWidth:346,sidebarWidth:240,sidebarCollapsed:false,expandedNodeIds:[folderId],...args};
    case 'get_app_state': if(h.bootstrapGate)await h.bootstrapGate.promise;return {root:'/tmp/react-mock-only',activeDocumentId:options.ordinary?ordinaryId:oldId,documents:[...docs.values()].map(d=>({id:d.id,kind:d.metadata.kind,title:d.metadata.title,path:d.path,version:d.version,tags:[]})),model:{configured:true,provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1/mock-only',model:'mock-only',usageGate:''}};
    case 'get_library_tree': if(h.treeGate)await h.treeGate.promise;return {nodes:nodes(),warnings:[]};
    case 'list_todos': case 'list_notifications': return [];
    case 'record_ui_metric': case 'plugin:window|destroy': return null;
    case 'get_ui_draft': if(args.key.startsWith('exploration-launch:')&&h.failDraftRead)throw {code:'IO',message:'Controlled draft read failure'};if(args.key==='section:'+oldId&&h.draftGate)await h.draftGate.promise;return clone(drafts.get(args.key)||null);
    case 'save_ui_draft': {if(args.data.requestId&&args.key!=='exploration-launch')throw {code:'INVALID_DRAFT',message:'Core permits requestId only on its explicit keys'};if(h.saveGate&&args.key.startsWith('exploration-launch:'))await h.saveGate.promise;const existing=drafts.get(args.key);assert.equal(args.expectedVersion,existing?.version,'draft writes preserve native expectedVersion');const saved={key:args.key,data:clone(args.data),version:'draft-'+commands.length,updatedAt:'2026-10-16T00:00:00Z'};drafts.set(args.key,saved);return clone(saved);}
    case 'open_document': return clone(docs.get(args.id));
    case 'start_session': {const id='session_'+args.sectionId;if(!sessions.has(id))sessions.set(id,{id,sectionId:args.sectionId,title:docs.get(args.sectionId).metadata.title,version:'s1',entries:[],contextManifest:[]});return {session:clone(sessions.get(id))};}
    case 'get_session': if(h.historyGate)await h.historyGate.promise;return clone(sessions.get(args.id));
    case 'list_sessions': return [...sessions.values()].map(s=>({id:s.id,title:s.title,kind:'session',path:'sessions/'+s.id+'.md',version:s.version}));
    case 'load_session_context': return {text:'',manifest:[],historyManifest:[],warnings:[],truncated:false};
    case 'create_exploration_note': {h.createStarted=true;const id='created-'+args.requestId,sid='session_'+id;const document=doc(id,'section',args.question,folderId);docs.set(id,document);const session={id:sid,sectionId:id,title:args.question,version:'s1',entries:[{id:'entry-'+args.requestId,kind:'user',content:args.question,selection:'',createdAt:'2026-10-16T00:00:00Z',relatedIds:[],data:{clientMessageId:args.requestId}}],contextManifest:[]};sessions.set(sid,session);if(h.createGate)await h.createGate.promise;return {document:clone(document),session:clone(session),userEntryId:session.entries[0].id,folderId};}
    default: throw new Error('Unexpected mock IPC '+command);
   }
  },{shouldMockEvents:true});
  return h;
 }
 async function mount(options={}){host=fixture(options);root=createRoot(document.getElementById('root'));await React.act(async()=>root.render(React.createElement(App)));return host;}
 async function unmount(){if(root){await React.act(async()=>root.unmount());root=null;}clearMocks();}
 try {
  await t.test('startup skips fixed-folder section; every entry is blank and legacy/unsent drafts survive close',async()=>{
   const h=await mount();await wait(atEntry);await wait(()=>!nav('开始探索')?.disabled || entry()?.value==='');
   assert.equal(entry().value,'');assert.equal(entry().disabled,false);assert.doesNotMatch(document.querySelector('.exploration-records').textContent,/提出新问题|返回探索记录/);assert.equal(document.querySelector('.exploration-records>.section-heading'),null);
   assert.equal(h.commands.some(x=>['open_document','start_session','create_exploration_note','start_generation','get_session'].includes(x.command)),false);
   await input('未提交问题A');await click(nav('待办'));await click(nav('探索'));assert.equal(entry().value,'');await wait(()=>[...h.drafts.values()].some(x=>x.data.question==='未提交问题A'));
   await input('未提交问题B');await click(nav('探索'));assert.equal(entry().value,'');await wait(()=>[...h.drafts.values()].some(x=>x.data.question==='未提交问题B'));
   await tick(520);await React.act(async()=>{await emit('tauri://close-requested');});await wait(()=>h.commands.some(x=>x.command==='plugin:window|destroy'));
   assert.deepEqual(h.drafts.get('exploration-launch'),h.legacy);
   assert.equal(h.commands.some(x=>['get_ui_draft','save_ui_draft'].includes(x.command)&&x.args.key==='exploration-launch'),false);
   assert.equal([...h.drafts.values()].filter(x=>x.key.startsWith('exploration-launch:')).length,2,'pristine blank visits do not write empty drafts');
   await unmount();
  });
  await t.test('legacy exploration kind is not restored either',async()=>{
   const h=await mount({kind:'exploration'});await wait(atEntry);assert.equal(entry().value,'');assert.equal(h.commands.some(x=>['open_document','start_session'].includes(x.command)),false);await unmount();
  });
  await t.test('late bootstrap and tree cannot override an explicit route or its close',async()=>{
   host=fixture({ordinary:true});host.bootstrapGate=deferred();root=createRoot(document.getElementById('root'));await React.act(async()=>root.render(React.createElement(App)));
   await click(nav('探索'));await click(button('返回学习',document.querySelector('.exploration-records')));host.bootstrapGate.resolve();await wait(()=>host.commands.some(x=>x.command==='get_library_tree'));await tick(40);assert.equal(atEntry(),false);assert.equal(document.querySelector('.document-title-row'),null);assert.equal(host.commands.some(x=>x.command==='open_document'),false);await unmount();
   host=fixture({ordinary:true});host.treeGate=deferred();root=createRoot(document.getElementById('root'));await React.act(async()=>root.render(React.createElement(App)));await wait(()=>host.commands.some(x=>x.command==='get_library_tree'));await click(nav('探索'));await click(nav('待办'));host.treeGate.resolve();await tick(40);assert.ok(nav('待办').classList.contains('active'));assert.equal(host.commands.some(x=>x.command==='open_document'),false);await unmount();
  });
  await t.test('explicit history loads old question, but a late history response cannot reopen a closed page',async()=>{
   const h=await mount();await wait(atEntry);await click(nav('历史'));await wait(()=>button('旧问题',document.querySelector('main > .utility-page:not([hidden])')));h.historyGate=deferred();await click(button('旧问题',document.querySelector('main > .utility-page:not([hidden])')));await wait(()=>document.querySelector('.history-page'));await click(button('回到历史',document.querySelector('.history-page')));h.historyGate.resolve();await tick(30);assert.equal(document.querySelector('.history-page'),null);
   h.historyGate=null;await wait(()=>button('旧问题',document.querySelector('main > .utility-page:not([hidden])')));await click(button('旧问题',document.querySelector('main > .utility-page:not([hidden])')));await wait(()=>document.querySelector('.history-page')?.textContent.includes('旧问题原文'));assert.equal(h.commands.some(x=>x.command==='create_exploration_note'),false);await click(button('打开来源文档',document.querySelector('.history-page')));await wait(()=>document.querySelector('.document-title-row h1')?.textContent.startsWith('旧问题'));await click(nav('探索'));assert.equal(entry().value,'');await click(button('返回学习',document.querySelector('.exploration-records')));assert.equal(document.querySelector('.document-title-row'),null,'old exploration does not reappear via back');await unmount();
  });
  await t.test('late document draft cannot repaint or start its session after leaving',async()=>{
   const h=await mount();await wait(atEntry);h.draftGate=deferred();h.drafts.set('section:'+h.oldId,{key:'section:'+h.oldId,data:{question:'迟到旧问题'},version:'old-v1'});await click([...document.querySelectorAll('.tree-row')].find(el=>el.title==='旧问题 '+serial));await wait(()=>h.commands.some(x=>x.command==='open_document'));await click(nav('探索'));await click(button('返回学习',document.querySelector('.exploration-records')));h.draftGate.resolve();await tick(40);assert.equal(entry().value,'');assert.equal(h.commands.some(x=>x.command==='start_session'),false);assert.equal(document.querySelector('.document-title-row'),null);await unmount();
  });
  await t.test('late create receipt retains one real note/user without stealing a newer blank entry or sending model',async()=>{
   const h=await mount();await wait(atEntry);h.createGate=deferred();await input('迟到创建问题');await click(nav('开始探索'));await wait(()=>h.createStarted);const original=h.commands.find(x=>x.command==='create_exploration_note');await click(nav('待办'));await click(nav('探索'));assert.equal(entry().value,'');h.createGate.resolve();await tick(60);assert.equal(atEntry(),true);assert.equal(entry().value,'');assert.equal(h.commands.filter(x=>x.command==='create_exploration_note').length,1);assert.equal(h.commands.some(x=>x.command==='open_document'||x.command==='start_generation'),false);assert.equal([...h.sessions.values()].find(s=>s.sectionId==='created-'+original.args.requestId).entries.length,1);assert.ok([...h.drafts.values()].some(x=>x.data.question==='迟到创建问题'&&x.key==='exploration-launch:'+original.args.requestId));await unmount();
  });
  await t.test('leaving while pre-save is pending never begins a stale creation',async()=>{
   const h=await mount();await wait(atEntry);h.saveGate=deferred();await input('只备份不创建');await click(nav('开始探索'));await wait(()=>h.commands.some(x=>x.command==='save_ui_draft'));await click(nav('待办'));h.saveGate.resolve();await tick(40);assert.ok(nav('待办').classList.contains('active'));assert.equal(h.commands.some(x=>x.command==='create_exploration_note'),false);assert.ok([...h.drafts.values()].some(x=>x.data.question==='只备份不创建'&&x.key.startsWith('exploration-launch:')));await unmount();
  });
  await t.test('failed draft read retains departed input and blocks close until backup succeeds',async()=>{
   const h=await mount();await wait(atEntry);h.failDraftRead=true;await click(nav('探索'));await wait(()=>document.querySelector('.exploration-start .inline-error'));await input('读取失败时输入');await click(nav('探索'));await tick();await React.act(async()=>{await emit('tauri://close-requested');});await tick(30);assert.equal(h.commands.some(x=>x.command==='plugin:window|destroy'),false);assert.equal(h.commands.some(x=>x.command==='save_ui_draft'),false);h.failDraftRead=false;await React.act(async()=>{await emit('tauri://close-requested');});await wait(()=>h.commands.some(x=>x.command==='plugin:window|destroy'));assert.ok([...h.drafts.values()].some(x=>x.data.question==='读取失败时输入'));assert.deepEqual(h.drafts.get('exploration-launch'),h.legacy);await unmount();
  });
  await t.test('ordinary reading still restores and survives an empty exploration visit',async()=>{
   const h=await mount({ordinary:true});await wait(()=>document.querySelector('.document-title-row h1')?.textContent.startsWith('普通材料'));await click(nav('探索'));assert.equal(entry().value,'');await click(button('返回学习',document.querySelector('.exploration-records')));assert.ok(document.querySelector('.document-title-row h1')?.textContent.startsWith('普通材料'));assert.equal(h.commands.some(x=>x.command==='create_exploration_note'),false);await unmount();
  });
 } finally { await unmount();dom.window.close();await unlink(filename).catch(()=>{}); }
});
