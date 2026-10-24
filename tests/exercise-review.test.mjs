import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('whole-question review renders options, answers and unchanged questions, persists one decision and exits', async () => {
 const dom = new JSDOM('<div id="root"></div>', {url:'http://127.0.0.1/'});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true,requestAnimationFrame:callback=>setTimeout(callback,0),cancelAnimationFrame:clearTimeout});
 const filename=new URL(`./.exercise-review-${process.pid}.mjs`,import.meta.url);
 await build({stdin:{contents:`import React from 'react'; import {useEditReview,EditReviewContent,EditReviewDock} from './src/EditReview'; export function Harness(props){ const review=useEditReview(props); return <>{review.proposal?<EditReviewContent review={review}/>:<p>Normal exercises</p>}<EditReviewDock review={review}/></>}`,resolveDir:process.cwd(),loader:'tsx'},outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',plugins:[{name:'fixture',setup(b){b.onResolve({filter:/^\.\/api$/},()=>({path:'ipc',namespace:'fixture'}));b.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const nativeAvailable=true;export const call=(...args)=>globalThis.exerciseReviewCall(...args);'}));}}]});
 const React=await import('react');const {createRoot}=await import('react-dom/client');const {Harness}=await import(filename.href);const root=createRoot(document.getElementById('root'));
 const q=(id,question,answer)=>({id,question,answers:[answer],format:'exact',explanation:`解析 ${answer}`});
 const original=[q('old','旧题\n\nA. 第一项\nB. 第二项','A'),q('same','未修改题','S'),q('removed','删除题','D')];
 const changes=[{id:'old',before:original[0],after:q('new-id','新题\n\nA. 第一项\nB. 第二项','B'),status:'pending'},{id:'new_0',after:q('added','新增题','N'),status:'pending'},{id:'removed',before:original[2],status:'pending'}];
 const proposal={id:'p',courseId:'c',path:'documents/ex.md',original:JSON.stringify({exercises:original}),content:'private-json',status:'pending',exercise:{documentId:'ex',version:'v1',changes}};
 let visible=true;const calls=[];let refreshes=0;
 globalThis.exerciseReviewCall=async(command,args)=>{
  if(command==='edit_proposals')return visible?[{id:'p',stamp:changes.map(c=>c.status).join()}]:[];
  if(command==='edit_proposal')return structuredClone(proposal);
  if(command==='decide_exercise_proposal'){calls.push(args);const change=changes.find(c=>c.id===args.questionId);change.status=args.accept?'accepted':'rejected';const complete=changes.every(c=>c.status!=='pending');visible=!complete;return{persisted:args.accept,status:change.status,complete};}
  throw Error(command);
 };
 try {
  await React.act(async()=>{root.render(React.createElement(Harness,{courseId:'c',documentId:'ex',isDirty:()=>false,isLoaded:()=>true,onAccepted:async()=>refreshes++,onOpen:async()=>{},notify:()=>{}}));await new Promise(r=>setTimeout(r,15));});
  assert.match(document.body.textContent,/旧题/);assert.match(document.body.textContent,/新题/);assert.match(document.body.textContent,/第一项/);assert.match(document.body.textContent,/解析 A/);assert.match(document.body.textContent,/解析 B/);
  assert.match(document.body.textContent,/未修改题/);assert.match(document.body.textContent,/新增题目/);assert.match(document.body.textContent,/删除题目/);
  assert.doesNotMatch(document.body.textContent,/private-json|"exercises"|new-id/);
  const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
  await React.act(async()=>button('接受此题修改').click());
  assert.equal(calls.length,1);assert.equal(calls[0].questionId,'old');assert.equal(refreshes,1);
  assert.doesNotMatch(document.body.textContent,/旧题/);assert.match(document.body.textContent,/新题/);
  await React.act(async()=>button('接受新增题目').click());
  await React.act(async()=>button('拒绝此题修改').click());
  assert.equal(calls.length,3);assert.equal(refreshes,2);assert.equal(document.querySelector('.review-dock'),null);assert.match(document.body.textContent,/Normal exercises/);
 } finally {await React.act(async()=>root.unmount());dom.window.close();delete globalThis.exerciseReviewCall;await unlink(filename);}
});

test('exercise acceptance and rejection pass through the real native adapter to Tauri invoke', async () => {
 const filename = new URL(`./.exercise-native-${process.pid}.mjs`, import.meta.url);
 await build({entryPoints:['src/native.ts'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',plugins:[{name:'exercise-tauri-invoke',setup(b){
  b.onResolve({filter:/^@tauri-apps\/api\/core$/},()=>({path:'core',namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const invoke=(...args)=>globalThis.exerciseNativeInvoke(...args);',loader:'js'}));
 }}]});
 const calls=[];
 let failure;
 globalThis.exerciseNativeInvoke=async(command,args)=>{
  calls.push([command,args]);
  if(failure)throw failure;
  return {persisted:args.accept,status:args.accept?'accepted':'rejected',complete:false,version:'saved-version'};
 };
 try {
  const {nativeCall}=await import(filename.href);
  for(const accept of [true,false]){
   const payload={courseId:'course',id:'proposal',questionId:'question',accept};
   const result=await nativeCall('decide_exercise_proposal',payload);
   assert.deepEqual(calls.at(-1),['decide_exercise_proposal',payload]);
   assert.deepEqual(result,{persisted:accept,status:accept?'accepted':'rejected',complete:false,version:'saved-version'});
  }
  failure={code:'CONFLICT',message:'原题已变化，建议保留。'};
  await assert.rejects(nativeCall('decide_exercise_proposal',{courseId:'course',id:'proposal',questionId:'question',accept:true}),error=>error===failure);
  assert.equal(calls.length,3);
 } finally {delete globalThis.exerciseNativeInvoke;await unlink(filename);}
});

test('an unloaded unrelated exercise draft blocks acceptance but never rejection', async () => {
 const dom=new JSDOM('<div id="root"></div>',{url:'http://127.0.0.1/'});
 Object.assign(globalThis,{window:dom.window,document:dom.window.document,IS_REACT_ACT_ENVIRONMENT:true,requestAnimationFrame:callback=>setTimeout(callback,0),cancelAnimationFrame:clearTimeout});
 const filename=new URL(`./.exercise-draft-rejection-${process.pid}.mjs`,import.meta.url);
 await build({stdin:{contents:`import React from 'react';import {useEditReview,EditReviewContent,EditReviewDock} from './src/EditReview';import {useExerciseDraft} from './src/exerciseDraft';export function Harness(props){const draft=useExerciseDraft('unrelated-question','');const review=useEditReview(props);return <><button onClick={()=>draft.setAnswer('保留的未提交答案')}>输入其他题草稿</button><output>{draft.answer}</output>{review.proposal?<EditReviewContent review={review}/>:<p>Normal exercises</p>}<EditReviewDock review={review}/></>}`,resolveDir:process.cwd(),loader:'tsx'},outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm',jsx:'automatic',plugins:[{name:'draft-rejection-ipc',setup(b){
  b.onResolve({filter:/^\.\/api$/},()=>({path:'ipc',namespace:'fixture'}));
  b.onLoad({filter:/.*/,namespace:'fixture'},()=>({contents:'export const nativeAvailable=true;export const call=(...args)=>globalThis.draftRejectionCall(...args);'}));
 }}]});
 const React=await import('react');const {createRoot}=await import('react-dom/client');const {Harness}=await import(filename.href);const root=createRoot(document.getElementById('root'));
 const before={id:'old',question:'原题',answers:['A'],format:'exact',explanation:'解析'};
 const proposal={id:'p',courseId:'c',path:'documents/ex.md',original:JSON.stringify({exercises:[before]}),content:'private-json',status:'pending',exercise:{documentId:'ex',version:'v1',changes:[{id:'old',before,after:{...before,id:'new',question:'新题'},status:'pending'}]}};
 const decisions=[];let draftWrites=0;
 globalThis.draftRejectionCall=async(command,args)=>{
  if(command==='draft_get')throw Error('其他题草稿尚未恢复');
  if(command==='draft_save'){draftWrites++;throw Error('草稿不能覆盖');}
  if(command==='edit_proposals')return[{id:'p',stamp:'1'}];
  if(command==='edit_proposal')return proposal;
  if(command==='decide_exercise_proposal'){decisions.push(args);return{persisted:false,status:'rejected',complete:true};}
  throw Error(command);
 };
 try {
  await React.act(async()=>{root.render(React.createElement(Harness,{courseId:'c',documentId:'ex',isDirty:()=>false,isLoaded:()=>true,onAccepted:async()=>{throw Error('拒绝不得刷新正文');},onOpen:async()=>{},notify:()=>{}}));await new Promise(r=>setTimeout(r,15));});
  const button=text=>[...document.querySelectorAll('button')].find(b=>b.textContent===text);
  await React.act(async()=>button('输入其他题草稿').click());
  await React.act(async()=>button('接受此题修改').click());
  assert.equal(decisions.length,0);assert.match(document.body.textContent,/草稿尚未读取成功/);
  await React.act(async()=>button('拒绝此题修改').click());
  assert.deepEqual(decisions,[{courseId:'c',id:'p',questionId:'old',accept:false}]);
  assert.equal(draftWrites,0);assert.equal(document.querySelector('output').textContent,'保留的未提交答案');
  assert.equal(document.querySelector('.review-dock'),null);assert.match(document.body.textContent,/Normal exercises/);
 } finally {await React.act(async()=>root.unmount());dom.window.close();delete globalThis.draftRejectionCall;await unlink(filename);}
});
