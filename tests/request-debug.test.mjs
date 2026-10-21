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
