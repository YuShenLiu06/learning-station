import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('native unknown-run retry never sends after IO or corrupt state, and only NOT_FOUND permits same-operation start', async () => {
  const dom=new JSDOM('',{url:'http://127.0.0.1/'});globalThis.window=dom.window;
  const {mockIPC,clearMocks}=await import('@tauri-apps/api/mocks');
  const session={id:'retry_session',sectionId:'retry_section',title:'fixture',version:'v1',entries:[],contextManifest:[]};
  let failureCode='IO';let started=false;const starts=[];const preferences=[];
  mockIPC(async(command,args={})=>{
    if(command==='get_session')return structuredClone(session);
    if(command==='get_run'){if(!started)throw {code:failureCode,message:'Controlled read failure'};return {id:args.runId,sessionId:session.id,status:'succeeded',content:'',toolCalls:[]};}
    if(command==='start_generation'){starts.push(args);started=true;return {};}
    if(command==='save_ui_preferences'){preferences.push(args);return {fontSize:16,assistantWidth:450,...args};}
    throw new Error('Unexpected fake IPC: '+command);
  });
  const filename=new URL(`./.native-retry-${process.pid}.mjs`,import.meta.url);
  await build({entryPoints:['src/native.ts'],outfile:filename.pathname,bundle:true,packages:'external',platform:'node',format:'esm'});
  try{
    const {nativeCall}=await import(filename.href);
    const payload={sectionId:session.sectionId,sessionId:session.id,question:'already saved',selection:'',mode:'explore',userEntryId:'entry_saved',operationId:'same_operation',resume:true};
    for(const code of ['IO','INVALID_JSON','INVALID_STATE']){
      failureCode=code;
      await assert.rejects(()=>nativeCall('session_ask',payload),error=>error.code==='STATUS_UNKNOWN'&&error.details.sendStage==='unknown');
      assert.equal(starts.length,0);
    }
    failureCode='NOT_FOUND';await nativeCall('session_ask',payload);
    assert.equal(starts.length,1);assert.equal(starts[0].requestId,'same_operation');assert.equal(starts[0].userEntryId,'entry_saved');
    await nativeCall('preferences_save',{assistantWidth:450});await nativeCall('preferences_save',{fontSize:20});
    assert.deepEqual(preferences,[{assistantWidth:450},{fontSize:20}]);
  }finally{clearMocks();dom.window.close();await unlink(filename).catch(()=>{});}
});
