import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MODEL_ID } from '../harness.mjs';
import { runLearningAgentWithSkills as runLearningAgent } from './skills-fixture.mjs';
import { createLearningPlugin, createStdioToolBridge, LEARNING_PLUGIN, LEGACY_TOOLS, MODE_POLICIES, MAX_TOOL_BYTES, validateCall, validateToolPolicy } from '../learning-plugin.mjs';
const SECRET = 'fixture-tools-key-never-real';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const request = (baseUrl) => ({ protocol: 1, operation: 'generate', learningTools: true, apiKey: SECRET, settings: { provider: 'glm-pi', baseUrl, model: MODEL_ID }, messages: [{ role: 'user', content: '{"type":"quoted_learning_material","mode":"practice","question":"请记录疑问，然后出一道练习。"}' }] });
const call = (id = 'question-1', name = 'record_question', args = { question: 'Why use since?' }) => ({ id, name, args });
async function payload(req) { let text = ''; for await (const bytes of req) text += bytes; return JSON.parse(text); }
function sse(res, { tool, text = '', final = false }) {
  res.writeHead(200, { 'content-type': 'text/event-stream' });
  const event = (delta, finish = null) => `data: ${JSON.stringify({ id: 'fixture', model: MODEL_ID, choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`;
  if (text) res.write(event({ role: 'assistant', content: text }));
  if (tool) res.write(event({ tool_calls: [{ index: 0, id: tool.id, type: 'function', function: { name: tool.name, arguments: JSON.stringify(tool.args) } }] }));
  res.end(event({}, tool ? 'tool_calls' : final ? 'stop' : null) + 'data: [DONE]\n\n');
}
async function withServer(handler, action) {
  const server = createServer(async (req, res) => {
    try { await handler(await payload(req), res); }
    catch { res.writeHead(500); res.end(); }
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  try { return await action(`http://127.0.0.1:${server.address().port}/v1`); }
  finally { server.closeAllConnections(); await new Promise((done) => server.close(done)); }
}
const toolMessages = (body) => body.messages.filter((message) => message.role === 'tool');
function assertToolSet(body) {
  assert.deepEqual(body.tools.map((tool) => tool.function.name).sort(), [...LEGACY_TOOLS].sort());
  assert.equal(body.tool_choice, 'auto'); assert.equal(body.tool_stream, true);
  assert.equal(body.thinking.type, 'enabled'); assert.equal(body.model, MODEL_ID);
  assert(!JSON.stringify(body).includes(SECRET));
}

test('real Pi provider tool loop returns Rust receipt to next turn and keeps final JSON separate', { timeout: 4000 }, async () => {
  let turns = 0; let executions = 0; const events = [];
  const final = '{"exercises":[{"question":"fixture"}],"summary":"fixture"}';
  await withServer((body, res) => {
    assertToolSet(body); turns++;
    if (turns === 1) sse(res, { tool: call(), text: '我先记录一下。' });
    else {
      const result = JSON.parse(toolMessages(body).at(-1).content);
      assert.equal(result.ok, true); assert.equal(result.result.persisted, true); assert.equal(result.result.id, 'fixture-real-receipt');
      sse(res, { text: final, final: true });
    }
  }, async (endpoint) => {
    const result = await runLearningAgent(request(endpoint), (event) => events.push(event), { requestTool: async (tool) => {
      executions++; assert.equal(tool.name, 'record_question'); return { result: { persisted: true, id: 'fixture-real-receipt' } };
    }, onAgent: (agent) => { assert.deepEqual(agent.state.tools.map((tool) => tool.name).sort(), [...LEGACY_TOOLS].sort()); } });
    assert.equal(result.output, final); assert(result.partialOutput.startsWith('我先记录一下。'));
    assert.equal(events.at(-1).finalContent, final); assert.equal(events.at(-1).settled, true);
    assert.doesNotThrow(() => JSON.parse(result.output));
  });
  assert.equal(turns, 2); assert.equal(executions, 1);
});
test('a tool failure is an error result for the next Agent turn, never a persistence claim', { timeout: 4000 }, async () => {
  let turns = 0;
  await withServer((body, res) => {
    turns++;
    if (turns === 1) sse(res, { tool: call() });
    else {
      const result = JSON.parse(toolMessages(body).at(-1).content);
      assert.equal(result.ok, false); assert.equal(result.error.code, 'CONFLICT'); assert(!JSON.stringify(result).includes(SECRET));
      sse(res, { text: '保存失败，请处理冲突后重试。', final: true });
    }
  }, async (endpoint) => {
    const result = await runLearningAgent(request(endpoint), () => {}, { requestTool: async () => ({ error: { code: 'CONFLICT', message: SECRET } }) });
    assert(result.output.includes('保存失败'));
  });
  assert.equal(turns, 2);
});
test('duplicate tool ids replay the same receipt without a second execution', { timeout: 4000 }, async () => {
  let turns = 0; let executions = 0;
  await withServer((body, res) => {
    turns++;
    if (turns <= 2) sse(res, { tool: call() });
    else { assert.equal(toolMessages(body).length, 2); sse(res, { text: '已收到持久化回执。', final: true }); }
  }, async (endpoint) => {
    await runLearningAgent(request(endpoint), () => {}, { requestTool: async () => { executions++; return { result: { persisted: true, id: 'one-record' } }; } });
  });
  assert.equal(turns, 3); assert.equal(executions, 1);
});
test('same tool id with changed arguments fails before another effect', { timeout: 4000 }, async () => {
  let turns = 0; let executions = 0;
  await withServer((_body, res) => { turns++; sse(res, { tool: call('same-id', 'record_question', { question: turns === 1 ? 'first' : 'changed' }) }); }, async (endpoint) => {
    await assert.rejects(runLearningAgent(request(endpoint), () => {}, { requestTool: async () => { executions++; return { result: { persisted: true } }; } }), (error) => error.code === 'TOOL_PROTOCOL');
  });
  assert.equal(executions, 1); assert.equal(turns, 2);
});
test('unregistered model tool call is aborted and never forwarded to Rust', { timeout: 4000 }, async () => {
  let executions = 0; let turns = 0;
  await withServer((_body, res) => { turns++; sse(res, { tool: call('shell-1', 'bash', { command: 'must not run' }) }); }, async (endpoint) => {
    await assert.rejects(runLearningAgent(request(endpoint), () => {}, { requestTool: async () => { executions++; } }), (error) => error.code === 'TOOLS_DISABLED');
  });
  assert.equal(turns, 1); assert.equal(executions, 0);
});
test('32KiB argument and result envelopes report capacity errors', async () => {
  const plugin = createLearningPlugin({ requestTool: async () => ({ result: { persisted: true } }) });
  const args = { question: '中'.repeat(8000), selection: '文'.repeat(4000) };
  assert(Buffer.byteLength(JSON.stringify(args)) > MAX_TOOL_BYTES);
  assert.throws(() => plugin.preflight({ id: 'large', name: 'record_question' }, args), (error) => error.code === 'TOOL_SIZE_LIMIT');
  const oversized = createLearningPlugin({ requestTool: async () => ({ result: { content: 'x'.repeat(MAX_TOOL_BYTES) } }) });
  await assert.rejects(oversized.tools.find(tool => tool.name === 'query_related_knowledge').execute('large-result', { query: 'x' }), error => error.code === 'TOOL_SIZE_LIMIT');
});
test('all tool argument and result string values redact the current key before IPC or model use', { timeout: 4000 }, async () => {
  let turns = 0;
  await withServer((body, res) => {
    turns++;
    if (turns === 1) sse(res, { tool: call('secret-safe', 'create_error', { front: `front ${SECRET}`, back: 'back', cause: SECRET, tags: [SECRET] }) });
    else { assert(!JSON.stringify(toolMessages(body)).includes(SECRET)); sse(res, { text: '已收到安全回执，错因仍待确认。', final: true }); }
  }, async (endpoint) => {
    const events = [];
    await runLearningAgent(request(endpoint), (event) => events.push(event), { requestTool: async (tool) => {
      assert(!JSON.stringify(tool).includes(SECRET)); assert.equal(tool.args.cause, '[REDACTED]'); assert.equal(tool.args.tags[0], '[REDACTED]');
      return { result: { persisted: true, id: 'safe', nested: { text: SECRET, items: [SECRET] } } };
    } });
    assert(!JSON.stringify(events).includes(SECRET));
  });
});
test('Markdown text and missing persisted receipt cannot be accepted as tool success', async () => {
  const plugin = createLearningPlugin({ requestTool: async () => ({ result: '## Saved (not a receipt)' }) });
  await assert.rejects(plugin.tools[0].execute('markdown', { question: 'Q' }), (error) => error.code === 'TOOL_PROTOCOL');
  const missing = createLearningPlugin({ requestTool: async () => ({ result: { message: 'Saved (not durable)' } }) });
  const result = await missing.tools[0].execute('missing', { question: 'Q' });
  assert.equal(result.isError, true); assert.equal(result.structuredContent.ok, false);
});
test('tool bridge rejects mismatched responses and cancellation wakes pending execution', async () => {
  const controller = new AbortController(); const emitted = [];
  const bridge = createStdioToolBridge((event) => emitted.push(event), controller.signal);
  const pending = bridge.requestTool(call(), controller.signal);
  await new Promise((done) => setImmediate(done));
  assert.equal(emitted[0].type, 'tool_request');
  assert.throws(() => bridge.accept({ type: 'tool_result', id: 'unknown', result: {} }), (error) => error.code === 'TOOL_PROTOCOL');
  controller.abort(); await assert.rejects(pending, (error) => error.code === 'CANCELLED'); bridge.close();
});
test('CLI performs actual NDJSON tool request/result round trip and cancels a pending tool safely', { timeout: 6000 }, async () => {
  for (const cancel of [false, true]) {
    let turns = 0;
    await withServer((body, res) => { turns++; if (turns === 1) sse(res, { tool: call() }); else { assert.equal(JSON.parse(toolMessages(body).at(-1).content).result.persisted, true); sse(res, { text: '保存回执已确认。', final: true }); } }, async (endpoint) => {
      const child = spawn(process.execPath, ['--permission', `--allow-fs-read=${root}`, join(root, 'sidecar.mjs')], { cwd: root, env: { PATH: '/usr/bin:/bin', LANG: 'en_US.UTF-8' }, stdio: ['pipe', 'pipe', 'pipe'] });
      let stdout = ''; let buffer = ''; let stderr = ''; const events = [];
      child.stdout.on('data', (chunk) => {
        stdout += chunk; buffer += chunk;
        for (;;) {
          const end = buffer.indexOf('\n'); if (end < 0) break;
          const event = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1); events.push(event);
          if (event.type === 'tool_request') child.stdin.write(`${JSON.stringify(cancel ? { type: 'cancel' } : { type: 'tool_result', id: event.id, result: { persisted: true, id: 'stdio-receipt' } })}\n`);
        }
      });
      child.stderr.on('data', (chunk) => { stderr += chunk; });
      const ended = new Promise((done, reject) => { child.on('exit', done); child.on('error', reject); });
      const timer = setTimeout(() => child.kill('SIGKILL'), 4000);
      child.stdin.write(`${JSON.stringify(request(endpoint))}\n`);
      const status = await ended; clearTimeout(timer);
      assert.equal(status, cancel ? 1 : 0); assert(events.some((event) => event.type === 'tool_request'));
      if (cancel) { assert.equal(events.at(-1).code, 'CANCELLED'); assert(!events.some((event) => event.type === 'done')); }
      else assert.equal(events.at(-1).type, 'done');
      assert(!stdout.includes(SECRET)); assert(!stderr.includes(SECRET));
    });
    assert.equal(turns, cancel ? 1 : 2);
  }
});

test('each mode registers only its fixed subset; feedback is zero-tool single-request JSON', { timeout: 5000 }, async () => {
  for (const mode of ['question', 'explain', 'practice', 'feedback']) {
    const policy = { mode, ...MODE_POLICIES[mode] }; let turns = 0;
    const answer = mode === 'feedback' ? '{"status":"uncertain","explanation":"证据不足","evidence":[],"sourceIds":[]}' : `fixture ${mode}`;
    await withServer((body, res) => {
      turns++;
      assert.deepEqual((body.tools ?? []).map((tool) => tool.function.name), policy.allowedTools);
      if (mode === 'feedback') { assert(body.messages[0].content.includes('status')); assert(body.messages[0].content.includes('单轮')); assert.equal(body.tool_stream, undefined); }
      sse(res, { text: answer, final: true });
    }, async (endpoint) => {
      const input = request(endpoint); input.toolPolicy = policy;
      const result = await runLearningAgent(input, () => {}, { requestTool: async () => { throw new Error('unexpected tool'); }, onAgent: (agent) => assert.deepEqual(agent.state.tools.map((tool) => tool.name), policy.allowedTools) });
      assert.equal(result.output, answer);
    });
    assert.equal(turns, 1);
  }
});
test('a known write tool outside the practice subset is blocked before any Rust execution', { timeout: 4000 }, async () => {
  let executions = 0;
  await withServer((_body, res) => sse(res, { tool: call('forbidden-card', 'create_review_card', { front: 'F', back: 'B' }) }), async (endpoint) => {
    const input = request(endpoint); input.toolPolicy = { mode: 'practice', ...MODE_POLICIES.practice };
    await assert.rejects(runLearningAgent(input, () => {}, { requestTool: async () => { executions++; } }), (error) => error.code === 'TOOLS_DISABLED');
  });
  assert.equal(executions, 0);
});
test('ordinary, exploration, practice and legacy tasks finish after twelve successful calls', { timeout: 10000 }, async () => {
  for (const mode of ['question', 'explain', 'practice', 'legacy']) {
    const policy = { mode, ...MODE_POLICIES[mode] }; let turns = 0; let executions = 0;
    await withServer((_body, res) => { turns++; sse(res, turns <= 12 ? { tool: call(`query-${turns}`, 'query_related_knowledge', { query: 'since' }) } : { text: 'Finished twelve calls.', final: true }); }, async (endpoint) => {
      const input = request(endpoint); input.toolPolicy = policy;
      const result = await runLearningAgent(input, () => {}, { requestTool: async () => { executions++; return { result: { results: [] } }; } });
      assert.equal(result.output, 'Finished twelve calls.');
    });
    assert.equal(executions, 12); assert.equal(turns, 13);
  }
});
test('ordinary and exploration chat share all learning tools and consume card receipts', { timeout: 6000 }, async () => {
  assert.deepEqual(MODE_POLICIES.question, MODE_POLICIES.explain);
  for (const mode of ['question', 'explain']) {
    let turns = 0; const executed = [];
    await withServer((body, res) => {
      turns++;
      assert.deepEqual(body.tools.map((tool) => tool.function.name), MODE_POLICIES.explain.allowedTools);
      if (turns === 1) sse(res, { tool: call('card-1', 'create_review_card', { front: 'Since?', back: 'A starting point', tags: ['since'] }) });
      else if (turns === 2) { assert.equal(JSON.parse(toolMessages(body).at(-1).content).result.persisted, true); sse(res, { tool: call('note-1', 'create_note', { content: 'Since marks a starting point.', selection: 'since Monday' }) }); }
      else { assert.equal(toolMessages(body).length, 2); sse(res, { text: '卡片与笔记的保存回执已收到。', final: true }); }
    }, async (endpoint) => {
      const input = request(endpoint); input.toolPolicy = { mode, ...MODE_POLICIES[mode] };
      await runLearningAgent(input, () => {}, { requestTool: async (tool) => { executed.push(tool.name); return { result: { persisted: true, entryId: tool.id } }; } });
    });
    assert.deepEqual(executed, ['create_review_card', 'create_note']); assert.equal(turns, 3);
  }
});

test('create_todo is registered in question mode and its result is consumed without enabling reminders', { timeout: 4000 }, async () => {
  let turns = 0; const executions = [];
  await withServer((body, res) => {
    turns++; assert.deepEqual(body.tools.map((tool) => tool.function.name), MODE_POLICIES.question.allowedTools);
    const schema = body.tools.find((tool) => tool.function.name === 'create_todo').function.parameters;
    assert.deepEqual(Object.keys(schema.properties).sort(), ['dueAt', 'relativeMinutes', 'reminderEnabled', 'title']); assert.equal(schema.additionalProperties, false);
    if (turns === 1) sse(res, { tool: call('todo-1', 'create_todo', { title: '复习 since 与 for', dueAt: '2000-01-01T08:00:00+08:00' }) });
    else { const receipt = JSON.parse(toolMessages(body).at(-1).content); assert.equal(receipt.result.persisted, true); assert.equal(receipt.result.todo.reminderEnabled, false); sse(res, { text: '待办已保存，提醒未开启。', final: true }); }
  }, async (endpoint) => {
    const input = request(endpoint); input.settings = { provider: 'custom', protocol: 'openai-completions', baseUrl: endpoint, model: 'user-custom-model' };
    input.toolPolicy = { mode: 'question', ...MODE_POLICIES.question };
    const result = await runLearningAgent(input, () => {}, { requestTool: async (tool) => { executions.push(tool); return { result: { persisted: true, todoId: 'fixture-todo', todo: { title: tool.args.title, reminderEnabled: false } } }; } });
    assert.equal(result.output, '待办已保存，提醒未开启。');
  });
  assert.equal(turns, 2); assert.equal(executions.length, 1); assert.equal(executions[0].name, 'create_todo');
  const plugin = createLearningPlugin({ requestTool: async () => {}, allowedTools: ['create_todo'] });
  for (const args of [{ title: 'T', reminderEnabled: true }, { title: 'T', sessionId: 'foreign' }, { title: 'T', dueAt: 'tomorrow' }]) assert.throws(() => plugin.validate({ id: 'invalid', name: 'create_todo', arguments: args }));
});

test('generate_exercises uses the actual SDK tool loop and structured rubric with a durable receipt', {timeout:4000}, async()=>{
  let turns=0, executions=0;
  const args={requirements:'生成一道物理解释题',summary:'平均速度',exercises:[{question:'如何求平均速度？',format:'self',answers:['总路程除以总时间'],explanation:'按定义计算',tags:['物理'],sourceIds:['section_physics'],rubric:{criteria:[{id:'definition',label:'定义',description:'说明总路程与总时间',maxPoints:5}]}}]};
  await withServer((body,res)=>{
    turns++;assert(body.tools.some(t=>t.function.name==='generate_exercises'));
    if(turns===1)sse(res,{tool:call('physics-exercise-1','generate_exercises',args)});
    else{const receipt=JSON.parse(toolMessages(body).at(-1).content);assert.equal(receipt.result.documentId,'exdoc_fixture');sse(res,{text:'练习已按真实回执保存。',final:true});}
  },async(endpoint)=>{
    const input=request(endpoint);input.toolPolicy={mode:'question',...MODE_POLICIES.question};
    const result=await runLearningAgent(input,()=>{},{requestTool:async tool=>{executions++;assert.deepEqual(tool.args,args);return{result:{persisted:true,documentId:'exdoc_fixture',exerciseEntryId:'entry_fixture'}};}});
    assert.equal(result.output,'练习已按真实回执保存。');
  });assert.equal(turns,2);assert.equal(executions,1);
});

test('exercise schema errors return to the actual SDK loop for bounded correction', {timeout:8000}, async()=>{
  for (const bad of [{requirements:'BST',exercises:[]},{requirements:'BST',summary:'',exercises:Array(6).fill({})},{requirements:'BST',summary:'',exercises:[{}],extra:'denied'}]) {
    let turns=0,executions=0;
    const good={requirements:'BST',summary:'汉'.repeat(4000),exercises:[{question:'BST 查找复杂度？',format:'exact',answers:['树高'],explanation:'',tags:[],sourceIds:[]}]};
    await withServer((body,res)=>{
      turns++;
      if(turns===1)sse(res,{tool:call('bad-exercise','generate_exercises',bad)});
      else if(turns===2){assert(toolMessages(body).length>0);sse(res,{tool:call('fixed-exercise','generate_exercises',good)});}
      else sse(res,{text:'已保存',final:true});
    },async endpoint=>{
      const input=request(endpoint);input.toolPolicy={mode:'question',...MODE_POLICIES.question};
      await runLearningAgent(input,()=>{},{requestTool:async tool=>{executions++;assert.deepEqual(tool.args,good);return {result:{persisted:true,documentId:'exdoc_synthetic'}};}});
    });
    assert.equal(turns,3);assert.equal(executions,1);
  }
});
test('safe Rust field diagnostics reach model correction without leaking generic errors',async()=>{
  const plugin=createLearningPlugin({allowedTools:['generate_exercises'],requestTool:async()=>({error:{code:'INVALID_MODEL_OUTPUT',message:'练习校验失败：exercises[1].sourceIds 来源编号重复'}})});
  const args={requirements:'BST',summary:'',exercises:[{question:'x',format:'exact',answers:['x'],explanation:'',tags:[],sourceIds:[]}]};
  const result=await plugin.tools[0].execute('validation-test',args);
  assert.equal(result.isError,true);assert.match(result.structuredContent.error.message,/sourceIds/);
});

test('final practice JSON receives safe validator feedback with two corrections maximum and no writes', {timeout:8000}, async()=>{
  for (const mode of ['repair','exhaust','source']) {
    let turns=0,validations=0;
    await withServer((body,res)=>{
      turns++;if(turns>1)assert(JSON.stringify(body.messages).includes('exercises[1].question'));
      sse(res,{text:turns===1?'not-json':'{"exercises":[],"summary":"synthetic"}',final:true});
    },async endpoint=>{
      const input=request(endpoint);input.toolPolicy={mode:'practice',...MODE_POLICIES.practice};input.outputValidation=true;
      const result=await runLearningAgent(input,()=>{},{requestTool:async call=>{
        assert.equal(call.type,'validate_output');validations++;
        return {result:{valid:mode==='repair'&&validations===2,recoverable:mode!=='source',message:'exercises[1].question 必须是非空文本，最多20000字符'}};
      }});
      assert(result.output);
    });
    assert.equal(turns,mode==='repair'?2:mode==='source'?1:3);assert.equal(validations,turns);
  }
});

test('guidance catalog and load register only in question, explain and practice with read-only semantics', async () => {
  for (const mode of ['question', 'explain', 'practice']) {
    for (const name of ['list_learning_skills', 'load_learning_skill']) {
      assert.ok(MODE_POLICIES[mode].allowedTools.includes(name), `${mode} must register ${name}`);
    }
    const policy = validateToolPolicy({ mode, ...MODE_POLICIES[mode] });
    assert.deepEqual(policy.allowedTools, MODE_POLICIES[mode].allowedTools);
  }
  for (const mode of ['review', 'feedback', 'grading', 'legacy']) {
    assert.ok(!MODE_POLICIES[mode].allowedTools.includes('load_learning_skill'));
    assert.throws(() => validateToolPolicy({ mode, allowedTools: ['load_learning_skill'] }), (error) => error.code === 'TOOL_PROTOCOL');
  }
  assert.deepEqual(validateCall('list-ok', 'list_learning_skills', {}), {});
  assert.throws(() => validateCall('list-bad', 'list_learning_skills', { path: 'x' }), (error) => error.code === 'TOOL_PROTOCOL');
  assert.deepEqual(validateCall('load-ok', 'load_learning_skill', { skillId: 'learning-review' }), { skillId: 'learning-review' });
  for (const args of [{}, { skillId: '' }, { skillId: 'x', extra: 1 }, { skillId: 7 }]) {
    assert.throws(() => validateCall('load-bad', 'load_learning_skill', args), (error) => error.code === 'TOOL_PROTOCOL', JSON.stringify(args));
  }
  // Read-only tools need no persisted receipt and replay safely.
  const plugin = createLearningPlugin({ allowedTools: ['list_learning_skills', 'load_learning_skill'], requestTool: async (tool) => tool.name === 'list_learning_skills'
    ? { result: { sessionId: 'session_fixture', skills: [{ id: 'learning-review', name: 'learning-review', description: '复习指导' }] } }
    : { result: { sessionId: 'session_fixture', skillId: 'learning-review', sha256: 'a'.repeat(64), content: '---\nname: learning-review\n---\n复习指导正文' } } });
  assert.deepEqual(plugin.tools.map((tool) => tool.replay), ['safe', 'safe']);
  const list = await plugin.tools[0].execute('list-1', {});
  assert.equal(list.isError, false);
  assert.equal(list.structuredContent.result.skills[0].id, 'learning-review');
  const load = await plugin.tools[1].execute('load-1', { skillId: 'learning-review' });
  assert.equal(load.isError, false);
  assert.ok(load.structuredContent.result.content.includes('复习指导正文'));
  const failing = createLearningPlugin({ allowedTools: ['load_learning_skill'], requestTool: async () => ({ result: { skillId: 'learning-review', alreadyLoaded: true } }) });
  const dedup = await failing.tools[0].execute('load-2', { skillId: 'learning-review' });
  assert.equal(dedup.isError, false);
});

test('question mode loads guidance through the real tool channel without persistence claims', { timeout: 4000 }, async () => {
  let turns = 0;
  await withServer((body, res) => {
    turns++;
    const names = (body.tools ?? []).map((tool) => tool.function.name);
    assert.ok(names.includes('list_learning_skills') && names.includes('load_learning_skill'));
    if (turns === 1) sse(res, { tool: call('skill-1', 'load_learning_skill', { skillId: 'learning-review' }) });
    else {
      const result = JSON.parse(toolMessages(body).at(-1).content);
      assert.equal(result.ok, true);
      assert.equal(result.result.skillId, 'learning-review');
      assert.ok(result.result.content.startsWith('---\nname: learning-review'));
      sse(res, { text: '指导正文已收到。', final: true });
    }
  }, async (endpoint) => {
    const input = request(endpoint);
    input.toolPolicy = { mode: 'question', ...MODE_POLICIES.question };
    const result = await runLearningAgent(input, () => {}, { requestTool: async () => ({ result: { sessionId: 'session_fixture', skillId: 'learning-review', sha256: 'f'.repeat(64), content: '---\nname: learning-review\ndescription: 复习\n---\n复习指导正文' } }) });
    assert.equal(result.output, '指导正文已收到。');
  });
  assert.equal(turns, 2);
});

test('authorization tiers add read/write/bash only to question and explain policies', () => {
  const tiers = { read: ['read'], write: ['read', 'write'], bash: ['read', 'write', 'bash'] };
  for (const [tier, tools] of Object.entries(tiers)) {
    for (const mode of ['question', 'explain']) {
      const policy = validateToolPolicy({ mode, allowedTools: [...MODE_POLICIES[mode].allowedTools, ...tools], authorization: tier });
      assert.equal(policy.authorization, tier);
      assert.deepEqual(policy.allowedTools, [...MODE_POLICIES[mode].allowedTools, ...tools]);
    }
    for (const mode of ['practice', 'review', 'feedback', 'grading', 'legacy']) {
      assert.throws(() => validateToolPolicy({ mode, allowedTools: ['read'], authorization: tier }), (error) => error.code === 'TOOL_PROTOCOL', `${mode} must not gain ${tier} tools`);
    }
  }
  assert.throws(() => validateToolPolicy({ mode: 'question', allowedTools: ['write'], authorization: 'read' }), (error) => error.code === 'TOOL_PROTOCOL');
  assert.throws(() => validateToolPolicy({ mode: 'explain', allowedTools: ['bash'], authorization: 'write' }), (error) => error.code === 'TOOL_PROTOCOL');
  assert.throws(() => validateToolPolicy({ mode: 'question', allowedTools: [], authorization: 'shell' }), (error) => error.code === 'TOOL_PROTOCOL');
  const plain = validateToolPolicy({ mode: 'question', ...MODE_POLICIES.question });
  assert.equal(plain.authorization, 'off');
  assert(!plain.allowedTools.includes('read'));
});

test('workspace tool arguments are shape-validated at the plugin boundary', () => {
  for (const [name, args] of [['read', { sourceId: 'knowledge_1' }], ['read', { path: 'notes/a.md' }], ['write', { path: 'a.md', content: 'x' }], ['write', { path: 'a.md', content: 'x', expectedVersion: 'version-1' }], ['bash', { command: 'printf hi' }], ['bash', { command: 'printf hi', timeoutMs: 1500 }]]) {
    assert.deepEqual(validateCall('shape-ok', name, args), args);
  }
  for (const [name, args] of [['read', {}], ['read', { sourceId: 'a', path: 'b' }], ['read', { sourceId: 'ok', extra: 1 }], ['write', { path: 'a.md' }], ['write', { path: '', content: 'x' }], ['write', { path: 'a.md', content: 'x', expectedVersion: 1 }], ['write', { path: 'a.md', content: 'x', expectedVersion: '' }], ['bash', { command: '   ' }], ['bash', { command: 'echo', timeoutMs: 100 }], ['bash', { command: 'echo', timeoutMs: 99000 }], ['bash', { command: 'echo', extra: 1 }]]) {
    assert.throws(() => validateCall('shape-bad', name, args), (error) => error.code === 'TOOL_PROTOCOL', `${name} ${JSON.stringify(args)} must be rejected`);
  }
});

test('read returns results without persisted receipts; write and bash require them', async () => {
  const plugin = createLearningPlugin({ allowedTools: ['read', 'write', 'bash'], requestTool: async (tool) => {
    if (tool.name === 'read') return { result: { sessionId: 'session_fixture', content: 'synthetic text' } };
    if (tool.name === 'write') return { result: { persisted: true, path: 'a.md', bytes: 1 } };
    return { result: { persisted: true, exitCode: 0, stdout: 'ok' } };
  } });
  assert.deepEqual(plugin.tools.map((tool) => tool.name), ['read', 'write', 'bash']);
  const read = await plugin.tools[0].execute('read-1', { path: 'a.md' });
  assert.equal(read.isError, false);
  assert.equal(read.structuredContent.result.content, 'synthetic text');
  const write = await plugin.tools[1].execute('write-1', { path: 'a.md', content: 'x' });
  assert.equal(write.structuredContent.result.persisted, true);
  const bash = await plugin.tools[2].execute('bash-1', { command: 'printf ok' });
  assert.equal(bash.structuredContent.result.persisted, true);
  const failing = createLearningPlugin({ allowedTools: ['write'], requestTool: async () => ({ result: { path: 'a.md' } }) });
  const failed = await failing.tools[0].execute('write-2', { path: 'a.md', content: 'x' });
  assert.equal(failed.isError, true);
});

test('authorized workspace tools register without exposing internal authorization tiers', { timeout: 4000 }, async () => {
  let system = '';
  await withServer((body, res) => { system = body.messages[0].content; sse(res, { text: 'fixture answer', final: true }); }, async (endpoint) => {
    const input = request(endpoint);
    input.toolPolicy = { mode: 'question', allowedTools: [...MODE_POLICIES.question.allowedTools, 'read', 'write', 'bash'], authorization: 'bash' };
    const result = await runLearningAgent(input, () => {}, { requestTool: async () => { throw new Error('unexpected tool'); } });
    assert.equal(result.output, 'fixture answer');
  });
  assert(!system.includes('工具授权档位'));
  assert(!system.includes('本机设置'));
  assert(!system.includes('本轮已注册工具'));
});

test('create_todo reminderEnabled is an opt-in boolean that requires a time', () => {
  assert.deepEqual(validateCall('reminder-1', 'create_todo', { title: 'T', reminderEnabled: false }), { title: 'T', reminderEnabled: false });
  assert.deepEqual(validateCall('reminder-2', 'create_todo', { title: 'T', relativeMinutes: 5, reminderEnabled: true }), { title: 'T', relativeMinutes: 5, reminderEnabled: true });
  for (const args of [{ title: 'T', reminderEnabled: true }, { title: 'T', reminderEnabled: 'yes' }, { title: 'T', reminderEnabled: 1 }]) {
    assert.throws(() => validateCall('reminder-bad', 'create_todo', args), (error) => error.code === 'TOOL_PROTOCOL', JSON.stringify(args));
  }
});


test('card execution needs no loaded skill and cancellation/retry never invents persistence', async () => {
  const args = { front: '合成问题', back: '合成答案' };
  const policy = validateToolPolicy({ mode: 'explain', allowedTools: ['create_review_card'] });
  let writes = 0;
  const controller = new AbortController();
  const plugin = createLearningPlugin({ ...policy, requestTool: async () => {
    writes++; controller.abort(); return { result: { persisted: true, entryId: 'saved-card' } };
  } });
  const card = plugin.tools[0];
  await assert.rejects(card.execute('before', args, controller.signal), e => e.code === 'CANCELLED');
  assert.equal(writes, 1);
  await assert.rejects(card.execute('before', args), e => e.code === 'CANCELLED');
  assert.equal(writes, 1); // Pending/cancelled calls do not replay effects.
  await assert.rejects(card.execute('never-started', args, controller.signal), e => e.code === 'CANCELLED');
  assert.equal(writes, 1);
  for (const reply of [{ error: { code: 'IO' } }, { result: { message: 'Saved' } }]) {
    const failed = createLearningPlugin({ ...policy, requestTool: async () => reply });
    const result = await failed.tools[0].execute('failed-card', args);
    assert.equal(result.isError, true);
    assert.equal(result.structuredContent.ok, false);
  }
});

 test('exercise tool never expands supplied skill guidance into descriptions',()=>{
 const plugin=createLearningPlugin({allowedTools:['generate_exercises'],exerciseGuidance:'FULL_SKILL_BODY_SENTINEL',requestTool:async()=>({})});
 assert(!plugin.tools[0].description.includes('FULL_SKILL_BODY_SENTINEL'));
 assert(plugin.tools[0].parameters.properties.exercises);
 });

test('write conflict recovery reaches the real Pi provider payload without arbitrary error text', { timeout: 8000 }, async () => {
  for (const reason of ['edit_review_pending', 'write_version_mismatch']) {
    let turns = 0;
    let received;
    await withServer((body, res) => {
      turns++;
      if (turns === 1) sse(res, { tool: call('blocked-write', 'write', { path: 'documents/note.md', content: 'draft', expectedVersion: 'read-version' }) });
      else {
        received = JSON.parse(toolMessages(body).at(-1).content);
        sse(res, { text: '请先处理原建议。', final: true });
      }
    }, async endpoint => {
      const input = request(endpoint);
      input.toolPolicy = { mode: 'question', allowedTools: ['write'], authorization: 'write' };
      await runLearningAgent(input, () => {}, { requestTool: async () => ({ error: {
        code: 'CONFLICT', message: 'SECRET ARBITRARY MESSAGE',
        details: { reason, proposalId: 'proposal_safe', courseId: 'course_safe', status: 'pending', path: '/private/path', content: 'SECRET CONTENT' },
      } }) });
    });
    assert.equal(turns, 2);
    assert.equal(received.ok, false);
    assert.equal(received.error.code, 'CONFLICT');
    assert.equal(received.error.details.reason, reason);
    assert.match(received.error.message, reason === 'edit_review_pending' ? /先接受或拒绝旧建议/ : /read 返回的 version/);
    assert(!JSON.stringify(received).includes('SECRET'));
    assert(!JSON.stringify(received).includes('/private'));
  }
});

test('unknown or malformed write conflict diagnostics remain sanitized', async () => {
  for (const details of [{ reason: 'other', path: '/private' }, { reason: 'edit_review_pending', proposalId: '../secret', courseId: 'safe', status: 'pending' }, { reason: 'edit_review_pending', proposalId: 'safe', courseId: 'safe', status: 'invalid' }]) {
    const plugin = createLearningPlugin({ allowedTools: ['write'], requestTool: async () => ({ error: { code: 'CONFLICT', message: 'SECRET ERROR', details } }) });
    const result = await plugin.tools[0].execute('invalid-feedback', { path: 'note.md', content: 'draft' });
    assert.equal(result.isError, true);
    assert.equal(result.structuredContent.error.details, undefined);
    assert(!JSON.stringify(result.structuredContent).includes('SECRET'));
    assert(!JSON.stringify(result.structuredContent).includes('/private'));
  }
});
