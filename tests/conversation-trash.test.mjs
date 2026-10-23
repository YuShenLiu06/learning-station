import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('conversation moves to the recoverable trash from its context menu and restores with its id', async () => {
  const dom = new JSDOM('<div id="root"></div>', {url:'http://127.0.0.1/',pretendToBeVisual:true});
  Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true});
  Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
  for(const name of ['Window','Range','HTMLElement','HTMLInputElement','HTMLTextAreaElement','HTMLSelectElement','Element','Node','Event','MouseEvent','KeyboardEvent','MutationObserver','DOMRect'])globalThis[name]=dom.window[name];
  globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window);globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window);globalThis.getComputedStyle=dom.window.getComputedStyle.bind(dom.window);globalThis.ResizeObserver=class{observe(){}unobserve(){}disconnect(){}};
  dom.window.Range.prototype.getClientRects=()=>[];dom.window.Range.prototype.getBoundingClientRect=()=>new dom.window.DOMRect();
  const {mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');mockWindows('main');
  const filename=new URL(`./.conversation-trash-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/App.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
  const React=await import('react'),{createRoot}=await import('react-dom/client'),{App}=await import(filename.href);
  const clone=value=>structuredClone(value),commands=[];
  const docs=new Map(),sessions=new Map(),trash=new Map(),drafts=new Map(),archived=new Map();
  const selected=new Map([['course_a','old_chat']]);
  let serial=0,trashed=false;
  const doc=(id,kind,title,parentId=null)=>({id,path:`courses/${id}.md`,version:'v1',metadata:{id,kind,title,parentId,order:0,goals:[],tags:[],links:[],version:1},body:`# ${title}\n\nSynthetic private material ${id}`,content:''});
  for(const value of [doc('course_a','course','英语课程'),doc('note_a','section','材料甲','course_a')])docs.set(value.id,value);
  const session=(id,title)=>({id,courseId:'course_a',sectionId:'',title,version:'v1',entries:[],contextManifest:[]});
  sessions.set('old_chat',session('old_chat','原有对话'));
  sessions.get('old_chat').entries.push({id:'old_answer',kind:'assistant',content:'原有对话的独立历史',data:{},createdAt:'2026-10-20T00:00:00Z',relatedIds:[]});
  sessions.set('spare_chat',session('spare_chat','备用对话'));
  const nodes=()=>[...docs.values()].map(value=>({id:value.id,title:value.metadata.title,kind:value.metadata.kind,parentId:value.metadata.parentId,courseId:'course_a',path:value.path,version:value.version}));
  let prefs={fontSize:16,assistantWidth:450,sidebarWidth:256,sidebarCollapsed:false,expandedNodeIds:['course_a','note_a']};
  const emptyContext={text:'',manifest:[],historyManifest:[],assistantReferenceManifest:[],exerciseReferenceManifest:[],warnings:[],truncated:false};
  const conversations=()=>({courseId:'course_a',activeSessionId:selected.get('course_a')||null,sessions:[...sessions.values()].map(({id,title,version})=>({id,title,version}))});
  const ipc=async(command,args={})=>{
    commands.push({command,args:clone(args)});
    switch(command){
      case 'get_app_state':return {root:'/synthetic',documents:[...docs.values()].map(value=>({...value.metadata,path:value.path,version:value.version})),activeDocumentId:'note_a',lastCourseId:'course_a',lastSessionId:selected.get('course_a'),model:{configured:true,verified:true,provider:'custom',baseUrl:'http://127.0.0.1/synthetic',model:'synthetic',harness:'pi-agent-core'}};
      case 'get_library_tree':return {nodes:nodes(),warnings:[]};
      case 'get_ui_preferences':return clone(prefs);
      case 'save_ui_preferences':prefs={...prefs,...args};return clone(prefs);
      case 'list_todos':case 'list_notifications':return [];
      case 'get_ui_draft':return clone(drafts.get(args.key)||null);
      case 'save_ui_draft':{const value={key:args.key,data:args.data,version:'draft_'+(++serial),updatedAt:'2026-10-20T00:00:00Z'};drafts.set(args.key,value);return clone(value);}
      case 'open_document':return clone(docs.get(args.id));
      case 'start_session':return {session:clone(sessions.get(selected.get('course_a'))),context:emptyContext};
      case 'get_session':{const value=sessions.get(args.id);if(!value)throw {code:'TRASHED',message:'该内容已在回收站中，恢复后才能读取或编辑。'};return clone(value);}
      case 'load_session_context':return clone(emptyContext);
      case 'course_sessions':return clone(conversations());
      case 'course_session_select':selected.set('course_a',args.sessionId);return clone(sessions.get(args.sessionId));
      case 'course_session_create':throw new Error('synthetic create not expected');
      case 'append_entry':throw new Error('synthetic append not expected');
      case 'start_generation':throw new Error('synthetic generation not expected');
      case 'preview_library_trash':return {id:args.id,title:sessions.get(args.id)?.title||'原有对话',kind:'session',nodeCount:0,sessionCount:1,version:'tp1'};
      case 'trash_library_node':{
        assert.equal(args.expectedVersion,'tp1');assert.ok(typeof args.requestId==='string'&&args.requestId.length>=8);
        const value=sessions.get(args.id);if(!value)throw {code:'NOT_FOUND',message:'synthetic session missing'};
        archived.set(args.id,clone(sessions.get(args.id)));sessions.delete(args.id);selected.delete('course_a');trashed=true;
        const receipt={trashId:'trash_1',id:args.id,title:value.title,kind:'session',nodeCount:0,sessionCount:1,createdAt:'2026-10-22T00:00:00Z',restoredAt:null,status:'trashed'};
        trash.set(receipt.trashId,receipt);return clone(receipt);
      }
      case 'list_library_trash':return clone([...trash.values()]);
      case 'restore_library_node':{const receipt=trash.get(args.trashId);const restored={...clone(receipt),status:'restored',restoredAt:'2026-10-22T00:01:00Z'};trash.set(args.trashId,restored);if(restored.kind==='session')sessions.set(restored.id,archived.get(restored.id));return clone(restored);}
      case 'record_ui_metric':case 'plugin:event|listen':case 'plugin:event|unlisten':return serial++;
      default:throw new Error('Unexpected synthetic IPC '+command);
    }
  };
  mockIPC(ipc);let root=createRoot(document.getElementById('root'));
  const tick=async(ms=0)=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});
  const wait=async(check)=>{for(let i=0;i<100;i++){if(check())return;await tick(10);}assert.ok(check(),'UI timeout: '+document.body.textContent);};
  const click=async(element)=>{assert.ok(element,'control exists');await React.act(async()=>element.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true})));};
  const row=title=>[...document.querySelectorAll('.tree-row')].find(value=>value.title===title);
  const button=text=>[...document.querySelectorAll('button')].find(value=>value.textContent.trim()===text);
  const composer=()=>document.querySelector('[aria-label="向学习助手提问"]');
  const heading=()=>document.querySelector('.assistant-heading h2')?.textContent;
  try{
    await React.act(async()=>root.render(React.createElement(App)));
    await wait(()=>heading()==='原有对话'&&row('原有对话')&&row('备用对话'));
    // The conversation's context menu keeps one entry point: rename plus trash.
    await React.act(async()=>row('原有对话').dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:40,clientY:100})));
    await wait(()=>document.querySelector('.tree-context-menu'));
    const items=[...document.querySelectorAll('.tree-context-menu [role="menuitem"]')].map(value=>value.textContent);
    assert.deepEqual(items,['重命名对话','移至回收区']);
    await click(items.length?document.querySelectorAll('.tree-context-menu [role="menuitem"]')[1]:null);
    await wait(()=>document.querySelector('.trash-scope'));
    await React.act(async()=>{const box=document.querySelector('.checkbox-label input');assert.ok(box,'confirmation checkbox exists');box.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true}));});
    await click(button('确认移至回收区'));
    await wait(()=>commands.some(value=>value.command==='trash_library_node'));
    assert.deepEqual(commands.find(value=>value.command==='preview_library_trash').args,{id:'old_chat'});
    assert.equal(commands.find(value=>value.command==='trash_library_node').args.id,'old_chat');
    // No dangling active conversation: the composer waits for an explicit choice.
    await wait(()=>row('原有对话')===undefined&&row('备用对话'));
    assert.equal(composer().disabled,true);
    assert.equal(document.body.textContent.includes('原有对话的独立历史'),false);
    // The receipt is recoverable from the trash panel and returns to the course list.
    await click(document.querySelector('button[aria-label="回收区"]'));
    await wait(()=>document.querySelector('.trash-page')?.textContent.includes('原有对话'));
    await click(button('恢复'));
    await wait(()=>row('原有对话')!==undefined&&row('备用对话'));
    assert.equal(trashed,true);
    assert.deepEqual(commands.find(value=>value.command==='restore_library_node').args,{trashId:'trash_1',requestId:commands.find(value=>value.command==='restore_library_node').args.requestId});
    await click(row('原有对话'));
    await wait(()=>heading()==='原有对话');
    assert.equal(document.querySelector('.messages').textContent.includes('原有对话的独立历史'),true);
  }finally{await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(filename).catch(()=>{});}
});
