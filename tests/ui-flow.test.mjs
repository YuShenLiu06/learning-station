import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';
import { createHash } from 'node:crypto';

// Real React components and native command mapping, with an in-process Tauri bridge.
// No window automation, network service, credentials or actual notification sending.
test('controlled React flow preserves next draft, retries one saved user entry, changes only model, and checks Feishu idempotently', async () => {
  const dom = new JSDOM('<div id="root"></div>', {url:'http://127.0.0.1/',pretendToBeVisual:true});
  globalThis.window=dom.window;globalThis.document=dom.window.document;globalThis.location=dom.window.location;
  Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
  for (const name of ['HTMLElement','HTMLInputElement','HTMLTextAreaElement','Element','Node','Event','MouseEvent','KeyboardEvent']) globalThis[name]=dom.window[name];
  globalThis.requestAnimationFrame=dom.window.requestAnimationFrame.bind(dom.window);
  globalThis.cancelAnimationFrame=dom.window.cancelAnimationFrame.bind(dom.window);
  globalThis.IS_REACT_ACT_ENVIRONMENT=true;globalThis.isTauri=true;
  const {mockIPC,mockWindows,clearMocks}=await import('@tauri-apps/api/mocks');
  const commands=[];const runs=new Map();let generationCount=0;let releaseSave;let practiceReads=0;
  const saveGate=new Promise(resolve=>{releaseSave=resolve;});
  const clone=value=>structuredClone(value);
  const model={configured:true,provider:'custom',protocol:'openai-completions',model:'fixture-original',baseUrl:'http://127.0.0.1:1/v1',requestUrl:'http://127.0.0.1:1/v1/chat/completions',verified:true,usageGate:''};
  const session={id:'session_fixture',sectionId:'section_fixture',title:'fixture',version:'v1',entries:[],contextManifest:[]};
  const context={text:'local fixture',manifest:[],warnings:[],truncated:false};
  let prefs={fontSize:16,assistantWidth:346,sidebarCollapsed:false,expandedNodeIds:[]};let feishuCount=0;const todos=[];
  const toolReceipt={id:'activity_fixture',toolCallId:'call_fixture',runId:'',sessionId:session.id,name:'create_todo',status:'succeeded',title:'untrusted title',summary:'untrusted summary',startedAt:'now',updatedAt:'now',persisted:true,object:{kind:'todo',id:'todo_fixture',sessionId:session.id,sectionId:session.sectionId}};
  mockWindows('main');
  mockIPC(async (command,args={})=>{
    commands.push({command,args:clone(args)});
    switch(command){
      case 'get_app_state':return {root:'/tmp/fake-learning-station',documents:[{id:'section_fixture',path:'fixture.md',title:'Local fixture',kind:'section',tags:[],version:'v1'}],activeDocumentId:'section_fixture',model:clone(model)};
      case 'get_ui_preferences':return clone(prefs);
      case 'get_library_tree':return {nodes:[{id:'section_fixture',parentId:null,courseId:'section_fixture',kind:'section',title:'Local fixture',version:'v1',order:0,path:'fixture.md'}],warnings:[]};
      case 'save_ui_preferences':prefs={...prefs,...args};return clone(prefs);
      case 'list_todos':return clone(todos);
      case 'list_notifications':case 'list_review_items':return [];
      case 'get_ui_draft':return null;
      case 'save_ui_draft':return {key:args.key,data:args.data,version:'draft_v1'};
      case 'record_ui_metric':return null;
      case 'open_document':return {id:'section_fixture',path:'fixture.md',metadata:{id:'section_fixture',title:'Local fixture',kind:'section',goals:[],tags:[],links:[],version:1},body:'# Local fixture\nReading material.',content:'',version:'v1',modifiedAt:'now'};
      case 'start_session':return {session:clone(session)};
      case 'get_session':return clone(session);
      case 'list_sessions':return [{id:session.id,title:'fixture session history',kind:'session',path:'fixture.md',version:session.version,modifiedAt:'2026-10-16T00:00:00Z'}];
      case 'load_session_context':return clone(context);
      case 'freeze_focus':{const {input,sessionId}=args.request;assert.equal(input.documentId,'section_fixture');assert.deepEqual(input.ranges,[]);assert.equal(input.expectedBody,'# Local fixture\nReading material.');return {id:createHash('sha256').update(sessionId+input.documentId).digest('hex'),sessionId,courseId:'',documentId:input.documentId,sourceDocumentId:input.documentId,path:'fixture.md',type:'markdown',view:input.view,commitHash:'',materialTreeOid:'',documentVersion:'c'.repeat(64),excludedFiles:[],paragraphs:[]};}
      case 'append_entry':{
        await saveGate;
        if(!session.entries.some(entry=>entry.data?.clientMessageId===args.entry.data?.clientMessageId)) session.entries.push({...args.entry,id:'entry_user_fixture',createdAt:'now'});
        session.version='v2';return clone(session);
      }
      case 'start_generation':{
        if(args.mode==='practice'){const id='run_'+createHash('sha256').update(args.requestId).digest('hex').slice(0,32);runs.set(id,{id,sessionId:session.id,status:'running',content:'{\"answers\":[\"REFERENCE_MUST_STAY_HIDDEN\"],\"explanation\":\"secret\"}',toolCalls:[]});return clone(runs.get(id));}
        generationCount++;assert.equal(args.userEntryId,'entry_user_fixture');
        const id='run_'+createHash('sha256').update(args.requestId).digest('hex').slice(0,32);
        runs.set(id,{id,sessionId:session.id,userEntryId:args.userEntryId,status:generationCount===1?'failed':'succeeded',content:'',toolCalls:generationCount===2?[{...toolReceipt,runId:id}]:[],error:generationCount===1?{code:'MODEL_RESPONSE',message:'controlled failure'}:undefined});
        if(generationCount===2){session.entries.push({id:'entry_assistant',kind:'assistant',content:'Controlled fixture response',createdAt:'now',relatedIds:[],data:{runId:id}},{id:'entry_tool',kind:'note',content:'internal activity snapshot',createdAt:'now',data:{origin:'pi-tool-activity',toolActivity:{...toolReceipt,runId:id}}});todos.push({id:'todo_fixture',title:'Controlled persisted task',done:false,reminderEnabled:false});}
        return clone(runs.get(id));
      }
      case 'get_run':if(runs.has(args.runId)){const value=runs.get(args.runId);if(value.content.includes('REFERENCE_MUST_STAY_HIDDEN')&&++practiceReads>1){value.status='failed';value.error={code:'MODEL_RESPONSE',message:'Controlled invalid practice'};}return clone(value);}throw {code:'NOT_FOUND',message:'not found'};
      case 'set_model':Object.assign(model,{model:args.model,verified:false});return clone(model);
      case 'get_feishu_bot_settings':return {configured:true,appId:'fixture',ownerOpenId:'fixture',chatId:'fixture',receiveEnabled:true,deliveryEnabled:true,secretStored:true,runtimeAvailable:true,runtimeReason:''};
      case 'get_feishu_bot_snapshot':return {state:'connected',reason:'controlled fixture',counts:{},processExited:false,outputClosed:false};
      case 'check_feishu_status':return {ready:true,reason:'Controlled fake bridge only'};
      case 'test_feishu_notification':return ++feishuCount===1?{status:'unknown',messageId:null,reason:'Controlled unknown result'}:{status:'sent',messageId:'fake_receipt',reason:'Controlled fake receipt'};
      default:throw new Error('Unexpected fake IPC: '+command);
    }
  },{shouldMockEvents:true});
  const filename=new URL(`./.ui-flow-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/App.tsx'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
  const React=await import('react');const {createRoot}=await import('react-dom/client');
  const {App}=await import(filename.href);const root=createRoot(document.getElementById('root'));
  const tick=async(ms=0)=>{await React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,ms));});};
  const waitFor=async(check)=>{for(let i=0;i<30;i++){if(check())return;await tick(10);}assert.ok(check(),'controlled UI condition timed out: '+document.body.textContent+' commands='+commands.map(x=>x.command).join(','));};
  const click=async(element)=>{assert.ok(element,'button exists');await React.act(async()=>element.dispatchEvent(new MouseEvent('click',{bubbles:true})));};
  const input=async(element,value)=>{await React.act(async()=>{Object.getOwnPropertyDescriptor(element instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(element,value);element.dispatchEvent(new Event('input',{bubbles:true}));});};
  const key=async(element,key)=>{await React.act(async()=>element.dispatchEvent(new KeyboardEvent('keydown',{key,bubbles:true,cancelable:true})));};
  const button=text=>[...document.querySelectorAll('button')].find(item=>item.textContent.includes(text));
  try{
    await React.act(async()=>root.render(React.createElement(App)));await waitFor(()=>document.querySelector('[aria-label="向学习助手提问"]')?.disabled===false&&document.querySelector('.reading-content'));
    const composer=document.querySelector('[aria-label="向学习助手提问"]');assert.ok(document.querySelector('.composer-bottom .chat-model-current'));assert.equal(document.querySelectorAll('[aria-label="收起学习助手"]').length,1);
    await input(composer,'first saved question');await key(composer,'Enter');
    assert.equal(composer.value,'');assert.match(document.querySelector('.messages').textContent,/first saved question/);assert.match(document.querySelector('.messages').textContent,/正在保存问题/);
    await input(composer,'next unsent draft');await key(composer,'Enter');await key(composer,'Enter');
    assert.equal(commands.filter(x=>x.command==='append_entry').length,1);
    await React.act(async()=>releaseSave());await waitFor(()=>button('重试生成'));
    assert.equal(composer.value,'next unsent draft');assert.equal(document.querySelectorAll('.message.user').length,1);
    assert.equal(generationCount,1);assert.equal(session.entries.filter(x=>x.kind==='user').length,1);await click(button('重试生成'));await waitFor(()=>document.querySelector('.message.assistant'));
    assert.equal(document.querySelectorAll('.tool-activity').length,1);assert.ok(document.querySelector('.message.assistant .tool-activity'),'receipt must stay adjacent to its real run reply');assert.match(document.querySelector('.tool-activity').textContent,/创建待办.*成功/);assert.doesNotMatch(document.querySelector('.messages').textContent,/internal activity snapshot/);
    await click(button('打开待办'));await waitFor(()=>document.querySelector('[role="dialog"]'));assert.match(document.querySelector('[role="dialog"]').textContent,/Controlled persisted task/);await click(document.querySelector('[aria-label="关闭弹窗"]'));
    const starts=commands.filter(x=>x.command==='start_generation');assert.equal(starts.length,2);assert.notEqual(starts[0].args.requestId,starts[1].args.requestId);assert.equal(starts[0].args.userEntryId,starts[1].args.userEntryId);
    assert.equal(commands.filter(x=>x.command==='append_entry').length,1);assert.equal(composer.value,'next unsent draft');assert.equal(document.querySelectorAll('.message.user').length,1);
    await click(button('生成练习子文档'));await waitFor(()=>practiceReads===1);assert.equal(document.querySelector('.partial-message'),null);assert.doesNotMatch(document.querySelector('.assistant-scroll').textContent,/REFERENCE_MUST_STAY_HIDDEN/);await tick(390);await waitFor(()=>document.querySelector('.recovery-message pre'));assert.match(document.querySelector('.recovery-message summary').textContent,/模型原返回（含参考）/);assert.equal(document.querySelector('.recovery-message').open,false);
    for(let i=0;i<4;i++)await click(document.querySelector('[aria-label="折叠为图标栏"],[aria-label="展开侧边栏"]'));await click(document.querySelector('[aria-label="折叠为图标栏"]'));await tick(250);assert.equal(prefs.sidebarCollapsed,true);assert.equal(document.querySelector('[aria-label="向学习助手提问"]'),composer);assert.equal(composer.value,'next unsent draft');
    await click(document.querySelector('.chat-model-current'));await input(document.querySelector('[aria-label="对话模型 ID"]'),'fixture-new');await click(button('应用模型'));await waitFor(()=>document.querySelector('.chat-model-current').textContent.includes('fixture-new'));
    assert.deepEqual(commands.filter(x=>x.command==='set_model').map(x=>x.args),[{model:'fixture-new'}]);assert.equal(document.querySelector('[aria-label="向学习助手提问"]'),composer);assert.equal(composer.value,'next unsent draft');
    const separator=document.querySelector('[aria-label="调整学习助手宽度"]');await key(separator,'ArrowRight');await tick(300);
    const widthWrite=commands.filter(x=>x.command==='save_ui_preferences').at(-1);assert.deepEqual(widthWrite.args,{assistantWidth:330});assert.equal(document.querySelector('[aria-label="向学习助手提问"]'),composer);
    await click(document.querySelector('[aria-label="供应商设置"]'));const address=document.querySelector('[aria-label="模型服务地址"]');await input(address,'http://127.0.0.1:9/v1');await click(document.querySelector('[aria-controls="settings-appearance"]'));await click(document.querySelector('[aria-controls="settings-provider"]'));assert.equal(document.querySelector('[aria-label="模型服务地址"]'),address);assert.equal(address.value,'http://127.0.0.1:9/v1');await click(document.querySelector('[aria-controls="settings-notifications"]'));await waitFor(()=>button('发送测试消息')&&!button('发送测试消息').disabled);
    assert.equal(document.querySelector('[role="dialog"]'),null);assert.match(document.querySelector('.settings-page h1').textContent,/设置/);assert.equal(feishuCount,0);
    await click(button('发送测试消息'));await waitFor(()=>button('核对上次发送结果'));await click(button('核对上次发送结果'));await waitFor(()=>document.querySelector('.feishu-settings').textContent.includes('测试消息已发送'));
    const tests=commands.filter(x=>x.command==='test_feishu_notification');assert.equal(tests.length,2);assert.equal(tests[0].args.requestId,tests[1].args.requestId);assert.match(tests[0].args.requestId,/^[A-Za-z0-9_-]{8,128}$/);
    assert.equal(commands.some(x=>['test_model','configure_model'].includes(x.command)),false);
    await click(document.querySelector('[aria-label="历史"]'));await waitFor(()=>button('fixture session history'));await click(button('fixture session history'));await waitFor(()=>document.querySelector('.history-page .tool-activity'));assert.equal(document.querySelectorAll('.history-page .tool-activity').length,1);assert.ok(document.querySelector('.history-page .message.assistant .tool-activity'));assert.match(document.querySelector('.history-page .tool-activity').textContent,/创建待办.*成功/);await click(button('打开待办'));await waitFor(()=>document.querySelector('[role="dialog"]'));assert.match(document.querySelector('[role="dialog"]').textContent,/Controlled persisted task/);assert.equal(commands.filter(x=>x.command==='start_generation').filter(x=>x.args.mode!=='practice').length,2);
  }finally{
    await React.act(async()=>root.unmount());clearMocks();dom.window.close();await unlink(filename).catch(()=>{});
  }
});
