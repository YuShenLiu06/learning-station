import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('native unknown-run retry never sends after IO or corrupt state, and only NOT_FOUND permits same-operation start', async () => {
  const dom=new JSDOM('',{url:'http://127.0.0.1/'});globalThis.window=dom.window;
  const {mockIPC,clearMocks}=await import('@tauri-apps/api/mocks');
  const session={id:'retry_session',sectionId:'retry_section',title:'fixture',version:'v1',entries:[],contextManifest:[{id:'unrelated_later_session',path:'later.md',version:'later',includedChars:1}],contextHistoryManifest:[{id:'history_entry',path:'learning-history/retry_session/history_entry',version:'history_hash',includedChars:42}],contextWarnings:['TRUNCATED_HISTORY'],contextTruncated:true};
  let failureCode='IO';let started=false;const starts=[];const preferences=[];
  mockIPC(async(command,args={})=>{
    if(command==='get_session')return structuredClone(session);
    if(command==='get_run'){if(!started && failureCode==='RECOVERY')return {id:args.runId,sessionId:session.id,status:'failed',content:'saved output',error:{code:'IO',message:'saved entry requires document',details:{recovery:'same-operation'}}};if(!started)throw {code:failureCode,message:'Controlled read failure'};return {id:args.runId,sessionId:session.id,status:'succeeded',content:'',toolCalls:[],contextSnapshotVersion:failureCode==='OLD'?0:1,contextManifest:[],historyManifest:[{id:'original_run_history',path:'learning-history/original',version:'original_hash',includedChars:84}],contextWarnings:['TRUNCATED_HISTORY'],contextTruncated:true};}
    if(command==='start_generation'){starts.push(args);started=true;return {};}
    if(command==='save_ui_preferences'){preferences.push(args);return {fontSize:16,assistantWidth:450,...args};}
    throw new Error('Unexpected fake IPC: '+command);
  });
  const filename=new URL(`./.native-retry-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/native.ts'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm'});
  try{
    const {nativeCall,onGenerationProgress}=await import(filename.href);const progress=[];const unsubscribe=onGenerationProgress(value=>progress.push(value));
    const payload={sectionId:session.sectionId,sessionId:session.id,question:'already saved',selection:'',mode:'explore',userEntryId:'entry_saved',operationId:'same_operation',resume:true};
    for(const code of ['IO','INVALID_JSON','INVALID_STATE']){
      failureCode=code;
      await assert.rejects(()=>nativeCall('session_ask',payload),error=>error.code==='STATUS_UNKNOWN'&&error.details.sendStage==='unknown');
      assert.equal(starts.length,0);
    }
    failureCode='NOT_FOUND';await nativeCall('session_ask',payload);
    assert.equal(starts.length,1);assert.equal(starts[0].requestId,'same_operation');assert.equal(starts[0].userEntryId,'entry_saved');
    const actual=progress.find(value=>value.context)?.context;assert.equal(actual.truncated,true);assert.deepEqual(actual.warnings,['TRUNCATED_HISTORY']);assert.equal(actual.historyManifest[0].includedChars,84);assert.deepEqual(actual.manifest,[]);assert.equal(actual.historyManifest[0].id,'original_run_history');
    failureCode='RECOVERY';started=false;await nativeCall('session_ask',payload);assert.equal(starts.length,2);assert.equal(starts[1].requestId,'same_operation');failureCode='OLD';await nativeCall('session_ask',payload);assert.match(progress.at(-1).contextError,/未记录实际来源快照/);assert.equal(progress.at(-1).context,undefined);unsubscribe();
    await nativeCall('preferences_save',{assistantWidth:450});await nativeCall('preferences_save',{fontSize:20});
    assert.deepEqual(preferences,[{assistantWidth:450},{fontSize:20}]);
  }finally{clearMocks();dom.window.close();await unlink(filename).catch(()=>{});}
});
