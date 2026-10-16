// Diagnostic observer only: never changes response bytes or persists body/header values.
const finishes = new Set(['stop', 'end', 'length', 'tool_calls', 'function_call', 'content_filter', 'network_error', 'end_turn', 'max_tokens', 'stop_sequence', 'tool_use', 'refusal']);
const mimeKinds = new Set(['sse', 'html', 'json', 'plain', 'other', 'missing']);
const paths = new Set(['/chat/completions', '/v1/chat/completions', '/v1/messages?beta=true', 'custom-base/chat/completions', 'custom-base/v1/messages?beta=true']);
const numbers = ['requests', 'httpStatus', 'maxTokens', 'transportBytes', 'providerEvents', 'textChars', 'thinkingChars', 'textEvents', 'thinkingEvents', 'usageOutputTokens', 'usageReasoningTokens'];
const flags = ['sawDone', 'cleanEof', 'messageStop', 'invalidDelta'];
export function safeResponseDiagnostic(input) {
  if (!input || typeof input !== 'object') return undefined;
  const result = { version: 1 };
  if (['test', 'generate'].includes(input.operation)) result.operation = input.operation;
  if (['openai-completions', 'anthropic-messages'].includes(input.protocol)) result.protocol = input.protocol;
  if (paths.has(input.requestPath)) result.requestPath = input.requestPath;
  if (mimeKinds.has(input.contentType)) result.contentType = input.contentType;
  if (finishes.has(input.finishReason) || ['missing', 'other'].includes(input.finishReason)) result.finishReason = input.finishReason;
  for (const key of numbers) if (Number.isSafeInteger(input[key]) && input[key] >= 0 && input[key] <= 1_000_000_000) result[key] = input[key];
  for (const key of flags) if (typeof input[key] === 'boolean') result[key] = input[key];
  return result;
}
function finish(value) { return finishes.has(value) ? value : value ? 'other' : 'missing'; }
function mime(value) {
  const type = String(value ?? '').split(';')[0].trim().toLowerCase();
  return !type ? 'missing' : type === 'text/event-stream' ? 'sse' : type === 'text/html' || type === 'application/xhtml+xml' ? 'html' : type === 'application/json' || type.endsWith('+json') ? 'json' : type === 'text/plain' ? 'plain' : 'other';
}
export function createResponseDiagnostic(operation, protocol) {
  let state = { operation, protocol, requests: 0, finishReason: 'missing', textChars: 0, thinkingChars: 0, providerEvents: 0, transportBytes: 0, sawDone: false, cleanEof: false, messageStop: false, invalidDelta: false };
  let decoder, tail = '';
  return {
    begin(endpoint, maxTokens) {
      const requests = state.requests + 1;
      state = { operation, protocol, requests, maxTokens, finishReason: 'missing', textChars: 0, thinkingChars: 0, providerEvents: 0, transportBytes: 0, sawDone: false, cleanEof: false, messageStop: false, invalidDelta: false };
      const path = new URL(endpoint).pathname;
      state.requestPath = protocol === 'anthropic-messages' ? path === '/' || path === '' ? '/v1/messages?beta=true' : 'custom-base/v1/messages?beta=true' : path === '/' || path === '' ? '/chat/completions' : path === '/v1' ? '/v1/chat/completions' : 'custom-base/chat/completions';
      decoder = new TextDecoder(); tail = '';
    },
    headers(response) { state.httpStatus = response.status; state.contentType = mime(response.headers.get('content-type')); },
    bytes(value) { state.transportBytes += value.byteLength; const text = tail + decoder.decode(value, { stream: true }); if (/(?:^|\n)data: *\[DONE\](?:\r?\n|$)/u.test(text)) state.sawDone = true; tail = text.slice(-64); },
    end() { const text = tail + decoder.decode(); if (/(?:^|\n)data: *\[DONE\](?:\r?\n|$)/u.test(text)) state.sawDone = true; tail = ''; state.cleanEof = true; },
    chunk(chunk) {
      state.providerEvents++;
      if (protocol === 'anthropic-messages') {
        if (chunk?.type === 'message_stop') state.messageStop = true;
        if (chunk?.delta?.stop_reason) state.finishReason = finish(chunk.delta.stop_reason);
        if (chunk?.delta?.type === 'text_delta' && typeof chunk.delta.text === 'string') { state.textChars += chunk.delta.text.length; state.textEvents = (state.textEvents ?? 0) + 1; }
        if (chunk?.delta?.type === 'thinking_delta' && typeof chunk.delta.thinking === 'string') { state.thinkingChars += chunk.delta.thinking.length; state.thinkingEvents = (state.thinkingEvents ?? 0) + 1; }
        if (Number.isSafeInteger(chunk?.usage?.output_tokens)) state.usageOutputTokens = chunk.usage.output_tokens;
        return;
      }
      const choice = chunk?.choices?.[0];
      if (choice?.finish_reason) state.finishReason = finish(choice.finish_reason);
      const delta = choice?.delta;
      if (delta?.content !== undefined && delta.content !== null && typeof delta.content !== 'string') state.invalidDelta = true;
      if (typeof delta?.content === 'string' && delta.content) { state.textChars += delta.content.length; state.textEvents = (state.textEvents ?? 0) + 1; }
      const thought = ['reasoning_content', 'reasoning', 'reasoning_text'].map(key => delta?.[key]).find(value => typeof value === 'string' && value);
      if (thought) { state.thinkingChars += thought.length; state.thinkingEvents = (state.thinkingEvents ?? 0) + 1; }
      const usage = chunk?.usage ?? choice?.usage;
      if (Number.isSafeInteger(usage?.completion_tokens)) state.usageOutputTokens = usage.completion_tokens;
      if (Number.isSafeInteger(usage?.completion_tokens_details?.reasoning_tokens)) state.usageReasoningTokens = usage.completion_tokens_details.reasoning_tokens;
    },
    snapshot() { return safeResponseDiagnostic(state); },
  };
}
export function responseFailureCode(diagnostic) {
  if (!diagnostic?.requests) return 'MODEL_RESPONSE';
  if (diagnostic.contentType === 'html') return 'MODEL_ENDPOINT_PAGE';
  if (diagnostic.contentType === 'json' || diagnostic.invalidDelta) return 'MODEL_FORMAT';
  if (['length', 'max_tokens'].includes(diagnostic.finishReason)) return 'MODEL_LENGTH';
  if (diagnostic.finishReason === 'missing') return 'MODEL_INCOMPLETE';
  if (diagnostic.protocol === 'anthropic-messages' && !diagnostic.messageStop) return 'MODEL_INCOMPLETE';
  if (!diagnostic.textChars && diagnostic.thinkingChars) return 'MODEL_THINKING_ONLY';
  if (!diagnostic.textChars) return 'MODEL_EMPTY';
  return 'MODEL_RESPONSE';
}
