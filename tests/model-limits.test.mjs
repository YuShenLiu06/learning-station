import { fileURLToPath } from 'node:url';
import test from 'node:test';import assert from 'node:assert/strict';import {build} from 'esbuild';import {JSDOM} from 'jsdom';import {unlink} from 'node:fs/promises';
const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/',pretendToBeVisual:true});Object.assign(globalThis,{window:dom.window,document:dom.window.document,location:dom.window.location,isTauri:true,IS_REACT_ACT_ENVIRONMENT:true});Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});for(const name of ['HTMLElement','HTMLInputElement','HTMLSelectElement','HTMLTextAreaElement','Element','Node','Event','MouseEvent','KeyboardEvent'])globalThis[name]=dom.window[name];
const file=new URL(`./.model-limits-${process.pid}.mjs`,import.meta.url);await build({stdin:{contents:`export {ModelLimitsSettings,modelLimitsInput} from './src/ModelLimitsSettings';export {SettingsPage} from './src/SettingsPage';export {nativeCall} from './src/native';export {modelFailureMessage} from './src/modelPolicy';`,resolveDir:process.cwd(),loader:'tsx'},outfile:fileURLToPath(file),bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',define:{'import.meta.env.DEV':'false'}});
const React=await import('react'),{createRoot}=await import('react-dom/client'),ui=await import(file.href),{mockIPC,clearMocks,mockWindows}=await import('@tauri-apps/api/mocks');
const initial={configured:true,credentialStored:true,verified:true,connected:true,provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1/mock-only/v1',requestUrl:'http://127.0.0.1/mock-only/v1/chat/completions',model:'fixture-model',maxOutputTokens:4096,contextWindow:null};
const tick=async()=>React.act(async()=>{await new Promise(resolve=>setTimeout(resolve,0));});const wait=async check=>{for(let i=0;i<100;i++){if(check())return;await tick();}assert.ok(check(),'UI timeout: '+document.body.textContent);};
const input=async(label,value)=>{const el=document.querySelector(`[aria-label="${label}"]`);assert.ok(el,label);await React.act(async()=>{Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,value);el.dispatchEvent(new Event('input',{bubbles:true}));});};const button=text=>[...document.querySelectorAll('button')].find(x=>x.textContent.includes(text));const click=async el=>{assert.ok(el);await React.act(async()=>el.dispatchEvent(new MouseEvent('click',{bubbles:true})));};
const deferred=()=>{let resolve;const promise=new Promise(done=>resolve=done);return {promise,resolve};};
async function fixture(handler,run,config={}){const calls=[],saved=[];mockWindows('model-limits');mockIPC(async(command,args={})=>{calls.push({command,args:structuredClone(args)});return handler(command,args);});const root=createRoot(document.getElementById('root'));function Host({model=config.model||initial,blocked=false,unsavedConnection=false}){const[value,setValue]=React.useState(model);return React.createElement(ui.ModelLimitsSettings,{model:value,blocked,unsavedConnection,onSaved:next=>{saved.push(next);setValue(next);}});}const render=element=>React.act(async()=>root.render(element));try{await render(React.createElement(Host,config));await run({calls,saved,render,Host});}finally{await React.act(async()=>root.unmount());clearMocks();}}

test('output/context validation accepts exact limits and unknown, rejects fractions/ranges and insufficient input reserve',()=>{
 assert.deepEqual(ui.modelLimitsInput('1',''),{maxOutputTokens:1,contextWindow:null});assert.deepEqual(ui.modelLimitsInput('131072','2000000'),{maxOutputTokens:131072,contextWindow:2000000});assert.deepEqual(ui.modelLimitsInput('1','1025'),{maxOutputTokens:1,contextWindow:1025});
 for(const value of ['','0','131073','3.2','1e4','-1','Infinity'])assert.throws(()=>ui.modelLimitsInput(value,''));for(const value of ['1023','2000001','4.2','2e6'])assert.throws(()=>ui.modelLimitsInput('1',value));assert.throws(()=>ui.modelLimitsInput('1','1024'),/预留 1024/);assert.throws(()=>ui.modelLimitsInput('4096','5000'),/预留 1024/);
});

test('explicit limit save sends only two fields, preserves binding/model/verification and remount reads authoritative values',async()=>{
 let stored={...initial};await fixture((command,args)=>{assert.equal(command,'set_model_limits');assert.deepEqual(args,{maxOutputTokens:8192,contextWindow:null});stored={...stored,...args};return stored;},async({calls,saved,render,Host})=>{assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'4096');assert.equal(document.querySelector('[aria-label="上下文窗口"]').value,'');assert.match(document.body.textContent,/未知（未声明）/);await input('最大输出 token','8192');await click(button('保存生成限额'));await wait(()=>document.body.textContent.includes('限额已保存到本机'));assert.equal(calls.length,1);assert.equal(saved[0].provider,initial.provider);assert.equal(saved[0].model,initial.model);assert.equal(saved[0].verified,true);assert.deepEqual(Object.keys(calls[0].args),['maxOutputTokens','contextWindow']);await render(null);await render(React.createElement(Host,{model:stored}));assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'8192');assert.equal(document.querySelector('[aria-label="上下文窗口"]').value,'');assert.match(document.body.textContent,/可能包含思考 token/);assert.match(document.body.textContent,/不是精确 token 计数/);});
});

test('invalid local input and authoritative BUSY retain edited values without model requests or automatic retries',async()=>{
 await fixture(()=>{throw {code:'BUSY',message:'已有批改运行，请先停止'};},async({calls})=>{await input('最大输出 token','9000');await input('上下文窗口','9500');await click(button('保存生成限额'));assert.equal(calls.length,0);assert.match(document.body.textContent,/预留 1024/);await input('上下文窗口','20000');await click(button('保存生成限额'));await wait(()=>document.querySelector('[role="alert"]')?.textContent.includes('已有批改运行'));assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'9000');assert.equal(document.querySelector('[aria-label="上下文窗口"]').value,'20000');assert.equal(calls.length,1);await tick();assert.equal(calls.length,1);});
});

test('unconfigured, unsaved connection and global busy block writes; missing maximum requires explicit read and never guesses 32768',async()=>{
 for(const config of [{model:{...initial,configured:false}},{unsavedConnection:true},{blocked:true}])await fixture(()=>{throw Error('no command expected');},async({calls})=>{assert.equal(button('保存生成限额').disabled,true);assert.equal(document.querySelector('fieldset').disabled,true);await click(button('保存生成限额'));assert.equal(calls.length,0);},config);
 await fixture(command=>{assert.equal(command,'get_model_settings');return {...initial,maxOutputTokens:6144,contextWindow:null};},async({calls})=>{assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'');assert.equal(button('保存生成限额').disabled,true);assert.match(document.body.textContent,/尚未从本机服务读取/);assert.doesNotMatch(document.body.textContent,/32768/);assert.equal(calls.length,0);await click(button('重新读取限额'));await wait(()=>document.querySelector('[aria-label="最大输出 token"]').value==='6144');assert.equal(button('保存生成限额').disabled,false);assert.deepEqual(calls.map(x=>x.command),['get_model_settings']);},{model:{...initial,maxOutputTokens:undefined,contextWindow:undefined}});
});

test('same-frame duplicate saves are locked and do not clear a pending input draft',async()=>{
 const gate=deferred();await fixture(async(command,args)=>{assert.equal(command,'set_model_limits');await gate.promise;return {...initial,...args};},async({calls})=>{await input('最大输出 token','5000');await React.act(async()=>{const form=document.querySelector('form');form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));form.dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));});await wait(()=>calls.length===1);assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'5000');assert.match(button('正在保存限额').textContent,/正在保存/);gate.resolve();await wait(()=>document.body.textContent.includes('限额已保存到本机'));assert.equal(calls.length,1);});
});

test('provider page keeps limits in a separate form and never includes a typed secret in limit updates',async()=>{
 await fixture((command,args)=>{if(command==='get_grading_settings')return {model:null,effectiveModel:'fixture-model',configured:true,usesCurrentProvider:true};if(command==='get_feishu_bot_settings')return {configured:false,appId:'',ownerOpenId:'',chatId:'',receiveEnabled:false,deliveryEnabled:false};if(command==='check_feishu_status')return {ready:false,reason:'not configured'};if(command==='set_model_limits')return {...initial,...args};throw Error(command);},async({calls,render})=>{const font={fontSize:16,setFontSize(){},status:'',error:'',flush:async()=>{},retry:async()=>{}};await render(React.createElement(ui.SettingsPage,{model:initial,onClose(){},onSaved(){},font,blocked:false}));const block=document.querySelector('[aria-label="模型生成限额"]');assert.ok(block);assert.equal(block.querySelector('form form'),null);assert.equal(block.querySelector('[type="password"]'),null);await input('API 密钥','local-ui-only-not-a-real-key');assert.equal(button('保存生成限额').disabled,true);await input('API 密钥','');await input('模型服务地址','http://127.0.0.1/mock-only/another');assert.equal(button('保存生成限额').disabled,true);assert.match(block.textContent,/未保存修改/);await input('模型服务地址',initial.baseUrl);await input('最大输出 token','1234');await click(button('保存生成限额'));await wait(()=>calls.some(x=>x.command==='set_model_limits'));assert.deepEqual(calls.find(x=>x.command==='set_model_limits').args,{maxOutputTokens:1234,contextWindow:null});assert.equal(calls.some(x=>['configure_model','test_model','start_generation','set_model'].includes(x.command)),false);});
});

test('native model mapping preserves limits and keeps length versus incomplete diagnostics distinct',async()=>{
 mockIPC((command,args)=>{if(command==='get_model_settings')return {...initial,maxOutputTokens:32768,contextWindow:null};if(command==='set_model_limits'){assert.deepEqual(args,{maxOutputTokens:2,contextWindow:2048});return {...initial,...args};}throw Error(command);});try{assert.equal((await ui.nativeCall('model_get',{})).maxOutputTokens,32768);const saved=await ui.nativeCall('model_limits_save',{maxOutputTokens:2,contextWindow:2048,apiKey:'ignored-ui-fixture',model:'must-not-change'});assert.equal(saved.maxOutputTokens,2);assert.equal(saved.contextWindow,2048);assert.equal(saved.model,initial.model);assert.match(ui.modelFailureMessage({code:'MODEL_LENGTH',message:'length'},'generation'),/达到输出上限.*MODEL_LENGTH/);assert.match(ui.modelFailureMessage({code:'MODEL_INCOMPLETE',message:'EOF'},'generation'),/未完整结束.*MODEL_INCOMPLETE/);}finally{clearMocks();}
});

test('image input setting persists tri-state capability without losing edited token limits',async()=>{
 await fixture((command,args)=>{assert.equal(command,'set_model_image_support');return {...initial,...args};},async({calls})=>{
  await input('最大输出 token','5000');
  for(const [selected,expected] of [['on',true],['off',false],['auto',null]]){
   const select=document.querySelector('[aria-label="图片输入"]');
   await React.act(async()=>{select.value=selected;select.dispatchEvent(new Event('change',{bubbles:true}));});
   assert.ok(calls.length,document.body.textContent); assert.deepEqual(calls.at(-1).args,{supportsImages:expected});
   assert.equal(document.querySelector('[aria-label="图片输入"]').value,selected);
   assert.equal(document.querySelector('[aria-label="最大输出 token"]').value,'5000');
  }
 });
});

test('fixed coding channel does not offer unsupported image capability override',async()=>{
 await fixture(()=>assert.fail('no model request'),async()=>{
  assert.equal(document.querySelector('[aria-label="图片输入"]'),null);
  assert.match(document.body.textContent,/固定通道仅支持文本/);
 },{model:{...initial,provider:'zhipu-coding'}});
});

test.after(async()=>{clearMocks();dom.window.close();await unlink(file).catch(()=>{});await unlink(new URL(file.href.replace(/\.mjs$/u,'.css'))).catch(()=>{});});
