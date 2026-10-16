// Display labels only; delivery status is owned by the Core notification ledger.
export function noticeBody(status?: string): string | undefined {
  switch (status) {
    case 'received': return '本人飞书消息，仅保存为通知，未自动执行';
    case 'sent': return '飞书已确认发送';
    case 'failed': return '飞书发送失败，不自动重发';
    case 'unknown': return '飞书发送结果未知，请先核对消息，不自动重发';
    case 'pending': return '站内提醒，尚未确认外部发送';
    case 'disabled': return '站内提醒 · 飞书未启用';
    default: return undefined;
  }
}
