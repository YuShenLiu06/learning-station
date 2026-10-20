import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';
import { createHash } from 'node:crypto';

test('actual React course conversations create, select, rename and restore independently of reading (synthetic IPC)', async () => {
  const dom = new JSDOM('<div id="root"></div>', {url:'http://127.0.0.1/',pretendToBeVisual:true});
  Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true});
  Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
  for(const name of ['Window','Range','HTMLElement','HTMLInputElement','HTMLTextAreaElement','HTMLSelectElement','Element','Node','Event','MouseEvent','KeyboardEvent','MutationObserver','DOMRect'])globalThis[name]=dom.window[name];
  globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window);globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window);globalThis.getComputedStyle=dom.window.getComputedStyle.bind(dom.window);globalThis.ResizeObserver=class{observe(){}unobserve(){}disconnect(){}};
  dom.window.Range.prototype.getClientRects=()=>[];dom.window.Range.prototype.getBoundingClientRect=()=>new dom.window.DOMRect();
  const {mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');mockWindows('main');
  const filename=new URL(`./.course-conversations-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/App.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
  const React=await import('react'),{createRoot}=await import('react-dom/client'),{App}=await import(filename.href);
  const clone=value=>structuredClone(value),commands=[],drafts=new Map(),docs=new Map(),sessions=new Map(),selected=new Map([['course_a','old_chat']]),runs=new Map(),created=new Map();
  const doc=(id,kind,title,parentId=null)=>({id,path:`courses/${id}.md`,version:'v1',metadata:{id,kind,title,parentId,order:0,goals:[],tags:[],links:[],version:1},body:`# ${title}\n\nSynthetic private material ${id}`,content:''});
  for(const value of [doc('course_a','course','英语课程'),doc('note_a','section','材料甲','course_a'),doc('note_b','section','材料乙','course_a'),doc('course_b','course','空白课程'),doc('exercise_a','exercise','旧练习','note_a')])docs.set(value.id,value);
  const entry=(id,kind,content,data={})=>({id,kind,content,data,createdAt:'2026-10-20T00:00:00Z',relatedIds:[]});
  const session=(id,courseId,title)=>({id,courseId,sectionId:id==='old_chat'?'note_a':'',title,version:'v1',entries:[],contextManifest:[]});
  sessions.set('old_chat',session('old_chat','course_a','原有对话'));
  sessions.get('old_chat').entries.push(entry('old_answer','assistant','旧会话的独立历史'),entry('exercise_entry','exercise','Synthetic exercise',{exercises:[{id:'question_a',question:'原始账本题目',format:'exact',answers:['yes'],explanation:'Synthetic explanation',sourceIds:['note_a']}]}));
  docs.get('exercise_a').metadata.originSessionId='old_chat';docs.get('exercise_a').metadata.exerciseEntryId='exercise_entry';
  const nodes=()=>[...docs.values()].map(value=>({id:value.id,title:value.metadata.title,kind:value.metadata.kind,parentId:value.metadata.parentId,courseId:value.id==='course_b'?'course_b':'course_a',path:value.path,version:value.version}));
  let activeDocumentId='note_a',serial=0,loseCreateAck=false,failChatDraft=false,loseDraftAck=false,blockedNoteRead=null,blockedList=null;
  let prefs={fontSize:16,assistantWidth:450,sidebarWidth:256,sidebarCollapsed:false,expandedNodeIds:['course_a','course_b','note_a']};
  drafts.set('section:note_a',{key:'section:note_a',data:{question:'升级前的旧问题'},version:'old_draft',updatedAt:'2026-10-20T00:00:00Z'});
  const emptyContext={text:'',manifest:[],historyManifest:[],assistantReferenceManifest:[],exerciseReferenceManifest:[],warnings:[],truncated:false};
  const hash=value=>createHash('sha256').update(value).digest('hex').slice(0,32);
  const ipc=async(command,args={})=>{
    commands.push({command,args:clone(args)});
    switch(command){
      case 'get_app_state':return {root:'/synthetic',documents:[...docs.values()].map(value=>({...value.metadata,path:value.path,version:value.version})),activeDocumentId,lastCourseId:activeDocumentId==='course_b'?'course_b':'course_a',lastSessionId:selected.get(activeDocumentId==='course_b'?'course_b':'course_a'),model:{configured:true,verified:true,provider:'custom',baseUrl:'http://127.0.0.1/synthetic',model:'synthetic',harness:'pi-agent-core'}};
      case 'get_library_tree':return {nodes:nodes(),warnings:[]};
      case 'get_ui_preferences':return clone(prefs);
      case 'save_ui_preferences':prefs={...prefs,...args};return clone(prefs);
      case 'list_todos':case 'list_notifications':return [];
      case 'get_ui_draft':if(failChatDraft&&args.key.startsWith('chat:'))throw {code:'IO',message:'合成草稿读取失败'};return clone(drafts.get(args.key)||null);
      case 'save_ui_draft':{if((drafts.get(args.key)?.version??null)!==(args.expectedVersion??null))throw {code:'CONFLICT',message:'合成草稿版本冲突'};const value={key:args.key,data:args.data,version:'draft_'+(++serial),updatedAt:'2026-10-20T00:00:00Z'};drafts.set(args.key,value);if(loseDraftAck&&args.key.startsWith('chat:')){loseDraftAck=false;throw {code:'IO',message:'合成草稿回执丢失'};}return clone(value);}
      case 'open_document':if(blockedNoteRead&&args.id==='note_b')await blockedNoteRead.promise;activeDocumentId=args.id;return clone(docs.get(args.id));
      case 'get_exercise_document':return {session:clone(sessions.get('old_chat'))};
      case 'start_session':{const owner=nodes().find(value=>value.id===args.sectionId).courseId;return {session:clone(sessions.get(selected.get(owner))),context:emptyContext};}
      case 'get_session':return clone(sessions.get(args.id));
      case 'load_session_context':return clone(emptyContext);
      case 'course_sessions':{const snapshot={courseId:args.courseId,activeSessionId:selected.get(args.courseId)||null,sessions:[...sessions.values()].filter(value=>value.courseId===args.courseId).map(({id,title,version})=>({id,title,version}))};if(blockedList&&args.courseId==='course_a'){const delay=blockedList;blockedList=null;await delay.promise;}return snapshot;}
      case 'course_session_create':{
        let id=created.get(args.requestId);if(!id){id='chat_'+hash(args.requestId);created.set(args.requestId,id);sessions.set(id,session(id,args.courseId,`新对话 ${created.size}`));}
        selected.set(args.courseId,id);
        if(loseCreateAck){loseCreateAck=false;throw {code:'IO',message:'合成丢失回执'};}
        return clone(sessions.get(id));
      }
      case 'course_session_select':selected.set(args.courseId,args.sessionId);return clone(sessions.get(args.sessionId));
      case 'course_session_rename':{const value=sessions.get(args.sessionId);assert.equal(args.expectedVersion,value.version);value.title=args.title;value.version='v'+(++serial);return clone(value);}
      case 'append_entry':{const value=sessions.get(args.sessionId);let saved=value.entries.find(item=>item.data?.clientMessageId===args.requestId);if(!saved){saved={...args.entry,id:'entry_'+(++serial)};value.entries.push(saved);value.version='v'+serial;}return clone(value);}
      case 'start_generation':{const value=sessions.get(args.sessionId),id='run_'+hash(args.requestId);value.entries.push(entry('answer_'+(++serial),'assistant','本对话的合成回复',{runId:id}));runs.set(id,{id,sessionId:value.id,userEntryId:args.userEntryId,status:'succeeded',content:'',contextSnapshotVersion:1,contextManifest:[],historyManifest:[],courseReadManifest:[{id:'AUTHORIZED_BUT_UNSENT',path:'unsent.md',version:'unseen_version',includedChars:0}],toolCalls:[]});return {};}
      case 'get_run':if(runs.has(args.runId))return clone(runs.get(args.runId));throw {code:'NOT_FOUND',message:'synthetic not found'};
      case 'record_ui_metric':case 'plugin:event|listen':case 'plugin:event|unlisten':return serial++;
      default:throw new Error('Unexpected synthetic IPC '+command);
    }
  };
  mockIPC(ipc);let root=createRoot(document.getElementById('root'));
  const tick=async(ms=0)=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});
  const wait=async(check)=>{for(let i=0;i<100;i++){if(check())return;await tick(10);}assert.ok(check(),'UI timeout: '+document.body.textContent);};
  const click=async(element)=>{assert.ok(element,'control exists');await React.act(async()=>element.dispatchEvent(new MouseEvent('click',{bubbles:true,cancelable:true})));};
  const input=async(element,text)=>{await React.act(async()=>{const type=element.tagName==='TEXTAREA'?HTMLTextAreaElement:HTMLInputElement;Object.getOwnPropertyDescriptor(type.prototype,'value').set.call(element,text);element.dispatchEvent(new Event('input',{bubbles:true}));});};
  const row=title=>[...document.querySelectorAll('.tree-row')].find(value=>value.title===title);
  const newChat=owner=>[...document.querySelectorAll('.tree-row')].find(value=>value.title==='新建对话'&&value.dataset.courseId===owner);
  const button=text=>[...document.querySelectorAll('button')].find(value=>value.textContent.trim()===text);
  const composer=()=>document.querySelector('[aria-label="向学习助手提问"]');
  const heading=()=>document.querySelector('.assistant-heading h2')?.textContent;
  const reading=()=>document.querySelector('.document-title-row h1')?.textContent;
  try{
    await React.act(async()=>root.render(React.createElement(App)));await wait(()=>heading()==='原有对话'&&row('原有对话'));
    assert.equal(reading(),'材料甲');await wait(()=>button('恢复到当前对话')&&!button('恢复到当前对话').disabled);assert.match(document.querySelector('.model-prompt').textContent,/升级前的旧问题/);await click(button('恢复到当前对话'));assert.equal(composer().value,'升级前的旧问题');assert.equal(drafts.get('section:note_a').data.question,'');assert.match(document.querySelector('.messages').textContent,/旧会话的独立历史/);
    await input(composer(),'原会话未发送的问题');await click(row('材料乙'));await wait(()=>reading()==='材料乙');assert.equal(heading(),'原有对话');assert.equal(composer().value,'原会话未发送的问题');
    const beforeOpen=commands.filter(value=>value.command==='open_document').length;
    await React.act(async()=>{newChat('course_a').dispatchEvent(new MouseEvent('click',{bubbles:true}));newChat('course_a').dispatchEvent(new MouseEvent('click',{bubbles:true}));});
    await wait(()=>heading()==='新对话 1');const first=selected.get('course_a');
    assert.equal(commands.filter(value=>value.command==='course_session_create').length,1);assert.equal(commands.filter(value=>value.command==='open_document').length,beforeOpen);assert.equal(reading(),'材料乙');assert.equal(composer().value,'');assert.doesNotMatch(document.querySelector('.messages').textContent,/旧会话/);
    await input(composer(),'新会话的草稿');await click(row('原有对话'));await wait(()=>heading()==='原有对话');assert.equal(composer().value,'原会话未发送的问题');
    await click(row('新对话 1'));await wait(()=>heading()==='新对话 1');assert.equal(composer().value,'新会话的草稿');assert.equal(reading(),'材料乙');
    await click(button('重命名对话'));await input(document.querySelector('[aria-label="对话名称"]'),'语法讨论');await click(button('保存名称'));await wait(()=>heading()==='语法讨论'&&row('语法讨论'));assert.equal(selected.get('course_a'),first);
    let releaseRead;blockedNoteRead={promise:new Promise(resolve=>{releaseRead=resolve;})};
    await click(button('带我梳理这一页'));await tick();assert.equal(document.querySelector('[aria-label="发送问题"]').disabled,true);assert.equal(commands.filter(value=>value.command==='start_generation').length,0);
    blockedNoteRead=null;releaseRead();await wait(()=>composer().value.includes('请带我梳理本页')&&document.querySelector('.composer-reference'));
    await click(document.querySelector('[aria-label="移除提问引用 note_b"]'));
    const paragraph=document.querySelector('.reading-content p');const range=document.createRange();range.selectNodeContents(paragraph);
    await React.act(async()=>{window.getSelection().removeAllRanges();window.getSelection().addRange(range);document.querySelector('.document-scroll').dispatchEvent(new MouseEvent('mouseup',{bubbles:true}));});
    await wait(()=>document.querySelector('.selection-tray'));await click([...document.querySelectorAll('.selection-tray button')].find(value=>value.textContent==='留作复习'));await click(button('保存到复习'));
    assert.equal(commands.filter(value=>value.command==='append_entry'&&value.args.entry.data?.reviewCard).at(-1).args.entry.data.sourceDocumentId,'note_b');assert.equal(commands.filter(value=>value.command==='append_entry'&&value.args.entry.data?.reviewCard).at(-1).args.sessionId,first);
    await click(document.querySelector('[aria-label="取消选句"]'));window.getSelection().removeAllRanges();await click(button('重命名对话'));await wait(()=>document.querySelector('[aria-label="对话名称"]'));await click(button('保存名称'));await wait(()=>!document.querySelector('[aria-label="对话名称"]'));
    await input(composer(),'普通聊天问题');await click(document.querySelector('[aria-label="发送问题"]'));await wait(()=>document.querySelector('.messages')?.textContent.includes('本对话的合成回复')&&!document.querySelector('.generating'));
    assert.equal(commands.filter(value=>value.command==='start_generation').at(-1).args.documentId,'');assert.equal(commands.filter(value=>value.command==='append_entry'&&value.args.entry.kind==='user').at(-1).args.entry.data.requestedContext.documentId,'');assert.equal(sessions.get('old_chat').entries.filter(value=>value.kind==='user').length,0);assert.doesNotMatch(document.body.textContent,/AUTHORIZED_BUT_UNSENT|unseen_version/);
    await click(row('旧练习'));await wait(()=>reading()==='旧练习'&&document.querySelector('.exercise-workspace')?.textContent.includes('原始账本题目'));assert.equal(heading(),'语法讨论');
    await input(composer(),'重启后仍在本会话');await tick(650);await React.act(async()=>root.unmount());root=createRoot(document.getElementById('root'));await React.act(async()=>root.render(React.createElement(App)));await wait(()=>heading()==='语法讨论'&&reading()==='旧练习'&&composer().value==='重启后仍在本会话');
    await click(row('空白课程'));await wait(()=>reading()==='空白课程');assert.equal(composer().disabled,true);
    failChatDraft=true;const emptyCreate=newChat('course_b');await click(emptyCreate);await wait(()=>heading()==='新对话 2');assert.equal(reading(),'空白课程');assert.equal(composer().disabled,false);assert.equal(composer().value,'');assert.doesNotMatch(document.querySelector('.messages').textContent,/旧会话|合成回复/);await wait(()=>document.querySelector('.inline-error')?.textContent.includes('合成草稿读取失败'));await input(composer(),'读取失败时的新输入');failChatDraft=false;await click(button('重试读取草稿'));await tick();assert.equal(composer().value,'读取失败时的新输入');
    await click(row('语法讨论'));await wait(()=>heading()==='语法讨论');assert.equal(reading(),'英语课程');assert.equal(composer().value,'重启后仍在本会话');
    loseCreateAck=true;await click(newChat('course_a'));await wait(()=>document.querySelector('.toast')?.textContent.includes('合成丢失回执'));assert.equal(heading(),'语法讨论');
    await click(newChat('course_a'));await wait(()=>heading()==='新对话 3');const creates=commands.filter(value=>value.command==='course_session_create');assert.equal(creates.at(-1).args.requestId,creates.at(-2).args.requestId);assert.equal(sessions.size,4);assert.equal(sessions.get('old_chat').entries[0].content,'旧会话的独立历史');
    const lastKey='chat:'+selected.get('course_a'),readCount=()=>commands.filter(value=>value.command==='get_ui_draft'&&value.args.key===lastKey).length,saveCount=()=>commands.filter(value=>value.command==='save_ui_draft'&&value.args.key===lastKey).length;const readsBefore=readCount(),savesBefore=saveCount();
    loseDraftAck=true;await input(composer(),'保存后回执丢失的草稿');await wait(()=>document.querySelector('.inline-error')?.textContent.includes('合成草稿回执丢失'));await click(button('重试读取草稿'));await wait(()=>!document.querySelector('.inline-error')?.textContent.includes('合成草稿回执丢失'));assert.equal(composer().value,'保存后回执丢失的草稿');assert.equal(readCount()-readsBefore,1);assert.equal(saveCount()-savesBefore,1);await click(row('原有对话'));await wait(()=>heading()==='原有对话');
    let releaseList;blockedList={promise:new Promise(resolve=>{releaseList=resolve;})};const selectedCalls=commands.filter(value=>value.command==='course_session_select').length;await click(row('英语课程'));await click(row('语法讨论'));await wait(()=>heading()==='语法讨论');releaseList();await tick(50);assert.equal(selected.get('course_a'),first);assert.equal(heading(),'语法讨论');assert.equal(commands.filter(value=>value.command==='course_session_select').length-selectedCalls,1);
    assert.ok(drafts.has('chat:old_chat'));assert.ok(drafts.has('chat:'+first));
  }finally{await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(filename).catch(()=>{});}
});
