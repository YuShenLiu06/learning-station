import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {requestText,requestDiff} from '../src/requestDebug.ts';
const record=(sequence,session,body,extra={})=>({sequence,identity:{sessionId:session},complete:true,body,...extra});
test('actual captured JSON strings expose Focus, citations and Changes; diff never crosses sessions',()=>{
  const before=record(1,'s1',{messages:[{role:'user',content:JSON.stringify({focus:{body:'Focus A'},citationContext:'citation A',changes:'Changes A'})}]});
  const after=record(3,'s1',{messages:[{role:'user',content:JSON.stringify({focus:{body:'Focus B'},citationContext:'citation A',changes:'Changes B'})}]},{previousSequence:1});
  assert.match(requestText(after),/Focus B/);assert.match(requestText(after),/citation A/);assert.match(requestText(after),/Changes B/);
  const diff=requestDiff(before,after);assert.match(diff,/--- 请求 #1/);assert.match(diff,/\+\+\+ 请求 #3/);assert.match(diff,/- Focus A/);assert.match(diff,/\+ Focus B/);
  assert.match(requestDiff({...before,identity:{sessionId:'s2'}},after),/不同会话/);
  assert.match(requestDiff(undefined,after),/基线 #1 已从有界内存移除/);
  assert.match(requestDiff(before,{...after,complete:false,omitted:'256 KiB'}),/不完整/);
  assert.match(requestText({...after,complete:false,omitted:'256 KiB'}),/不完整：256 KiB/);
  assert.match(requestDiff(before,before),/未变化/);
});
test('only Settings contains the debug switch; lifecycle clears departed session; packaged sidecar includes capture',()=>{
  const settings=readFileSync(new URL('../src/SettingsPage.tsx',import.meta.url),'utf8');
  const app=readFileSync(new URL('../src/App.tsx',import.meta.url),'utf8');
  assert.match(settings,/RequestDebugSettings/);assert.doesNotMatch(app,/RequestDebugSettings|开启请求调试/);
  assert.match(app,/request_debug_clear_session/);
  const config=JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json',import.meta.url)));
  assert.equal(config.bundle.resources['../pi-sidecar/request-debug.mjs'],'pi-sidecar/request-debug.mjs');
});
test('debug commands pass through the real native adapter to Tauri invoke',async()=>{
  const {build}=await import('esbuild');const {unlink}=await import('node:fs/promises');
  const file=new URL(`./.debug-native-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/native.ts'],outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',plugins:[{name:'tauri-invoke',setup(b){b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'core',namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const invoke=(...args)=>globalThis.debugNativeInvoke(...args);',loader:'js'}));}}]});
  const calls=[];globalThis.debugNativeInvoke=async(command,args)=>{calls.push([command,args]);return {enabled:args.enabled??false,records:[],dropped:0};};
  try{
    const {nativeCall}=await import(file.href);
    for(const [command,args] of [['request_debug_get',{}],['request_debug_set',{enabled:true}],['request_debug_enter_session',{sessionId:'synthetic-session'}],['request_debug_clear_session',{sessionId:'synthetic-session'}]]) {
      await nativeCall(command,args);assert.deepEqual(calls.at(-1),[command,args]);
    }
    assert.equal(calls.length,4);
    await assert.rejects(nativeCall('request_debug_not_registered',{}));assert.equal(calls.length,4);
  }finally{delete globalThis.debugNativeInvoke;await unlink(file);}
});
test('real settings component toggles capture and clears shown request after disabling',async()=>{
  const {build}=await import('esbuild');const {JSDOM}=await import('jsdom');const {unlink}=await import('node:fs/promises');
  const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/',pretendToBeVisual:true});Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true});Object.defineProperty(globalThis,'navigator',{configurable:true,value:dom.window.navigator});
  const file=new URL(`./.debug-ui-${process.pid}.mjs`,import.meta.url);
  await build({stdin:{contents:"export {RequestDebugSettings} from './src/RequestDebugSettings';",resolveDir:process.cwd(),loader:'tsx'},outfile:file.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',plugins:[{name:'debug-api',setup(b){b.onResolve({filter:/^\.\/api$/},()=>({path:'api',namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const call=(...args)=>globalThis.debugTestCall(...args);',loader:'js'}));}}]});
  const React=await import('react');const {createRoot}=await import('react-dom/client');const {RequestDebugSettings}=await import(file.href);
  let enabled=false;const commands=[];const entries=[record(1,'s1',{messages:[{role:'user',content:'Focus A'}]}),record(2,'s1',{messages:[{role:'user',content:'Focus B'}]},{previousSequence:1})].map(r=>({...r,phase:'initial',status:'HTTP 200',time:'fixture',bodyBytes:100}));
  entries[1].registrations=[{key:'actual_provider',lifetime:'OneShot',position:'References',value:'registered line one\nregistered line two'}];
  entries[1].sources=[{path:['messages',0,'content'],origin:'bridge',name:'learning-station-material',sourcePath:'native/fixture/material'}];
  globalThis.debugTestCall=async(command,args)=>{commands.push(command);if(command==='request_debug_set')enabled=args.enabled;return {enabled,records:enabled?entries:[],dropped:0};};
  const root=createRoot(document.getElementById('root'));
  try{
    await React.act(async()=>root.render(React.createElement(RequestDebugSettings)));
    assert.equal(document.querySelector('input').checked,false);assert.match(document.body.textContent,/已关闭，不采集/);
    await React.act(async()=>document.querySelector('input').click());assert.equal(enabled,true);assert.match(document.querySelector('pre').textContent,/Focus B/);
    await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent.startsWith('diff')).click());assert.match(document.querySelector('pre').textContent,/- Focus A/);
    await React.act(async()=>[...document.querySelectorAll('button')].find(b=>b.textContent==='按来源 / 注册键').click());assert.match(document.body.textContent,/actual_provider/);assert.match(document.body.textContent,/OneShot/);assert.match(document.body.textContent,/registered line one\nregistered line two/);assert.match(document.body.textContent,/learning-station-material/);
    await React.act(async()=>document.querySelector('input').click());assert.equal(enabled,false);assert.equal(document.querySelector('pre'),null);assert.doesNotMatch(document.body.textContent,/Focus [AB]/);assert.ok(commands.includes('request_debug_set'));
  }finally{await React.act(async()=>root.unmount());delete globalThis.debugTestCall;await unlink(file);dom.window.close();}
});
test('readable message view excludes tool schemas and decodes JSON once without altering code literals',()=>{
  const body={model:'fixture-model',max_tokens:99,messages:[{role:'user',content:JSON.stringify({frames:[{kind:'full',text:'line one\nline two\ncode: \\n'}]})}],tools:[{function:{description:'TOOL_SCHEMA_MUST_STAY_COLLAPSED'}}]};
  const text=requestText(record(1,'s1',body));assert.match(text,/line one\nline two/);assert.match(text,/code: \\n/);assert.doesNotMatch(text,/TOOL_SCHEMA_MUST_STAY_COLLAPSED|fixture-model|max_tokens/);
  const first=requestDiff(undefined,record(1,'s1',body));assert.match(first,/无前次记录，展示完整请求/);assert.match(first,/TOOL_SCHEMA_MUST_STAY_COLLAPSED/);
});
