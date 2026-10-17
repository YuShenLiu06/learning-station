export type ToolAuthorizationTier = 'off' | 'read' | 'write' | 'bash';
export const TOOL_AUTHORIZATION_TIERS: readonly ToolAuthorizationTier[] = ['off', 'read', 'write', 'bash'];
export const TIER_LABELS: Record<ToolAuthorizationTier, string> = {
  off: '关闭',
  read: '受控读取',
  write: '受控读写',
  bash: '受控命令',
};
export const TIER_DESCRIPTIONS: Record<ToolAuthorizationTier, string> = {
  off: '默认。不注册 read/write/bash，模型不能读写文件或执行命令。',
  read: '注册 read：仅读取本轮已授权来源或当前会话工作区文件。',
  write: '在受控读取之上注册 write：仅写入当前会话工作区，不改动课程与学习记录。',
  bash: '在受控读写之上注册 bash：仅在会话工作区内执行命令，环境、超时与输出受限。',
};
export function isToolAuthorizationTier(value: unknown): value is ToolAuthorizationTier {
  return typeof value === 'string' && (TOOL_AUTHORIZATION_TIERS as readonly string[]).includes(value);
}
/** Untrusted or missing values stay closed: the default tier is always off. */
export function normalizeToolAuthorizationTier(value: unknown): ToolAuthorizationTier {
  return isToolAuthorizationTier(value) ? value : 'off';
}
