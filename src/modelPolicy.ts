import type { ModelSettings } from './types';
export const supportedProtocol = 'openai-completions';
type EndpointInput = { provider: string; protocol: string; baseUrl: string };
export function modelEndpointError(input: EndpointInput) {
  if (!['openai-completions', 'anthropic-messages'].includes(input.protocol)) return '当前仅支持 OpenAI Chat Completions 或 Anthropic Messages。';
  if (input.provider === 'glm-pi' && input.protocol !== supportedProtocol) return 'GLM Pi 专用通道仅支持 OpenAI Chat Completions；Anthropic Messages 请使用自定义模型服务。';
  if (!input.baseUrl.trim()) return '请填写模型服务的 base_url。';
  try {
    const raw = input.baseUrl.trim(); const url = new URL(raw);
    if (/[\u0000-\u0020\u007f]/u.test(raw)) return '服务地址中不能包含空格或控制字符。';
    if (url.username || url.password || raw.split('://')[1]?.split(/[/?#]/u)[0].includes('@') || raw.includes('?') || raw.includes('#')) return '服务地址不能包含账号、查询参数或片段；密钥请在专用输入框填写。';
    if (url.protocol !== 'https:' && !(url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname))) return '服务地址须使用 HTTPS；仅本机测试可使用 HTTP。';
    const path = url.pathname.replace(/\/+$/, '');
    if (input.protocol === 'anthropic-messages' && (path.endsWith('/v1') || path.endsWith('/messages'))) return 'Anthropic Messages 请填写根地址或代理前缀，不要以 /v1 或 /messages 结尾；应用会追加 /v1/messages?beta=true。';
    if (input.protocol === supportedProtocol && path.endsWith('/chat/completions')) return '请填写基础地址 base_url，不要包含 /chat/completions。';
  } catch { return '服务地址格式不正确，请填写完整的 base_url。'; }
  return '';
}
export function modelRequestTarget(input: EndpointInput) {
  if (modelEndpointError(input)) return '';
  const base = new URL(input.baseUrl.trim()).href.replace(/\/+$/, '');
  return `${base}${input.protocol === 'anthropic-messages' ? '/v1/messages?beta=true' : '/chat/completions'}`;
}
export function modelAccessReason(model: ModelSettings) {
  return model.usageGate ?? (model.provider === 'glm-pi' ? 'PLAN_USAGE_UNCONFIRMED' : '');
}
export const planUsageExplanation = '当前已保存配置使用 Coding Plan 专用通道，本学习应用的用途尚未确认，已在本机停止，未发送网络请求。可保留此配置等待确认，或自行填写具备普通 API 权限的服务地址；费用由所选服务决定。';
export function modelAccessMessage(model: ModelSettings) {
  const reason = modelAccessReason(model);
  return reason === 'PLAN_USAGE_UNCONFIRMED' ? planUsageExplanation : reason ? '当前已保存配置受到本地用途限制，请先核对服务的使用权限。' : '';
}
export function modelFailureMessage(error: unknown, operation: 'test' | 'generation' | 'settings') {
  const value = error && typeof error === 'object' ? error as { code?: unknown; message?: unknown } : null;
  if (value?.code === 'PLAN_USAGE_UNCONFIRMED') return `本地用途限制：${planUsageExplanation}`;
  const message = typeof value?.message === 'string' ? value.message : typeof error === 'string' ? error : '本机服务暂时没有响应，请重试。';
  const code = typeof value?.code === 'string' ? value.code : '';
  const category = ({ NETWORK: '网络请求未完成', TIMEOUT: '请求超时', AUTH: '服务认证未通过', RATE_LIMIT: '服务额度或频率受限', MODEL_HTTP: '服务返回错误', MODEL_RESPONSE: '响应解析未完成', INVALID_URL: '服务地址需要调整', INVALID_MODEL_SETTINGS: '本机配置需要调整' } as Record<string, string>)[code] || (operation === 'test' ? '测试未完成' : operation === 'settings' ? '设置未完成' : '生成未完成');
  return `${category}（学习站诊断${code ? ` · ${code}` : ''}）：${message}`;
}
export function modelBindingChanged(model: ModelSettings, input: { provider: string; protocol: string; baseUrl: string }) {
  return model.provider !== input.provider || (model.protocol || supportedProtocol) !== input.protocol || model.baseUrl !== input.baseUrl.trim();
}
export function modelSelectionError(name: string, current: ModelSettings) {
  if (!current.configured) return '请先在供应商设置中保存连接。';
  const value = name.trim();
  if (!value || new TextEncoder().encode(value).length > 256 || /[\u0000-\u001f\u007f]/u.test(value)) return '请输入有效的模型 ID（最多 256 字节，不含控制字符）。';
  if (current.provider === 'glm-pi' && value !== 'glm-5.3-flash') return 'GLM Pi 专用通道当前仅支持 glm-5.3-flash；其他模型请使用获准的自定义服务。';
  return '';
}
export function modelFormError(input: { provider: string; protocol: string; baseUrl: string; model: string; apiKey: string }, current: ModelSettings) {
  if (!['custom', 'glm-pi'].includes(input.provider)) return '请选择模型服务类型。';
  const endpointError = modelEndpointError(input);
  if (endpointError) return endpointError;
  if (!input.model.trim()) return '请填写服务端支持的模型名称。';
  if ((!current.credentialStored || modelBindingChanged(current, input)) && !input.apiKey.trim()) return '此服务地址或协议需要新密钥，请在本机输入后保存。';
  return '';
}
