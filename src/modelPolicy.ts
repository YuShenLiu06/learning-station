import type { ModelSettings } from './types';
export const supportedProtocol = 'openai-completions';
export function modelAccessReason(model: ModelSettings) {
  return model.usageGate ?? (model.provider === 'glm-pi' ? 'PLAN_USAGE_UNCONFIRMED' : '');
}
export const planUsageExplanation = '当前已保存配置使用 Coding Plan 专用通道，本学习应用的用途尚未确认，已在本机停止，未发送网络请求。可保留此配置等待确认，或自行填写具备普通 API 权限的服务地址；费用由所选服务决定。';
export function modelAccessMessage(model: ModelSettings) {
  const reason = modelAccessReason(model);
  return reason === 'PLAN_USAGE_UNCONFIRMED' ? planUsageExplanation : reason ? '当前已保存配置受到本地用途限制，请先核对服务的使用权限。' : '';
}
export function modelFailureMessage(error: unknown, operation: 'test' | 'generation') {
  const value = error && typeof error === 'object' ? error as { code?: unknown; message?: unknown } : null;
  if (value?.code === 'PLAN_USAGE_UNCONFIRMED') return `本地用途限制：${planUsageExplanation}`;
  const message = typeof value?.message === 'string' ? value.message : typeof error === 'string' ? error : '本机服务暂时没有响应，请重试。';
  return operation === 'test' ? `测试未完成：${message}` : message;
}
export function modelBindingChanged(model: ModelSettings, input: { provider: string; protocol: string; baseUrl: string }) {
  return model.provider !== input.provider || (model.protocol || supportedProtocol) !== input.protocol || model.baseUrl !== input.baseUrl.trim();
}
export function modelFormError(input: { provider: string; protocol: string; baseUrl: string; model: string; apiKey: string }, current: ModelSettings) {
  if (!['custom', 'glm-pi'].includes(input.provider)) return '请选择模型服务类型。';
  if (input.protocol !== supportedProtocol) return '当前仅支持 OpenAI Chat Completions（openai-completions）。';
  if (!input.baseUrl.trim()) return '请填写模型服务的 base_url。';
  try { const url = new URL(input.baseUrl); if (url.pathname.replace(/\/$/, '').endsWith('/chat/completions')) return '请填写基础地址 base_url，不要包含 /chat/completions。'; if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password) return '请填写 http 或 https 服务地址，地址中不要包含密钥。'; } catch { return '服务地址格式不正确，请填写完整的 base_url。'; }
  if (!input.model.trim()) return '请填写服务端支持的模型名称。';
  if ((!current.credentialStored || modelBindingChanged(current, input)) && !input.apiKey.trim()) return '此服务地址或协议需要新密钥，请在本机输入后保存。';
  return '';
}
