import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createServer} from 'node:http';
import {SessionManager} from '@earendil-works/pi-coding-agent';
import {runLearningAgent} from '../harness.mjs';
import {LEARNING_PLUGIN} from '../learning-plugin.mjs';
const hash=v=>createHash('sha256').update(v).digest('hex');
const text=m=>typeof m.content==='string'?m.content:(m.content??[]).filter(p=>p.type==='text').map(p=>p.text).join('');
const part=(key,value)=>({key,description:'合成冻结输入：'+key,value});
const position='【Focus：用户当前阅读的位置】',changesTitle='【Changes：课程材料相对已知版本的变化】';
test('actual SDK HTTP follows the readable Focus/Changes contract with native Git text across turns',{timeout:60000},async()=>{
  const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'readable-context-'))),repo=path.join(root,'course'),directory=path.join(root,'native');
  fs.mkdirSync(path.join(repo,'materials/documents'),{recursive:true});fs.mkdirSync(directory);
  fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:'readable',transcript:'transcript.jsonl',initialized:false}));
  const git=(args,input)=>execFileSync('git',['-C',repo,'-c','user.name=Fixture','-c','user.email=fixture@localhost','-c','commit.gpgSign=false','-c','core.hooksPath=/dev/null',...args],{input,encoding:'utf8'});
  git(['init','--template=','.']);
  const file='materials/documents/focus.md',other='materials/documents/other.md';
  const save=(p,v)=>fs.writeFileSync(path.join(repo,p),v);
  const commit=()=>{git(['add','materials']);git(['commit','-qm','Synthetic fixture']);return git(['rev-parse','HEAD']).trim();};
  save(file,'第一行😀\n第二行\n第三行\n');save(other,'OTHER_BODY_MUST_NOT_SEND\n');let current=commit(),round=0,paused;
  const packets=[],snapshots=[],gitTexts=[];const empty=git(['hash-object','-t','tree','--stdin'],'').trim();
  const server=createServer(async(req,res)=>{
    let raw='';for await(const chunk of req)raw+=chunk;const packet=JSON.parse(raw);packets.push(packet);
    const tools=round===6&&!packet.messages.some(m=>m.tool_calls?.some(c=>c.id==='read_6'));
    const delta=tools?{tool_calls:[{index:0,id:'read_6',type:'function',function:{name:'read',arguments:JSON.stringify({path:'not-present.md'})}},{index:1,id:'skills_6',type:'function',function:{name:'list_learning_skills',arguments:'{}'}}]}:{content:'答复 '+round};
    res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: '+JSON.stringify({choices:[{index:0,delta,finish_reason:tools?'tool_calls':'stop'}]})+'\n\ndata: [DONE]\n\n');
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const run=async(i,runId='run_'+i)=>{
    const fixed=current,body=git(['show',fixed+':'+file]),viewStart=i===3?3:0,selection=i===4?[{startChar:1,endChar:3}]:[];
    const focus={courseId:'course',documentId:'INTERNAL_SOURCE_ID',sourceDocumentId:'INTERNAL_SOURCE_ID',path:'documents/focus.md',type:'markdown',documentVersion:hash(body),commitHash:fixed,materialTreeOid:git(['rev-parse',fixed+':materials']).trim(),repositoryVersion:2,view:'read',paragraphs:[{startChar:viewStart,endChar:6,start:viewStart,end:7,text:Array.from(body).slice(viewStart,6).join('')}],selectionRanges:selection};
    const frozen=[part('current_time',{referenceTime:'2026-10-21T12:00:0'+i+'Z',timeZone:'Asia/Shanghai'}),part('references',i===4?'用户明确引用的范围：[1, 3)（正文 Unicode 字符，从 0 开始，左闭右开）':''),part('focus',focus),part('question','相同问题')];
    const start=packets.length,statCalls=[];
    await runLearningAgent({protocol:1,operation:'generate',apiKey:'synthetic-readable-key',settings:{provider:'custom',protocol:'openai-completions',baseUrl:'http://127.0.0.1:'+server.address().port+'/v1',model:'fixture'},timeContext:frozen[0].value,learningTools:true,learningSkills:[],toolPolicy:{mode:'question',authorization:'read',allowedTools:[...LEARNING_PLUGIN.tools,'read'],maxCalls:6},materialChanges:true,nativeSession:{directory,learningSessionId:'readable',runId,userEntryId:'user_'+i,authorizedSourceIds:['INTERNAL_SOURCE_ID'],protectHistory:false,legacyRecords:[]},messages:[{role:'user',content:JSON.stringify({bridge:frozen,contextBridgeManifest:{version:1,entries:[]}})}]},()=>{},{requestTool:async call=>{
      if(call.type==='session_sync')return {result:{synced:true}};
      if(call.type==='material_changes'){
        if(paused){paused.entered.resolve();await paused.release.promise;}
        const stat=git(['diff','--no-ext-diff','--no-textconv','--no-color','--find-renames','--stat',call.baseCommitOid??empty,fixed,'--','materials']);
        const patch=call.focusBaseCommitOid?git(['diff','--no-ext-diff','--no-textconv','--no-color','--no-renames','--patch','--full-index',call.focusBaseCommitOid,fixed,'--',file]):'';
        const source={id:'INTERNAL_SOURCE_ID',path:'documents/focus.md',modelPath:file,version:hash(body),text:body,textHash:hash(body),range:{startChar:0,endChar:Array.from(body).length,totalBytes:Buffer.byteLength(body),complete:true},gitCommitOid:fixed,gitBlobOid:git(['rev-parse',fixed+':'+file]).trim(),...(patch?{gitChange:{baseCommitOid:call.focusBaseCommitOid,baseBlobOid:git(['rev-parse',call.focusBaseCommitOid+':'+file]).trim(),unifiedDiff:patch}}:{})};
        statCalls.push({base:call.baseCommitOid,stat,patch});
        return {result:{complete:true,checkpoint:{courseId:'course',checkpointCommitOid:fixed,materialTreeOid:focus.materialTreeOid,excludedFiles:[]},baseCommitOid:call.baseCommitOid,stat,focusPath:file,focused:source,changes:[],exclusionsChanged:call.baseCommitOid===null}};
      }
      if(call.name==='read')return {error:{code:'NOT_FOUND',message:'Synthetic missing file'}};
      if(call.name==='list_learning_skills')return {result:{skills:[]}};
      assert.fail('Unexpected synthetic tool '+call.type+'/'+call.name);
    }});
    const packet=packets[start],messages=packet.messages,at=messages.findLastIndex(m=>text(m)==='相同问题');
    const focusAt=messages.findLastIndex(m=>text(m).startsWith(position));
    // Tool schemas retain their existing API parameters; these assertions bound
    // application context, not the permitted native tool declarations.
    const currentTexts=messages.slice(focusAt,at).map(text),location=currentTexts[0],wire=JSON.stringify(messages);
    assert(location.includes('文件：'+file)&&location.includes('版本：'+fixed));
    assert(!location.includes('可见范围：'),'frozen viewport coordinates are not rendered');
    assert(!location.includes(body),'location reports metadata, not another full body');
    assert(!location.includes('用户选区：'));
    if(i===4)assert(!location.includes('[1, 3)'),'a frozen selection is not rendered as coordinates');
    for(const forbidden of ['INTERNAL_SOURCE_ID','documentId','sourceDocumentId','materialTreeOid','checkpointCommitOid','fixedVersion','textHash','materialRole','projectionVersion','learning_rules','完整正文或真实 Git 差量','OTHER_BODY_MUST_NOT_SEND'])assert(!wire.includes(forbidden),forbidden);
    assert(!wire.includes(hash(body)),'custom body hash stays internal');
    assert.equal(messages.filter(m=>m.role==='user'&&text(m)==='相同问题').length,i>=7?i-6:i+1,'repeated text is a distinct native question');
    const full=currentTexts.filter(v=>v.startsWith('以下为当前用户正在看的内容'));
    const changes=currentTexts.find(v=>v.startsWith(changesTitle));
    if(i===0||i===7){assert.equal(full.length,1);assert(full[0].startsWith('以下为当前用户正在看的内容（固定 Git 版本；'+(i===0?'首次正文基准':'正文基准已丢失，重新载入')+'）'));assert(full[0].endsWith(body));assert(changes.includes('首次建立课程材料基准'));assert.equal(statCalls[0].base,null);assert(!changes.includes('diff --git'));}
    else assert.equal(full.length,0,'valid historical body is retained without another full');
    if([1,3,4,8].includes(i))assert.equal(changes,undefined,'unchanged course/body adds no stat or patch');
    if([2,5,6].includes(i)){assert(changes.includes(statCalls[0].stat),'native stat bytes reach HTTP unchanged');if(i!==5){assert(changes.endsWith(statCalls[0].patch));assert(changes.indexOf(statCalls[0].stat)<changes.indexOf('当前聚焦文件的完整 Git diff'));assert(statCalls[0].stat.includes('focus.md'));}else assert(!changes.includes('diff --git'),'other file has only stat');}
    assert(currentTexts.every(v=>messages.indexOf(messages.find(m=>text(m)===v))<at));
    snapshots.push({round:i,runId,focusAt,questionAt:at,currentTexts});gitTexts.push(...statCalls);
    return {start,packet};
  };
  try{
    for(round=0;round<8;round++){
      if(round===2){save(file,'第一行😀\n编辑后的第二行\n第三行\n');current=commit();}
      if(round===5){save(other,'OTHER_BODY_MUST_NOT_SEND\n非聚焦新增\n');current=commit();}
      if(round===6){save(file,'第一行😀\n再改第二行\n第三行\n');save(other,'OTHER_BODY_MUST_NOT_SEND\n非聚焦再次新增\n');current=commit();}
      if(round===7){const m=SessionManager.open(path.join(directory,'transcript.jsonl'));m.appendCompaction('Synthetic summary does not preserve exact body',m.getEntries().filter(e=>e.type==='message').at(-1).id,100);}
      const result=await run(round);
      if(round===6){const continued=packets[result.start+1];assert.equal(continued.messages.at(-1).role,'tool');assert.deepEqual(continued.messages.slice(0,result.packet.messages.length),result.packet.messages,'tool continuation retains the frozen prefix');}
    }
    // The fixed commit, body, viewport and selection remain from before the pause.
    round=8;paused={entered:Promise.withResolvers(),release:Promise.withResolvers()};const before=packets.length,pending=run(8);
    await paused.entered.promise;save(file,'UNFROZEN_EDIT_MUST_NOT_SEND\n');current=commit();assert.equal(packets.length,before);
    paused.release.resolve();await pending;assert(!JSON.stringify(packets.at(-1)).includes('UNFROZEN_EDIT_MUST_NOT_SEND'));
    if(process.env.LS_CONTEXT_WIRE_EVIDENCE){fs.mkdirSync(process.env.LS_CONTEXT_WIRE_EVIDENCE,{recursive:true});fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'readable-wire.json'),JSON.stringify(packets,null,2));fs.writeFileSync(path.join(process.env.LS_CONTEXT_WIRE_EVIDENCE,'readable-contract-map.json'),JSON.stringify({snapshots,gitTexts},null,2));}
  }finally{paused?.release.resolve();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));fs.rmSync(root,{recursive:true,force:true});}
});
