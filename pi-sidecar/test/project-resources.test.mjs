import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {syncBuiltinESMExports} from 'node:module';

const self=fileURLToPath(import.meta.url);
if(process.argv[2]==='fixture'){
  const root=process.argv[4],scenario=process.argv[3];
  // Model an empty filesystem outside the synthetic tree without reading any
  // real ancestor instructions/user skills. Permission mode also enforces it.
  const component=path.dirname(path.dirname(self)),exists=fs.existsSync;
  const contextNames=new Set(['AGENTS.override.md','AGENTS.md','AGENTS.MD','CLAUDE.md','CLAUDE.MD']);
  fs.existsSync=file=>{
    const name=path.resolve(String(file));
    if(name===root||name.startsWith(root+path.sep)||name===component||name.startsWith(component+path.sep))return exists(file);
    // Mask only the real ancestor instruction candidates that native discovery
    // would inspect; every other unexpected path still meets Node's permission check.
    const ancestor=path.dirname(name);
    if(contextNames.has(path.basename(name))&&(ancestor===path.parse(root).root||root.startsWith(ancestor+path.sep)))return false;
    return exists(file);
  };
  os.homedir=()=>path.join(root,'synthetic-user-home');
  syncBuiltinESMExports();
  const {createHash}=await import('node:crypto');
  const {createAssistantMessageEventStream}=await import('@earendil-works/pi-ai/utils/event-stream');
  const {openNativeSession}=await import('../native-session.mjs');
  const sha=s=>createHash('sha256').update(s).digest('hex');
  const put=(relative,text)=>{const target=path.join(root,relative);fs.mkdirSync(path.dirname(target),{recursive:true});fs.writeFileSync(target,text);};
  const skill=(name,body)=>`---\nname: ${name}\ndescription: Synthetic project skill\n---\n${body}`;
  put('AGENTS.md','PARENT_STUDENT_PROFILE');
  for(const name of ['a','b']){
    fs.mkdirSync(path.join(root,'courses',name,'.git'),{recursive:true});
    put(`courses/${name}/AGENTS.md`,`COURSE_${name.toUpperCase()}_GUIDANCE`);
    put(`courses/${name}/materials/note.md`,`PRIVATE_${name.toUpperCase()}_NOTE_BODY`);
    put(`courses/${name}/.pi/skills/course-${name}/SKILL.md`,skill(`course-${name}`,`SKILL_${name.toUpperCase()}_BODY`));
  }
  put('user/AGENTS.md','USER_ALLOWED_GUIDANCE');
  put('user/skills/user-guide/SKILL.md',skill('user-guide','USER_SKILL_BODY'));
  put('user/auth.json','NOT_A_CREDENTIAL_SOURCE');
  const projectRoot=path.join(root,'courses','a'),agentDir=path.join(root,'user');
  const makeSpec=id=>{
    const directory=path.join(root,'sessions',id);fs.mkdirSync(directory,{recursive:true});
    fs.writeFileSync(path.join(directory,'binding.json'),JSON.stringify({version:1,learningSessionId:id,transcript:'transcript.jsonl',initialized:false}));
    return {directory,projectRoot,agentDir,learningSessionId:id,runId:'run_first',userEntryId:'user_first',authorizedSourceIds:[],protectHistory:false,legacyRecords:[]};
  };
  const model={provider:'learning-custom',id:'fixture',name:'fixture',api:'openai-completions',baseUrl:'http://127.0.0.1:1/v1',reasoning:false,input:['text'],contextWindow:0,maxTokens:1024,cost:{input:0,output:0,cacheRead:0,cacheWrite:0}};
  let observed;
  const streamFn=(m,c)=>{
    observed=c;const stream=createAssistantMessageEventStream();
    queueMicrotask(()=>{const message={role:'assistant',content:[{type:'text',text:'Synthetic reply'}],api:m.api,provider:m.provider,model:m.id,usage:{input:5,output:3,cacheRead:0,cacheWrite:0,totalTokens:8,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:Date.now()};stream.push({type:'start',partial:message});stream.push({type:'done',reason:'stop',message});stream.end();});return stream;
  };
  // Resource tests stub the separate Rust session_sync receipt; no durability claim.
  const options={model,systemPrompt:'Application rules stay explicit.',tools:[],streamFn,apiKey:'synthetic-secret',thinkingLevel:'off',checkpoint:async()=>{}};
  const envelope={nativeMaterials:[],nativeReferences:'',context:'PRIVATE_LEGACY_CONTEXT_BODY',contextBridgeManifest:{version:1,entries:[]}};
  const open=spec=>openNativeSession(spec,options);
  if(scenario==='discovery'){
    const s=makeSpec('session_a'),native=await open(s);
    try{
      assert.throws(()=>fs.readFileSync(path.join(root,'courses/b/materials/note.md')),e=>e.code==='ERR_ACCESS_DENIED');
      assert.throws(()=>fs.readFileSync(path.join(agentDir,'auth.json')),e=>e.code==='ERR_ACCESS_DENIED');
      // Node permissions follow existing symlinks. Keep this limitation explicit;
      // the adapter must keep native read/Bash inactive and retain Rust tool scope.
      assert.equal(fs.readFileSync(path.join(projectRoot,'sibling-link.md'),'utf8'),'PRIVATE_B_NOTE_BODY');
      assert.equal(native.manager.getCwd(),projectRoot);
      const loader=native.session.resourceLoader;
      const files=loader.getAgentsFiles().agentsFiles;
      assert(files.some(f=>f.content==='PARENT_STUDENT_PROFILE'));
      assert(files.some(f=>f.content==='COURSE_A_GUIDANCE'));
      assert(files.some(f=>f.content==='USER_ALLOWED_GUIDANCE'));
      assert(!files.some(f=>f.content.includes('COURSE_B')));
      assert.deepEqual(loader.getSkills().skills.map(s=>s.name).sort(),['course-a','user-guide']);
      assert.equal(loader.getExtensions().extensions.length,0);
      await native.applyContext(envelope);await native.prompt('Ordinary question');
      const wire=JSON.stringify(observed);
      for(const marker of ['PARENT_STUDENT_PROFILE','COURSE_A_GUIDANCE','USER_ALLOWED_GUIDANCE'])assert(wire.includes(marker));
      for(const marker of ['PRIVATE_A_NOTE_BODY','PRIVATE_B_NOTE_BODY','PRIVATE_LEGACY_CONTEXT_BODY','COURSE_B_GUIDANCE','SKILL_A_BODY','NOT_A_CREDENTIAL_SOURCE'])assert(!wire.includes(marker),marker);
      assert.equal(observed.tools?.length??0,0);
    }finally{native.close();}
  }else if(scenario==='skill-retry'){
    const s=makeSpec('session_skill'),question='/skill:course-a Explain the topic';
    let native=await open(s);
    try{await native.applyContext(envelope);await native.prompt(question);}finally{native.close();}
    assert(JSON.stringify(observed).includes('SKILL_A_BODY'));
    put('courses/a/.pi/skills/course-a/SKILL.md',skill('course-a','SKILL_A_CHANGED_AFTER_ACCEPTANCE'));
    native=await open({...s,runId:'run_retry'});
    try{await native.applyContext(envelope);await native.prompt(question);}finally{native.close();}
    assert(JSON.stringify(observed).includes('SKILL_A_BODY'));
    assert(!JSON.stringify(observed).includes('SKILL_A_CHANGED_AFTER_ACCEPTANCE'));
    const entries=fs.readFileSync(path.join(s.directory,'transcript.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(entries.filter(e=>e.type==='message'&&e.message.role==='user').length,1);
    assert(entries.some(e=>e.type==='custom'&&e.customType==='learning-station-user'&&e.data.inputHash===sha(question)));
  }else if(scenario==='sessions'){
    const first=makeSpec('session_one'),second=makeSpec('session_two');
    let native=await open(first);
    try{await native.applyContext(envelope);await native.prompt('ONLY_FIRST_SESSION_QUESTION');}finally{native.close();}
    native=await open(second);
    try{await native.applyContext(envelope);await native.prompt('Second session');assert.equal(native.manager.getCwd(),projectRoot);}finally{native.close();}
    assert(!JSON.stringify(observed).includes('ONLY_FIRST_SESSION_QUESTION'));
    assert.notEqual(fs.readFileSync(path.join(first.directory,'transcript.jsonl'),'utf8'),fs.readFileSync(path.join(second.directory,'transcript.jsonl'),'utf8'));
  }else if(scenario==='citation'){
    const native=await open(makeSpec('session_citation'));
    try{await native.applyContext({...envelope,nativeReferences:'EXPLICIT_USER_QUOTE'});await native.prompt('Explain my quotation');assert(JSON.stringify(observed).includes('EXPLICIT_USER_QUOTE'));assert(!JSON.stringify(observed).includes('PRIVATE_A_NOTE_BODY'));}finally{native.close();}
  }else if(scenario==='focus-baseline'){
    const s={...makeSpec('session_focus'),authorizedSourceIds:['note_a']};
    const body='FOCUS_BODY_V1',full={kind:'full',version:'v1',textHash:sha(body),text:body};
    const source={id:'note_a',version:'v1',textHash:sha(body),text:body,range:{startChar:0,endChar:body.length,complete:true,totalBytes:Buffer.byteLength(body)},frames:[full]};
    const focused={...envelope,nativeMaterials:[source],contextBridgeManifest:{version:1,entries:[{id:'material:note_a',kind:'material',sourceId:'note_a'}]}};
    let native=await open(s);
    try{await native.applyContext(focused);await native.prompt('First focus');assert(JSON.stringify(observed).includes(body));}finally{native.close();}
    native=await open({...s,runId:'run_unchanged',userEntryId:'user_unchanged'});
    try{
      await native.applyContext(focused);
      assert.equal(native.manager.getEntries().filter(e=>e.type==='custom_message'&&e.customType==='learning-station-material').length,1);
      const next='FOCUS_BODY_V2',diff={kind:'diff',version:'v2',baseHash:sha(body),textHash:sha(next),unifiedDiff:'@@ -1 +1 @@\n-FOCUS_BODY_V1\n+FOCUS_BODY_V2\n'};
      const changed={...focused,nativeMaterials:[{...source,version:'v2',text:next,textHash:sha(next),frames:[full,diff]}]};
      await native.applyContext(changed);
      const entry=native.manager.getEntries().filter(e=>e.type==='custom_message'&&e.customType==='learning-station-material').at(-1);
      assert.equal(JSON.parse(entry.content).frames[0].kind,'diff');
      native.manager.appendCompaction('Summary is not a full text baseline',entry.id,1000);
      await native.applyContext(changed);
      assert.equal(JSON.parse(native.manager.getEntries().filter(e=>e.type==='custom_message'&&e.customType==='learning-station-material').at(-1).content).frames[0].kind,'full');
    }finally{native.close();}
    native=await open({...makeSpec('session_focus_new'),authorizedSourceIds:['note_a']});
    try{await native.applyContext(focused);assert.equal(JSON.parse(native.manager.getEntries().find(e=>e.type==='custom_message'&&e.customType==='learning-station-material').content).frames[0].kind,'full');}finally{native.close();}
  }else if(scenario==='changed-input'){
    const s=makeSpec('session_identity'),question='/skill:course-a Original input';
    let native=await open(s);
    try{await native.applyContext(envelope);await native.prompt(question);await native.complete('Synthetic reply');}finally{native.close();}
    native=await open(s);
    try{await assert.rejects(native.prompt('/skill:course-a Changed input'),e=>e.code==='NATIVE_SESSION_INVALID');assert.deepEqual(await native.prompt(question),{cached:'Synthetic reply'});}finally{native.close();}
  }else throw new Error('Unknown scenario');
}else{
  function run(scenario){
    const root=fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(),'ls-project-test-')));
    try{
      // The fixture process can only see installed components and its synthetic tree;
      // native discovery cannot read the real user's ~/.agents or global settings.
      const contextNames=['AGENTS.override.md','AGENTS.md','AGENTS.MD','CLAUDE.md','CLAUDE.MD'];
      const readPaths=[path.dirname(path.dirname(self)),path.join(root,'courses/a'),path.join(root,'sessions'),path.join(root,'synthetic-user-home/.agents/skills'),...contextNames.flatMap(name=>[path.join(root,name),path.join(root,'courses',name),path.join(root,'user',name)]),...['SYSTEM.md','APPEND_SYSTEM.md','skills','prompts','themes','extensions'].map(name=>path.join(root,'user',name))];
      for(const relative of ['courses/a','sessions','synthetic-user-home/.agents/skills',...['skills','prompts','themes','extensions'].map(name=>'user/'+name)])fs.mkdirSync(path.join(root,relative),{recursive:true});
      fs.symlinkSync(path.join(root,'courses/b/materials/note.md'),path.join(root,'courses/a/sibling-link.md'));
      const result=spawnSync(process.execPath,['--permission',...readPaths.map(p=>`--allow-fs-read=${p}`),`--allow-fs-write=${root}`,self,'fixture',scenario,root],{encoding:'utf8',env:{PATH:process.env.PATH,LANG:'en_US.UTF-8'},maxBuffer:2*1024*1024});
      assert.equal(result.status,0,result.stderr||result.stdout||String(result.error));
    }finally{fs.rmSync(root,{recursive:true,force:true});}
  }
  test('native course discovery inherits parent/user AGENTS, excludes sibling resources and injects no note body',()=>run('discovery'));
  test('native skill expansion survives a retry without rereading changed skill body or duplicating the saved user',()=>run('skill-retry'));
  test('two sessions share a course cwd while preserving independent native transcripts',()=>run('sessions'));
  test('explicit quotations remain supplied when ordinary note bodies are absent',()=>run('citation'));
  test('focused project text retains its session baseline, uses diff only while full survives and reloads after compaction',()=>run('focus-baseline'));
  test('completed native skill request cannot be replayed with changed original input',()=>run('changed-input'));
}
