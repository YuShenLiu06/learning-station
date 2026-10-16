import type { LearningSession, RequestContext } from './types';
export function assertExplorationScope(result: { session: Pick<LearningSession, 'sectionId'>; context: Pick<RequestContext, 'manifest'> }, associateCurrentSection: boolean, sectionId?: string) {
  if (!associateCurrentSection && (result.session.sectionId !== '' || result.context.manifest.length !== 0)) throw new Error('独立探索返回了关联材料，已停止发送。请重试打开独立会话。');
  if (associateCurrentSection && (!sectionId || result.session.sectionId !== sectionId)) throw new Error('探索会话与当前小节不匹配，已停止发送。');
}
export function validFontSize(value: number) { return Number.isInteger(value) && value >= 12 && value <= 24; }
