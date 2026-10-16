import { Type } from 'typebox';

export const LEARNING_PLUGIN = Object.freeze({
  id: 'learning-station.learning', version: '1.0.0',
  tools: Object.freeze(['record_question', 'create_error', 'query_related_knowledge', 'create_review_card', 'create_note']),
});
export const LEGACY_TOOLS = Object.freeze(LEARNING_PLUGIN.tools.slice(0, 3));
export const MODE_POLICIES = Object.freeze({
  question: { allowedTools: ['query_related_knowledge', 'record_question', 'create_note'], maxCalls: 4 },
  explain: { allowedTools: [...LEARNING_PLUGIN.tools], maxCalls: 6 },
  practice: { allowedTools: ['query_related_knowledge'], maxCalls: 2 },
  feedback: { allowedTools: [], maxCalls: 0 },
  legacy: { allowedTools: [...LEGACY_TOOLS], maxCalls: 8 },
});
export const MAX_TOOL_CALLS = 8;
export const MAX_TOOL_BYTES = 32 * 1024;
const known = new Set(LEARNING_PLUGIN.tools);
const safeMessages = Object.freeze({
  TOOL_PROTOCOL: '学习工具协议或参数无效。', TOOL_LIMIT: '学习工具调用超过本次预算。',
  TOOL_FAILED: '学习工具执行失败，未确认完成。', TOOLS_DISABLED: '请求了未注册的学习工具。',
  CANCELLED: '学习工具调用已取消。',
});
export class LearningToolError extends Error {
  constructor(code = 'TOOL_FAILED') { super(safeMessages[code] ?? safeMessages.TOOL_FAILED); this.code = Object.hasOwn(safeMessages, code) ? code : 'TOOL_FAILED'; }
}
export function redactStrings(value, secret) {
  if (typeof value === 'string') return secret ? value.replaceAll(secret, '[REDACTED]') : value;
  if (Array.isArray(value)) return value.map((item) => redactStrings(item, secret));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, redactStrings(item, secret)]));
  return value;
}
function stable(value) {
  if (Array.isArray(value)) return value.map(stable);
  if (value && typeof value === 'object') return Object.fromEntries(Object.keys(value).sort().map((key) => [key, stable(value[key])]));
  return value;
}
export function fingerprint(name, args) { return JSON.stringify([name, stable(args)]); }
function bounded(value) {
  let json;
  try { json = JSON.stringify(value); } catch { throw new LearningToolError('TOOL_PROTOCOL'); }
  if (typeof json !== 'string' || Buffer.byteLength(json) > MAX_TOOL_BYTES) throw new LearningToolError('TOOL_LIMIT');
  return json;
}
function validId(id) { return typeof id === 'string' && id.length > 0 && [...id].length <= 256 && !/[\u0000-\u001f\u007f]/u.test(id); }
const string = (maxLength) => Type.String({ minLength: 1, maxLength });
const optionalString = (maxLength) => Type.Optional(string(maxLength));
const definitions = [
  { name: 'record_question', label: '记录疑问',
    description: '记录用户明确提出的学习疑问。只使用当前会话，成功必须以Rust返回的persisted:true与记录ID为依据。',
    parameters: Type.Object({ question: string(8000), selection: Type.Optional(Type.String({ maxLength: 12000 })) }, { additionalProperties: false }) },
  { name: 'create_error', label: '创建错题',
    description: '将当前学习任务中的题面与答案保存为错题。cause仅是待确认建议，不代表用户确认的错因。成功以Rust持久化结果为准。',
    parameters: Type.Object({ front: string(8000), back: string(12000), cause: Type.Optional(Type.String({ maxLength: 4000 })), exerciseId: optionalString(128), tags: Type.Optional(Type.Array(string(64), { maxItems: 12 })) }, { additionalProperties: false }) },
  { name: 'query_related_knowledge', label: '查询关联知识',
    description: '经Rust查询当前会话允许范围内的关联知识，query和knowledgeId至少提供一项。不接受文件路径、会话ID或任意访问范围。',
    parameters: Type.Object({ query: optionalString(500), knowledgeId: optionalString(128) }, { additionalProperties: false }) },
  { name: 'create_review_card', label: '创建复习卡片',
    description: '按用户明确要求，把正反面内容保存为当前会话的复习卡片；只有Rust返回persisted:true才算成功。',
    parameters: Type.Object({ front: string(8000), back: string(12000), tags: Type.Optional(Type.Array(string(64), { maxItems: 12 })) }, { additionalProperties: false }) },
  { name: 'create_note', label: '创建笔记',
    description: '按用户明确要求，保存当前会话的学习笔记，可附原文选区。AI整理待本人确认，成功以Rust持久化回执为准。',
    parameters: Type.Object({ content: string(12000), selection: Type.Optional(Type.String({ maxLength: 12000 })) }, { additionalProperties: false }) },
];
export function validateCall(id, name, args) {
  if (!validId(id) || !known.has(name)) throw new LearningToolError('TOOLS_DISABLED');
  if (!args || typeof args !== 'object' || Array.isArray(args)) throw new LearningToolError('TOOL_PROTOCOL');
  bounded(args);
  const fields = name === 'record_question' ? { question: 8000, selection: 12000 }
    : name === 'create_error' ? { front: 8000, back: 12000, cause: 4000, exerciseId: 128, tags: 0 }
      : name === 'create_review_card' ? { front: 8000, back: 12000, tags: 0 }
        : name === 'create_note' ? { content: 12000, selection: 12000 } : { query: 500, knowledgeId: 128 };
  for (const [key, value] of Object.entries(args)) {
    if (!Object.hasOwn(fields, key)) throw new LearningToolError('TOOL_PROTOCOL');
    if (key === 'tags') {
      if (!Array.isArray(value) || value.length > 12 || value.some((tag) => typeof tag !== 'string' || !tag.trim() || [...tag].length > 64)) throw new LearningToolError('TOOL_PROTOCOL');
    } else if (typeof value !== 'string' || (!['selection', 'cause'].includes(key) && !value.trim()) || [...value].length > fields[key]) throw new LearningToolError('TOOL_PROTOCOL');
  }
  const required = name === 'record_question' ? ['question'] : ['create_error', 'create_review_card'].includes(name) ? ['front', 'back'] : name === 'create_note' ? ['content'] : [];
  if (required.some((key) => typeof args[key] !== 'string') || (name === 'query_related_knowledge' && !args.query && !args.knowledgeId)) throw new LearningToolError('TOOL_PROTOCOL');
  return args;
}
export function validateToolPolicy(input, legacyEnabled = false, operation = 'generate') {
  if (operation === 'test') return { mode: 'legacy', allowedTools: [], maxCalls: 0 };
  const policy = input ?? { mode: 'legacy', allowedTools: legacyEnabled ? [...LEGACY_TOOLS] : [], maxCalls: legacyEnabled ? 8 : 0 };
  if (!policy || typeof policy !== 'object' || Array.isArray(policy) || Object.keys(policy).some((key) => !['mode', 'allowedTools', 'maxCalls'].includes(key))) throw new LearningToolError('TOOL_PROTOCOL');
  const maximum = MODE_POLICIES[policy.mode];
  if (!maximum || !Array.isArray(policy.allowedTools) || policy.allowedTools.length > 5 || new Set(policy.allowedTools).size !== policy.allowedTools.length
    || policy.allowedTools.some((name) => !maximum.allowedTools.includes(name)) || !Number.isInteger(policy.maxCalls) || policy.maxCalls < 0 || policy.maxCalls > maximum.maxCalls
    || (policy.maxCalls === 0 && policy.allowedTools.length > 0)) throw new LearningToolError('TOOL_PROTOCOL');
  return { mode: policy.mode, allowedTools: [...policy.allowedTools], maxCalls: policy.maxCalls };
}
export function createLearningPlugin({ requestTool, secret = '', fail = () => {}, allowedTools = LEGACY_TOOLS, maxCalls = MAX_TOOL_CALLS }) {
  if (typeof requestTool !== 'function') throw new LearningToolError('TOOLS_DISABLED');
  if (!Array.isArray(allowedTools) || allowedTools.some((name) => !known.has(name)) || !Number.isInteger(maxCalls) || maxCalls < 1 || maxCalls > MAX_TOOL_CALLS) throw new LearningToolError('TOOL_PROTOCOL');
  const permitted = new Set(allowedTools);
  let attempts = 0;
  let executed = 0;
  const cache = new Map();
  const safeCall = (id, name, args) => {
    if ((secret && id?.includes(secret)) || !permitted.has(name)) throw new LearningToolError('TOOLS_DISABLED');
    const safeArgs = redactStrings(args, secret);
    validateCall(id, name, safeArgs);
    return { id, name, args: safeArgs };
  };
  const preflight = (toolCall, args) => {
    const call = safeCall(toolCall.id, toolCall.name, args);
    attempts++;
    if (attempts > maxCalls) throw new LearningToolError('TOOL_LIMIT');
    const cached = cache.get(call.id);
    if (cached && cached.fingerprint !== fingerprint(call.name, call.args)) throw new LearningToolError('TOOL_PROTOCOL');
    return call;
  };
  const execute = async (name, id, args, signal) => {
    if (signal?.aborted) throw new LearningToolError('CANCELLED');
    let call;
    try { call = safeCall(id, name, args); } catch (error) { fail(error.code); throw error; }
    const identity = fingerprint(name, call.args);
    const cached = cache.get(id);
    if (cached) {
      if (cached.fingerprint !== identity) { fail('TOOL_PROTOCOL'); throw new LearningToolError('TOOL_PROTOCOL'); }
      return cached.promise;
    }
    executed++;
    if (executed > maxCalls) { fail('TOOL_LIMIT'); throw new LearningToolError('TOOL_LIMIT'); }
    const promise = (async () => {
      let reply;
      try { reply = await requestTool(call, signal); }
      catch (error) {
        const code = error instanceof LearningToolError ? error.code : signal?.aborted ? 'CANCELLED' : 'TOOL_FAILED';
        if (code !== 'TOOL_FAILED') { fail(code); throw new LearningToolError(code); }
        reply = { error: { code: 'TOOL_FAILED' } };
      }
      if (signal?.aborted) throw new LearningToolError('CANCELLED');
      bounded(reply);
      if (!reply || typeof reply !== 'object' || (Object.hasOwn(reply, 'result') === Object.hasOwn(reply, 'error'))) { fail('TOOL_PROTOCOL'); throw new LearningToolError('TOOL_PROTOCOL'); }
      let body;
      let isError = false;
      if (Object.hasOwn(reply, 'error')) {
        isError = true;
        const code = typeof reply.error?.code === 'string' && /^[A-Z_]{1,64}$/u.test(reply.error.code) ? reply.error.code : 'TOOL_FAILED';
        body = { ok: false, error: { code, message: safeMessages.TOOL_FAILED } };
      } else {
        const result = redactStrings(reply.result, secret);
        if (!result || typeof result !== 'object' || Array.isArray(result)) { fail('TOOL_PROTOCOL'); throw new LearningToolError('TOOL_PROTOCOL'); }
        // Markdown prose is not durable execution evidence. Writes require Rust's explicit receipt.
        if (name !== 'query_related_knowledge' && result.persisted !== true) {
          isError = true; body = { ok: false, error: { code: 'TOOL_FAILED', message: safeMessages.TOOL_FAILED } };
        } else body = { ok: true, result };
      }
      body = redactStrings(body, secret);
      bounded(body);
      return { content: [{ type: 'text', text: JSON.stringify(body) }], details: { plugin: LEARNING_PLUGIN.id, version: LEARNING_PLUGIN.version, tool: name }, structuredContent: body, isError };
    })();
    cache.set(id, { fingerprint: identity, promise });
    return promise;
  };
  return {
    id: LEARNING_PLUGIN.id, version: LEARNING_PLUGIN.version,
    preflight,
    validate: (call) => safeCall(call.id, call.name, call.arguments),
    tools: allowedTools.map((name) => definitions.find((definition) => definition.name === name)).map((definition) => ({ ...definition, executionMode: 'sequential', replay: definition.name === 'query_related_knowledge' ? 'safe' : 'never',
      execute: (id, args, signal) => execute(definition.name, id, args, signal) })),
    stats: () => ({ attempts, executed }),
  };
}

/** One in-memory correlation table. No filesystem, external sessions, or process tools. */
export function createStdioToolBridge(emit, signal) {
  const pending = new Map();
  const completed = new Map();
  const rejectAll = () => { for (const item of pending.values()) item.reject(new LearningToolError('CANCELLED')); pending.clear(); };
  signal?.addEventListener('abort', rejectAll, { once: true });
  return {
    requestTool: async (call, toolSignal) => {
      if (signal?.aborted || toolSignal?.aborted) throw new LearningToolError('CANCELLED');
      if (pending.has(call.id) || completed.has(call.id)) throw new LearningToolError('TOOL_PROTOCOL');
      const response = new Promise((resolve, reject) => pending.set(call.id, { resolve, reject }));
      response.catch(() => {}); // Attach before awaiting stdout backpressure or cancellation.
      const cancel = () => { pending.get(call.id)?.reject(new LearningToolError('CANCELLED')); pending.delete(call.id); };
      toolSignal?.addEventListener('abort', cancel, { once: true });
      try { await emit({ type: 'tool_request', ...call }); return await response; }
      finally { pending.delete(call.id); toolSignal?.removeEventListener('abort', cancel); }
    },
    accept: (message) => {
      if (message?.type !== 'tool_result' || !validId(message.id)) throw new LearningToolError('TOOL_PROTOCOL');
      const { result, error } = message;
      const reply = Object.hasOwn(message, 'result') ? { result } : { error };
      bounded(reply);
      if (Object.hasOwn(message, 'result') === Object.hasOwn(message, 'error')) throw new LearningToolError('TOOL_PROTOCOL');
      const duplicate = completed.get(message.id);
      if (duplicate) { if (duplicate !== JSON.stringify(stable(reply))) throw new LearningToolError('TOOL_PROTOCOL'); return; }
      const item = pending.get(message.id);
      if (!item) throw new LearningToolError('TOOL_PROTOCOL');
      completed.set(message.id, JSON.stringify(stable(reply)));
      pending.delete(message.id); item.resolve(reply);
    },
    close: () => { rejectAll(); signal?.removeEventListener('abort', rejectAll); completed.clear(); },
  };
}
