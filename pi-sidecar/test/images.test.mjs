import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/utils/event-stream';
import { createModel, runLearningAgent } from '../harness.mjs';
import { openNativeSession } from '../native-session.mjs';
import { validateImages, imageCompatibleStream } from '../image-input.mjs';

const image = { type: 'image', mimeType: 'image/png', data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAEElEQVR4AQEFAPr/AP8AAP8FAAH/+lyI0QAAAABJRU5ErkJggg==' };
const settings = { provider: 'custom', protocol: 'openai-completions', baseUrl: 'http://127.0.0.1:1/v1', model: 'fixture-vision', supportsImages: true };
test('desktop bundle includes every local module imported by the sidecar', () => {
  const resources = JSON.parse(fs.readFileSync(new URL('../../src-tauri/tauri.conf.json', import.meta.url), 'utf8')).bundle.resources;
  const directory = fileURLToPath(new URL('../', import.meta.url));
  for (const file of fs.readdirSync(directory).filter(name => name.endsWith('.mjs'))) {
    const source = fs.readFileSync(path.join(directory, file), 'utf8');
    for (const match of source.matchAll(/(?:from\s*|import\s*\()['"]\.\/([^'"]+\.mjs)['"]/gu)) {
      assert.equal(resources[`../pi-sidecar/${match[1]}`], `pi-sidecar/${match[1]}`, `${file} requires bundled ${match[1]}`);
    }
  }
});
const countImages = context => context.messages.flatMap(m => Array.isArray(m.content) ? m.content : []).filter(p => p.type === 'image').length;
function spec() {
  const directory = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'ls-images-')));
  fs.writeFileSync(path.join(directory, 'binding.json'), JSON.stringify({ version: 1, learningSessionId: 'images', transcript: 'transcript.jsonl', initialized: false }));
  return { directory, learningSessionId: 'images', runId: 'run_1', userEntryId: 'user_1', authorizedSourceIds: ['image_asset'], protectHistory: false, legacyRecords: [] };
}
function options(inspect, visual = true) {
  return { model: createModel({ ...settings, supportsImages: visual }), systemPrompt: 'Fixture', tools: [], apiKey: 'synthetic-image-test', thinkingLevel: 'off', streamFn(model, context) {
    inspect?.(context);
    const stream = createAssistantMessageEventStream();
    queueMicrotask(() => {
      const message = { role: 'assistant', content: [{ type: 'text', text: 'Image fixture response' }], api: model.api, provider: model.provider, model: model.id, usage: { input: 10, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 15, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } }, stopReason: 'stop', timestamp: Date.now() };
      stream.push({ type: 'done', reason: 'stop', message }); stream.end();
    });
    return stream;
  } };
}

test('image capabilities use explicit override or exact unanimous Pi catalog IDs', () => {
  assert.deepEqual(createModel(settings).input, ['text', 'image']);
  assert.deepEqual(createModel({ ...settings, supportsImages: false }).input, ['text']);
  assert.deepEqual(createModel({ ...settings, model: 'unlisted-local', supportsImages: undefined }).input, ['text']);
  assert.deepEqual(createModel({ ...settings, model: 'gpt-4o', supportsImages: undefined }).input, ['text', 'image']);
  assert.deepEqual(createModel({ ...settings, model: 'gpt-4o', supportsImages: null }).input, ['text', 'image']);
  assert.throws(() => createModel({ ...settings, supportsImages: 'yes' }), { code: 'INVALID_MODEL_SETTINGS' });
});

test('image IPC rejects malformed, unsupported and oversized payloads', () => {
  assert.deepEqual(validateImages([image]), [image]);
  for (const value of [null, [null], [ { ...image, mimeType: 'image/svg+xml' } ], [{ ...image, data: 'not-base64' }], Array(5).fill(image), [{ ...image, data: Buffer.alloc(5 * 1024 * 1024 + 1).toString('base64') }]]) {
    assert.throws(() => validateImages(value), { code: 'INVALID_INPUT' });
  }
  assert.equal(validateImages([{ ...image, data: Buffer.alloc(5 * 1024 * 1024).toString('base64') }]).length, 1);
  for(const sourceId of ['asset_../escape', 'asset_'+'A'.repeat(32), null, 3])assert.throws(()=>validateImages([{...image,sourceId}]),{code:'INVALID_INPUT'});
});

test('text-only stream strips user/tool/custom history image blocks without changing persisted input', () => {
  const context = { messages: ['user', 'toolResult', 'custom'].map(role => ({ role, content: [{ type: 'text', text: 'retained' }, image] })) };
  let observed;
  imageCompatibleStream((_, value) => { observed = value; })(createModel({ ...settings, supportsImages: false }), context, {});
  assert.equal(countImages(observed), 0); assert.equal(countImages(context), 3);
  assert.equal(observed.messages.every(m => m.content[0].text === 'retained'), true);
});

test('native images survive exact reopening, retry once, and stay out of text-only model requests', async () => {
  const s = spec(); let native; let observed;
  try {
    native = await openNativeSession(s, options(c => { observed = c; }));
    await native.prompt('Describe this', [image]);
    assert.equal(countImages(observed), 1); native.close();
    native = await openNativeSession({ ...s, runId: 'retry' }, options(c => { observed = c; }));
    await native.prompt('Describe this', [image]);
    assert.equal(countImages(observed), 1);
    assert.equal(native.manager.getEntries().filter(e => e.type === 'message' && e.message.role === 'user').length, 1);
    await assert.rejects(native.prompt('Describe this', [{ ...image, data: 'AAAA' }]), { code: 'NATIVE_SESSION_INVALID' });
    native.close();
    native = await openNativeSession({ ...s, runId: 'text_turn', userEntryId: 'text_user' }, options(c => { observed = c; }, false));
    await native.prompt('Continue in text', [image]);
    assert.equal(countImages(observed), 0); native.close();
    native = await openNativeSession({ ...s, runId: 'vision_return', userEntryId: 'vision_user' }, options(c => { observed = c; }));
    await native.prompt('Continue with vision'); assert.equal(countImages(observed), 1);
  } finally { native?.close(); fs.rmSync(s.directory, { recursive: true, force: true }); }
});

test('real Pi OpenAI request contains image_url for vision and omits it for text models', async () => {
  const received = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    received.push(JSON.parse(raw));
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.end('data: '+JSON.stringify({id:'image-fixture',object:'chat.completion.chunk',choices:[{index:0,delta:{role:'assistant',content:'OK'},finish_reason:null}]})+'\n\ndata: '+JSON.stringify({id:'image-fixture',object:'chat.completion.chunk',choices:[{index:0,delta:{},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    for (const supportsImages of [true, false]) {
      await runLearningAgent({ protocol: 1, operation: 'generate', settings: { ...settings, supportsImages, baseUrl: `http://127.0.0.1:${server.address().port}/v1` }, apiKey: 'synthetic-image-test', images: [image], messages: [{ role: 'user', content: 'Describe this picture' }] }, async () => {});
    }
    const blocks = received[0].messages.flatMap(m => Array.isArray(m.content) ? m.content : []);
    assert(blocks.some(p => p.type === 'image_url' && p.image_url.url === `data:${image.mimeType};base64,${image.data}`));
    assert(!JSON.stringify(received[1]).includes(image.data));
    assert(!JSON.stringify(received[1]).includes('image_url'));
  } finally { await new Promise(resolve => server.close(resolve)); }
});

test('switching a failed text-only turn to vision restores normalized images only once', async () => {
  const s=spec(); let native, observed;
  try {
    native=await openNativeSession(s,options(undefined,false));
    await native.prompt('Original frozen picture',[image]); native.close();
    for(const runId of ['vision_retry_1','vision_retry_2']){
      native=await openNativeSession({...s,runId},options(c=>{observed=c;}));
      await native.prompt('Original frozen picture',[image]);
      assert.equal(countImages(observed),1);
      assert.equal(native.manager.getEntries().filter(e=>e.type==='message'&&e.message.role==='user').length,1);
      native.close();
    }
  } finally {native?.close();fs.rmSync(s.directory,{recursive:true,force:true});}
});

test('native user text preserves image source order and hashes source identity without extending Pi image blocks',async()=>{
  const s=spec();let native,observed;
  const images=[{...image,sourceId:'asset_'+'b'.repeat(32)},{...image,sourceId:'asset_'+'a'.repeat(32)}];
  try{
    native=await openNativeSession(s,options(c=>{observed=c;}));await native.prompt('Compare these',images);
    const user=observed.messages.find(m=>m.role==='user');
    const text=user.content.filter(p=>p.type==='text').map(p=>p.text).join('');
    assert(text.endsWith(`图片顺序与来源：\n1: asset:${images[0].sourceId}\n2: asset:${images[1].sourceId}`));
    assert.equal(user.content.filter(p=>p.type==='image').length,2);
    assert(user.content.filter(p=>p.type==='image').every(p=>!('sourceId' in p)));
    native.close();native=await openNativeSession({...s,runId:'source_retry'},options());
    await native.prompt('Compare these',images);
    await assert.rejects(native.prompt('Compare these',[images[1],images[0]]),{code:'NATIVE_SESSION_INVALID'});
  }finally{native?.close();fs.rmSync(s.directory,{recursive:true,force:true});}
});

test('image preflight budgets Pi normalization expansion and a 4 MiB reserve before transcript mutation',async()=>{
  for(const [paddingMiB,retry,rejected,count] of [[23,false,false,1],[24,false,true,1],[24,true,true,1],[11,false,true,4]]){
    const s=spec();let native,calls=0;
    const attachments=Array(count).fill(image);
    try{
      native=await openNativeSession(s,options(()=>{calls++;},!retry));
      if(retry)await native.prompt('Picture',attachments);
      native.manager.appendCustomEntry('fixture-padding',{value:'x'.repeat(paddingMiB*1024*1024)});
      if(retry){native.close();native=await openNativeSession({...s,runId:'capacity_retry'},options(()=>{calls++;}));}
      calls=0;const file=path.join(s.directory,'transcript.jsonl'),before=fs.statSync(file).size;
      if(rejected){
        await assert.rejects(native.prompt('Picture',attachments),{code:'IMAGE_LIMIT'});
        assert.equal(calls,0);assert.equal(fs.statSync(file).size,before);
      }else{await native.prompt('Picture',attachments);assert.equal(calls,1);}
      assert(fs.statSync(file).size<32*1024*1024);
      native.close();
      if(rejected)await assert.rejects(runLearningAgent({protocol:1,operation:'generate',nativeSession:{...s,runId:'wire_capacity'},settings,apiKey:'synthetic-image-test',images:attachments,messages:[{role:'user',content:JSON.stringify({question:'Picture',nativeMaterials:[],nativeReferences:''})}]},async()=>{}, {requestTool:async()=>({result:{synced:true}})}),{code:'IMAGE_LIMIT'});
      native=await openNativeSession({...s,runId:'verify_reopen',userEntryId:'verify_reopen'},options());
    }finally{native?.close();fs.rmSync(s.directory,{recursive:true,force:true});}
  }
});

test('permission-mode native Pi sends OpenAI and Anthropic images without addon or worker grants', async () => {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const requests = [];
  const server = createServer(async (req, res) => {
    let raw = ''; for await (const chunk of req) raw += chunk;
    requests.push(JSON.parse(raw)); res.writeHead(200, { 'content-type': 'text/event-stream' });
    if (req.url.startsWith('/v1/messages')) {
      const events = [
        {type:'message_start',message:{id:'fixture',type:'message',role:'assistant',model:'fixture-vision',content:[],stop_reason:null,stop_sequence:null,usage:{input_tokens:1,output_tokens:0}}},
        {type:'content_block_start',index:0,content_block:{type:'text',text:''}},
        {type:'content_block_delta',index:0,delta:{type:'text_delta',text:'OK'}},
        {type:'content_block_stop',index:0},
        {type:'message_delta',delta:{stop_reason:'end_turn',stop_sequence:null},usage:{output_tokens:1}},
        {type:'message_stop'},
      ];
      res.end(events.map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(''));
    } else res.end('data: '+JSON.stringify({id:'fixture',choices:[{index:0,delta:{role:'assistant',content:'OK'},finish_reason:null}]})+'\n\ndata: '+JSON.stringify({id:'fixture',choices:[{index:0,delta:{},finish_reason:'stop'}]})+'\n\ndata: [DONE]\n\n');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  try {
    for (const protocol of ['openai-completions', 'anthropic-messages']) for (const supportsImages of [true, false]) {
      const s = spec();
      try {
        // A valid PNG with inert trailing bytes proves the old 4 MiB stdin cap is gone.
        const large = { ...image, data: Buffer.concat([Buffer.from(image.data, 'base64'), Buffer.alloc(3 * 1024 * 1024)]).toString('base64') };
        const input = { protocol:1, operation:'generate', nativeSession:s, settings:{...settings,protocol,supportsImages,baseUrl:`http://127.0.0.1:${server.address().port}${protocol==='openai-completions'?'/v1':''}`}, apiKey:'synthetic-permission-image', images:[protocol==='openai-completions'?large:image], messages:[{role:'user',content:JSON.stringify({question:'Describe the picture',nativeMaterials:[],nativeReferences:''})}] };
        const child = spawn(process.execPath, ['--permission',`--allow-fs-read=${root}`,`--allow-fs-read=${s.directory}`,`--allow-fs-write=${s.directory}`,path.join(root,'sidecar.mjs')], {cwd:root,stdio:['pipe','pipe','pipe'],windowsHide:true});
        let pending='', stderr=''; const events=[];
        child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8');
        child.stdout.on('data',chunk=>{
          pending+=chunk; let end;
          while((end=pending.indexOf('\n'))>=0){
            const event=JSON.parse(pending.slice(0,end)); pending=pending.slice(end+1); events.push(event);
            if(event.type==='session_sync') child.stdin.write(JSON.stringify({type:'tool_result',id:event.id,result:{synced:true}})+'\n');
          }
        });
        child.stderr.on('data',chunk=>{stderr+=chunk;});
        const ended=new Promise((resolve,reject)=>{child.on('error',reject);child.on('exit',resolve);});
        const timer=setTimeout(()=>child.kill(),12000);
        child.stdin.write(JSON.stringify(input)+'\n');
        const code=await ended;clearTimeout(timer);
        assert.equal(code,0,JSON.stringify({events,stderr})); assert.equal(events.at(-1).type,'done');
        const blocks=requests.at(-1).messages.flatMap(m=>Array.isArray(m.content)?m.content:[]);
        assert.equal(blocks.some(b=>b.type===(protocol==='openai-completions'?'image_url':'image')),supportsImages);
        assert(!events.some(e=>e.type==='error'));
      } finally { fs.rmSync(s.directory,{recursive:true,force:true}); }
    }
  } finally { server.closeAllConnections(); await new Promise(resolve=>server.close(resolve)); }
});
