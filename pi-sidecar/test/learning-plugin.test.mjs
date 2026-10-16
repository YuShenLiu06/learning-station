import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { MODEL_ID } from '../harness.mjs';
import { runLearningAgentWithSkills as runLearningAgent } from './skills-fixture.mjs';
import { createLearningPlugin, createStdioToolBridge, LEARNING_PLUGIN, LEGACY_TOOLS, MODE_POLICIES, MAX_TOOL_BYTES } from '../learning-plugin.mjs';
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
test('eight calls per run and a 32KiB argument envelope are hard limits', { timeout: 5000 }, async () => {
  let turns = 0; let executions = 0;
  await withServer((_body, res) => { turns++; sse(res, { tool: call(`query-${turns}`, 'query_related_knowledge', { query: 'since' }) }); }, async (endpoint) => {
    await assert.rejects(runLearningAgent(request(endpoint), () => {}, { requestTool: async () => { executions++; return { result: { items: [] } }; } }), (error) => error.code === 'TOOL_LIMIT');
  });
  assert.equal(executions, 8); assert.equal(turns, 9);
  const plugin = createLearningPlugin({ requestTool: async () => ({ result: { persisted: true } }) });
  const args = { question: '中'.repeat(8000), selection: '文'.repeat(4000) };
  assert(Buffer.byteLength(JSON.stringify(args)) > MAX_TOOL_BYTES);
  assert.throws(() => plugin.preflight({ id: 'large', name: 'record_question' }, args), (error) => error.code === 'TOOL_LIMIT');
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
test('question/explain/practice enforce their own call budgets', { timeout: 5000 }, async () => {
  for (const mode of ['question', 'explain', 'practice']) {
    const policy = { mode, ...MODE_POLICIES[mode] }; let turns = 0; let executions = 0;
    await withServer((_body, res) => { turns++; sse(res, { tool: call(`query-${turns}`, 'query_related_knowledge', { query: 'since' }) }); }, async (endpoint) => {
      const input = request(endpoint); input.toolPolicy = policy;
      await assert.rejects(runLearningAgent(input, () => {}, { requestTool: async () => { executions++; return { result: { results: [] } }; } }), (error) => error.code === 'TOOL_LIMIT');
    });
    assert.equal(executions, policy.maxCalls); assert.equal(turns, policy.maxCalls + 1);
  }
});
test('explain registers card and note tools and consumes their real result envelopes', { timeout: 4000 }, async () => {
  let turns = 0; const executed = [];
  await withServer((body, res) => {
    turns++;
    assert.deepEqual(body.tools.map((tool) => tool.function.name), MODE_POLICIES.explain.allowedTools);
    if (turns === 1) sse(res, { tool: call('card-1', 'create_review_card', { front: 'Since?', back: 'A starting point', tags: ['since'] }) });
    else if (turns === 2) { assert.equal(JSON.parse(toolMessages(body).at(-1).content).result.persisted, true); sse(res, { tool: call('note-1', 'create_note', { content: 'Since marks a starting point.', selection: 'since Monday' }) }); }
    else { assert.equal(toolMessages(body).length, 2); sse(res, { text: '卡片与笔记的保存回执已收到。', final: true }); }
  }, async (endpoint) => {
    const input = request(endpoint); input.toolPolicy = { mode: 'explain', ...MODE_POLICIES.explain };
    await runLearningAgent(input, () => {}, { requestTool: async (tool) => { executed.push(tool.name); return { result: { persisted: true, entryId: tool.id } }; } });
  });
  assert.deepEqual(executed, ['create_review_card', 'create_note']); assert.equal(turns, 3);
});

test('create_todo is registered in question mode and its result is consumed without enabling reminders', { timeout: 4000 }, async () => {
  let turns = 0; const executions = [];
  await withServer((body, res) => {
    turns++; assert.deepEqual(body.tools.map((tool) => tool.function.name), MODE_POLICIES.question.allowedTools);
    const schema = body.tools.find((tool) => tool.function.name === 'create_todo').function.parameters;
    assert.deepEqual(Object.keys(schema.properties).sort(), ['dueAt', 'relativeMinutes', 'title']); assert.equal(schema.additionalProperties, false);
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
