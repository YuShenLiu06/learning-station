import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { mkdtemp, writeFile, mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Agent } from '@earendil-works/pi-agent-core';
import { createAssistantMessageEventStream } from '@earendil-works/pi-ai/utils/event-stream';
import { runLearningAgent, MODEL_ID, normalizeEndpoint, createModel, safeErrorEvent } from '../harness.mjs';

const SECRET = 'fixture-secret-not-a-real-key';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const usage = () => ({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } });
const request = (endpoint = 'http://127.0.0.1:12345/v1') => ({ protocol: 1, operation: 'generate', settings: { provider: 'glm-pi', baseUrl: endpoint, model: MODEL_ID }, apiKey: SECRET,
  messages: [{ role: 'system', content: 'UNTRUSTED SYSTEM MUST BE IGNORED' }, { role: 'user', content: 'Committed Markdown history supplied by Rust.' }, { role: 'assistant', content: 'Earlier committed assistant response.' }, { role: 'user', content: '{"type":"quoted_learning_material","mode":"explain","question":"why?"}' }] });
function fakeStream(text = 'fixture response', { inspect, tool = false, error = false, waitForAbort = false } = {}) {
  return (model, context, options) => {
    inspect?.(model, context, options);
    const stream = createAssistantMessageEventStream();
    const message = { role: 'assistant', content: [], api: model.api, provider: model.provider, model: model.id, usage: usage(), stopReason: 'pending', timestamp: Date.now() };
    queueMicrotask(async () => {
      stream.push({ type: 'start', partial: message });
      if (tool) {
        const call = { type: 'toolCall', id: 'forbidden', name: 'bash', arguments: { command: 'must never execute' } };
        message.content.push(call);
        stream.push({ type: 'toolcall_start', contentIndex: 0, partial: message });
        message.stopReason = 'toolUse'; stream.push({ type: 'done', reason: 'toolUse', message }); stream.end(); return;
      }
      message.content.push({ type: 'text', text: '' });
      stream.push({ type: 'text_start', contentIndex: 0, partial: message });
      for (const delta of [text.slice(0, 6), text.slice(6)]) {
        message.content[0].text += delta;
        stream.push({ type: 'text_delta', contentIndex: 0, delta, partial: message });
      }
      if (waitForAbort) {
        if (!options.signal.aborted) await new Promise((resolveAbort) => options.signal.addEventListener('abort', resolveAbort, { once: true }));
        message.stopReason = 'aborted'; message.errorMessage = `unsafe ${SECRET}`;
        stream.push({ type: 'error', reason: 'aborted', error: message });
      } else if (error) {
        message.stopReason = 'error'; message.errorMessage = `unsafe ${SECRET}`;
        stream.push({ type: 'error', reason: 'error', error: message });
      } else {
        message.stopReason = 'stop';
        stream.push({ type: 'text_end', contentIndex: 0, content: text, partial: message });
        stream.push({ type: 'done', reason: 'stop', message });
      }
      stream.end();
    });
    return stream;
  };
}
async function withServer(handler, action) {
  const server = createServer(handler);
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen));
  try { return await action(`http://127.0.0.1:${server.address().port}/v1`); }
  finally { server.closeAllConnections(); await new Promise((resolveClose) => server.close(resolveClose)); }
}
function sendSse(response, text = 'provider fixture 文本', finish = true) {
  response.writeHead(200, { 'content-type': 'text/event-stream' });
  const event = (delta, reason = null) => `data: ${JSON.stringify({ id: 'fixture', model: MODEL_ID, choices: [{ index: 0, delta, finish_reason: reason }] })}\n\n`;
  const body = event({ role: 'assistant', content: text }) + (finish ? event({}, 'stop') + 'data: [DONE]\n\n' : '');
  const bytes = Buffer.from(body);
  for (let offset = 0; offset < bytes.length; offset += 7) response.write(bytes.subarray(offset, offset + 7));
  response.end();
}
async function readPayload(req) { let text = ''; for await (const chunk of req) text += chunk; return JSON.parse(text); }
async function cli(input, { cwd = root, cancelAfterReady = false } = {}) {
  const args = ['--permission', `--allow-fs-read=${root}`, join(root, 'sidecar.mjs')];
  assert(!args.join(' ').includes(SECRET));
  const child = spawn(process.execPath, args, { cwd, env: { PATH: '/usr/bin:/bin', LANG: 'en_US.UTF-8' }, stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = ''; let stderr = '';
  child.stdout.on('data', (data) => {
    stdout += data;
    if (cancelAfterReady && stdout.includes('"type":"ready"')) child.stdin.write('{"type":"cancel"}\n');
  });
  child.stderr.on('data', (data) => { stderr += data; });
  const ended = new Promise((resolveExit, reject) => { child.on('error', reject); child.on('exit', (code) => resolveExit(code)); });
  child.stdin.write(`${JSON.stringify(input)}\n`);
  const timer = setTimeout(() => child.kill('SIGKILL'), 6000);
  const code = await ended;
  clearTimeout(timer);
  return { code, stdout, stderr, events: stdout.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line)) };
}

test('only Coding Plan official endpoints or localhost fixtures are accepted', () => {
  for (const endpoint of ['https://open.bigmodel.cn/api/coding/paas/v4/', 'https://api.z.ai/api/coding/paas/v4']) assert(normalizeEndpoint(endpoint).includes('/coding/'));
  for (const endpoint of ['https://api.z.ai/api/paas/v4', 'https://open.bigmodel.cn/api/paas/v4', 'https://example.com/v1', 'http://api.z.ai/api/coding/paas/v4', 'https://@api.z.ai/api/coding/paas/v4', 'https://api.z.ai/api/coding/paas/v4?key=bad']) assert.throws(() => normalizeEndpoint(endpoint));
  assert.throws(() => createModel({ ...request().settings, model: 'another-model' }));
});
test('real Pi Agent has empty tools, only a short clock system, explicit Rust history, and streams', async () => {
  const events = [];
  let actualAgent;
  const input = {...request(),timeContext:{referenceTime:'2026-10-21T09:00:00Z',timeZone:'Etc/UTC'}};
  const result = await runLearningAgent(input, (event) => events.push(event), {
    onAgent: (agent) => { actualAgent = agent; assert(agent instanceof Agent); assert.deepEqual(agent.state.tools, []); },
    streamFn: fakeStream('fixture 文本', { inspect: (model, context) => {
      assert.equal(model.id, MODEL_ID); assert.equal(model.compat.thinkingFormat, 'zai');
      assert(context.messages.some(m=>{try{return JSON.parse(m.content).referenceTime===input.timeContext.referenceTime;}catch{return false;}}));

      assert(!JSON.stringify(context).includes('UNTRUSTED SYSTEM'));
      assert(context.messages.some((message) => message.role === 'assistant'));
      assert(!context.messages.some((message) => message.role === 'toolResult'));
    } }),
  });
  assert.equal(result.output, 'fixture 文本'); assert.equal(result.settled, true);
  assert.equal(events.filter((event) => event.type === 'delta').map((event) => event.delta).join(''), result.output);
  assert.equal(events.at(-1).type, 'done'); assert.equal(actualAgent.state.isStreaming, false);
});
test('assistant message_end and agent_end do not bypass awaited settlement', { timeout: 2000 }, async () => {
  const events = [];
  let release; let entered;
  const barrier = new Promise((resolveBarrier) => { release = resolveBarrier; });
  const atBarrier = new Promise((resolveEntered) => { entered = resolveEntered; });
  let completed = false;
  const run = runLearningAgent(request(), (event) => events.push(event), { streamFn: fakeStream(), settleBarrier: async () => { entered(); await barrier; } }).then(() => { completed = true; });
  await atBarrier; await new Promise((resolveTick) => setTimeout(resolveTick, 15));
  assert.equal(completed, false); assert.equal(events.some((event) => event.type === 'done'), false);
  release(); await run; assert.equal(events.at(-1).settled, true);
});
test('any model toolcall aborts with no executable tools and no follow-up', async () => {
  let calls = 0; const events = [];
  await assert.rejects(runLearningAgent(request(), (event) => events.push(event), { streamFn: (...args) => { calls++; return fakeStream('', { tool: true })(...args); } }), (error) => error.code === 'TOOLS_DISABLED');
  assert.equal(calls, 1); assert(!events.some((event) => event.type === 'done'));
});
test('preflight unconditionally blocks tools', async () => {
  await assert.rejects(runLearningAgent(request(), () => {}, { streamFn: fakeStream(), onAgent: async (agent) => {
    const result = await agent.beforeToolCall({ toolCall: { name: 'anything' } });
    assert.deepEqual(result, { block: true, reason: 'Tools are disabled.', terminate: true });
  } }), (error) => error.code === 'TOOLS_DISABLED');
});
test('cancellation calls Agent abort and never reports success', async () => {
  const controller = new AbortController(); const events = [];
  const run = runLearningAgent(request(), (event) => { events.push(event); if (event.type === 'ready') setTimeout(() => controller.abort(), 10); }, { signal: controller.signal, streamFn: fakeStream('partial output that is long enough to stream', { waitForAbort: true }) });
  await assert.rejects(run, (error) => error.code === 'CANCELLED');
  assert(!events.some((event) => event.type === 'done')); assert(!JSON.stringify(events).includes(SECRET));
});
test('raw provider errors and a key split across deltas cannot echo on protocol output', async () => {
  const events = [];
  const result = await runLearningAgent(request(), (event) => events.push(event), { streamFn: fakeStream(`before ${SECRET} after`) });
  assert.equal(result.output, 'before [REDACTED] after'); assert(!JSON.stringify(events).includes(SECRET));
  await assert.rejects(runLearningAgent(request(), () => {}, { streamFn: fakeStream('partial', { error: true }) }), (error) => { assert(!error.message.includes(SECRET)); return error.code === 'MODEL_RESPONSE'; });
  assert(!JSON.stringify(safeErrorEvent(`unsafe ${SECRET}`)).includes(SECRET));
});
test('external Coding Plan use is gated before any stream call', async () => {
  let calls = 0;
  for (const endpoint of ['https://open.bigmodel.cn/api/coding/paas/v4', 'https://api.z.ai/api/coding/paas/v4']) {
    await assert.rejects(runLearningAgent(request(endpoint), () => {}, { streamFn: () => { calls++; } }), (error) => error.code === 'PLAN_USAGE_UNCONFIRMED');
  }
  assert.equal(calls, 0);
});
test('real Pi provider sends exact GLM model, thinking enabled, no tools, and streams UTF-8', async () => {
  await withServer(async (req, res) => {
    assert.equal(req.url, '/v1/chat/completions'); assert.equal(req.headers.authorization, `Bearer ${SECRET}`);
    const payload = await readPayload(req);
    assert.equal(payload.model, MODEL_ID); assert.equal(payload.stream, true);
    assert.deepEqual(payload.thinking, { type: 'enabled', clear_thinking: false }); assert.equal(payload.reasoning_effort, 'max');
    assert.equal(payload.tools, undefined); assert.equal(payload.tool_choice, undefined);
    assert(!JSON.stringify(payload).includes(SECRET));
    sendSse(res);
  }, async (endpoint) => {
    const events = [];
    const result = await runLearningAgent(request(endpoint), (event) => events.push(event));
    assert.equal(result.output, 'provider fixture 文本'); assert.equal(events.at(-1).settled, true);
  });
});
test('truncated provider stream cannot produce settled success', async () => {
  await withServer(async (req, res) => { await readPayload(req); sendSse(res, 'partial', false); }, async (endpoint) => {
    const events = [];
    await assert.rejects(runLearningAgent(request(endpoint), (event) => events.push(event)), (error) => error.code === 'MODEL_INCOMPLETE');
    assert(!events.some((event) => event.type === 'done'));
  });
});
test('HTTP auth/rate errors are sanitized and never automatically retried', async () => {
  for (const [status, code] of [[401, 'AUTH'], [403, 'AUTH'], [429, 'RATE_LIMIT']]) {
    let calls = 0;
    await withServer(async (req, res) => { calls++; await readPayload(req); res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify({ error: { message: SECRET } })); }, async (endpoint) => {
      await assert.rejects(runLearningAgent(request(endpoint), () => {}), (error) => { assert(!error.message.includes(SECRET)); return error.code === code; });
    });
    assert.equal(calls, 1);
  }
});
test('redirect never sends credentials to a second endpoint', async () => {
  let followed = 0;
  await withServer((req, res) => { followed++; res.end(); }, async (other) => {
    await withServer(async (req, res) => { await readPayload(req); res.writeHead(302, { location: `${other}/chat/completions` }); res.end(); }, async (endpoint) => {
      await assert.rejects(runLearningAgent(request(endpoint), () => {}));
    });
  });
  assert.equal(followed, 0);
});
test('CLI permission sandbox ignores cwd Pi config and emits only safe NDJSON', async () => {
  const cwd = await mkdtemp(join(tmpdir(), 'learning-pi-fixture-'));
  await mkdir(join(cwd, '.pi')); await writeFile(join(cwd, '.pi', 'settings.json'), '{"apiKey":"unrelated-fixture-secret","tools":["bash"]}');
  try {
    await withServer(async (req, res) => { const payload = await readPayload(req); assert(!JSON.stringify(payload).includes('unrelated-fixture-secret')); sendSse(res, `safe ${SECRET} text`); }, async (endpoint) => {
      const result = await cli(request(endpoint), { cwd });
      assert.equal(result.code, 0); assert.equal(result.events.at(-1).type, 'done');
      assert(!result.stdout.includes(SECRET)); assert(!result.stderr.includes(SECRET));
      assert.equal(result.events.filter((event) => event.type === 'delta').map((event) => event.delta).join(''), 'safe [REDACTED] text');
    });
  } finally { await rm(cwd, { recursive: true, force: true }); }
});
test('CLI cancel exits promptly, without done or secret output', async () => {
  await withServer(async (req, res) => { await readPayload(req); res.writeHead(200, { 'content-type': 'text/event-stream' }); res.write(': waiting\n\n'); }, async (endpoint) => {
    const result = await cli(request(endpoint), { cancelAfterReady: true });
    assert.equal(result.code, 1); assert(!result.events.some((event) => event.type === 'done'));
    assert.equal(result.events.at(-1).code, 'CANCELLED'); assert(!result.stdout.includes(SECRET));
  });
});

const customRequest = (endpoint) => ({ ...request(endpoint), settings: { provider: 'custom', protocol: 'openai-completions', baseUrl: endpoint, model: 'user-selected/model-2026' } });
test('custom transport requires an explicit supported protocol, valid URL and user model', () => {
  assert.equal(createModel(customRequest('https://api.example.com/custom/v1/').settings).baseUrl, 'https://api.example.com/custom/v1');
  for (const protocol of ['', 'unsupported-protocol', 'openai-responses']) assert.throws(() => createModel({ ...customRequest('https://example.com/v1').settings, protocol }));
  for (const model of ['', 'x'.repeat(257), 'bad\nmodel']) assert.throws(() => createModel({ ...customRequest('https://example.com/v1').settings, model }));
  for (const endpoint of ['http://example.com/v1', 'file:///tmp/fixture-secret', 'https://user:fixture-secret@example.com/v1', 'https://@example.com/v1', 'https://example.com/v1?', 'https://example.com/v1#']) assert.throws(() => createModel(customRequest(endpoint).settings), (error) => !error.message.includes(SECRET));
});
test('custom real Pi transport preserves model/path without GLM thinking or fallback', async () => {
  let calls = 0;
  await withServer(async (req, res) => {
    calls++; assert.equal(req.url, '/custom/v2/chat/completions'); assert.equal(req.headers.authorization, `Bearer ${SECRET}`);
    const body = await readPayload(req);
    assert.equal(body.model, 'user-selected/model-2026'); assert.equal(body.stream, true);
    for (const name of ['thinking', 'reasoning_effort', 'tool_stream', 'tools']) assert.equal(body[name], undefined);
    sendSse(res, 'Custom Pi fixture 文本');
  }, async (endpoint) => {
    const input = customRequest(endpoint.replace('/v1', '/custom/v2'));
    const events = []; const result = await runLearningAgent(input, (event) => events.push(event));
    assert.equal(result.output, 'Custom Pi fixture 文本'); assert.equal(events.at(-1).settled, true);
  });
  assert.equal(calls, 1);
});
test('custom connection test uses same transport and failures never emit done or raw secrets', async () => {
  for (const failure of ['auth', 'truncated']) {
    let calls = 0;
    await withServer(async (req, res) => {
      calls++; const body = await readPayload(req); assert.equal(body.model, 'user-selected/model-2026'); assert.equal(body.tools, undefined);
      if (failure === 'auth') { res.writeHead(401); res.end(SECRET); } else sendSse(res, 'partial fixture', false);
    }, async (endpoint) => {
      const input = customRequest(endpoint); input.operation = 'test'; const events = [];
      await assert.rejects(runLearningAgent(input, (event) => events.push(event)), (error) => error.code === (failure === 'auth' ? 'AUTH' : 'MODEL_INCOMPLETE') && !error.message.includes(SECRET));
      assert(!events.some((event) => event.type === 'done')); assert(!JSON.stringify(events).includes(SECRET));
    });
    assert.equal(calls, 1);
  }
});
test('custom provider cannot bypass a known Coding Plan endpoint gate', async () => {
  let calls = 0;
  await assert.rejects(runLearningAgent(customRequest('https://open.bigmodel.cn/api/coding/paas/v4'), () => {}, { streamFn: () => { calls++; } }), (error) => error.code === 'PLAN_USAGE_UNCONFIRMED');
  assert.equal(calls, 0);
});
