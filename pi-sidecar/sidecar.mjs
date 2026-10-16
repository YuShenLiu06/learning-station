// This entry point emits NDJSON only. No console/SDK diagnostics or raw errors reach stdio.
import { StringDecoder } from 'node:string_decoder';
import { safeResponseDiagnostic } from './response-diagnostics.mjs';
console.log = console.info = console.warn = console.error = console.debug = () => {};
let running = false;
let finished = false;
let bytes = 0;
let pending = '';
let toolBridge;
const decoder = new StringDecoder('utf8');
const abort = new AbortController();
const emit = (event) => new Promise((resolve, reject) => {
  process.stdout.write(`${JSON.stringify(event)}\n`, (error) => error ? reject(error) : resolve());
});
async function exitError(code = 'PI_RUNTIME', diagnostic) {
  if (finished) return;
  finished = true;
  abort.abort();
  const safe = ['PLAN_USAGE_UNCONFIRMED', 'AUTH', 'RATE_LIMIT', 'TIMEOUT', 'CANCELLED', 'NETWORK', 'INVALID_MODEL_SETTINGS', 'INVALID_URL', 'INVALID_INPUT', 'MODEL_HTTP', 'MODEL_RESPONSE', 'MODEL_ENDPOINT_PAGE', 'MODEL_FORMAT', 'MODEL_LENGTH', 'MODEL_INCOMPLETE', 'MODEL_THINKING_ONLY', 'MODEL_EMPTY', 'OUTPUT_LIMIT', 'TOOL_LIMIT', 'TOOL_PROTOCOL', 'TOOL_FAILED', 'TOOLS_DISABLED', 'PI_RUNTIME'].includes(code) ? code : 'PI_RUNTIME';
  await emit({ type: 'error', code: safe, ...(diagnostic ? { diagnostic: safeResponseDiagnostic(diagnostic) } : {}) }).catch(() => {});
  process.exit(1);
}
async function start(request) {
  if (running) { await exitError('INVALID_INPUT'); return; }
  running = true;
  const total = setTimeout(() => { void exitError('TIMEOUT'); }, request.operation === 'test' ? 30_000 : 120_000);
  try {
    const { runLearningAgent } = await import('./harness.mjs');
    const { createStdioToolBridge } = await import('./learning-plugin.mjs');
    toolBridge = createStdioToolBridge(emit, abort.signal);
    await runLearningAgent(request, emit, { signal: abort.signal, requestTool: toolBridge.requestTool });
    toolBridge.close();
    finished = true;
    clearTimeout(total);
    process.exit(0);
  } catch (error) {
    clearTimeout(total);
    await exitError(typeof error?.code === 'string' ? error.code : 'PI_RUNTIME', error?.diagnostic);
  }
}
function line(value) {
  let message;
  try { message = JSON.parse(value); } catch { void exitError('INVALID_INPUT'); return; }
  if (message?.type === 'cancel') { abort.abort(); return; }
  if (message?.type === 'tool_result') {
    try { if (!toolBridge) throw new Error(); toolBridge.accept(message); } catch { void exitError('TOOL_PROTOCOL'); }
    return;
  }
  void start(message);
}
process.stdin.on('data', (chunk) => {
  bytes += chunk.length;
  if (bytes > 4 * 1024 * 1024) { void exitError('INVALID_INPUT'); return; }
  pending += decoder.write(chunk);
  for (;;) {
    const index = pending.indexOf('\n');
    if (index < 0) break;
    const value = pending.slice(0, index); pending = pending.slice(index + 1);
    if (value.trim()) line(value);
  }
});
process.stdin.on('end', () => {
  pending += decoder.end();
  if (!running || pending.trim()) void exitError('INVALID_INPUT');
});
process.stdin.on('error', () => { void exitError('PI_RUNTIME'); });
process.stdout.on('error', () => process.exit(1));
process.on('uncaughtException', () => { void exitError('PI_RUNTIME'); });
process.on('unhandledRejection', () => { void exitError('PI_RUNTIME'); });
