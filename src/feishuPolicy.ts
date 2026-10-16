export type FeishuTestResult = { status: 'sent' | 'blocked' | 'failed' | 'unknown'; messageId: string | null; reason: string };
export function feishuTestResult(value: FeishuTestResult): FeishuTestResult {
  return value.status === 'sent' && !value.messageId ? { status: 'unknown', messageId: null, reason: '尚未收到消息回执，请先核对本次发送结果。' } : value;
}
export function feishuRequestId(currentId: string, result: FeishuTestResult | null, newId: string) { return currentId && result?.status === 'unknown' ? currentId : newId; }
