import { StationError } from './types.ts';
import type { ExerciseReference, ExerciseReferencePart } from './types';
export const exercisePartLabels: Record<ExerciseReferencePart, string> = { question: '题干', attempt: '我的已提交作答', feedback: '已公开反馈', reference: '已公开参考与解析' };
export const exerciseReferenceKey = (item: Pick<ExerciseReference, 'documentId' | 'questionId' | 'part'>) => `${item.documentId}:${item.questionId}:${item.part}`;
export const freezeExerciseReferences = (items: ExerciseReference[] = []) => items.map(item => ({ ...item }));
export function validateExerciseSelection(text: string, selection: string) {
 if (!selection || !text.includes(selection)) throw new StationError('显示选段与规范原文不完全相同，请从本分区原文重选。', 'CONTEXT_CONFLICT');
 if (Array.from(selection).length > 8000 || new TextEncoder().encode(selection).length > 30000) throw new StationError('每段最多 8000 字符、30000 字节，请缩小选段。', 'LIMIT_EXCEEDED');
}
export function addExerciseReference(items: ExerciseReference[], item: ExerciseReference, otherCount: number) {
 const kept = items.filter(value => exerciseReferenceKey(value) !== exerciseReferenceKey(item));
 if (kept.length + otherCount >= 8) throw new StationError('本次手动引用合计最多 8 份，请先移除一份。', 'LIMIT_EXCEEDED');
 return [...kept, { ...item }];
}
export function renderedExerciseSelection(card: HTMLElement, selection: Selection | null) {
 if (!selection || selection.isCollapsed) return;
 const element = (node: Node | null) => node?.nodeType === 1 ? node as Element : node?.parentElement;
 const anchor = element(selection.anchorNode), focus = element(selection.focusNode);
 if (!anchor || !focus) return;
 if (!card.contains(anchor) || !card.contains(focus)) { if (card.contains(anchor) || card.contains(focus)) throw new StationError('请只选同一道题的单个分区，不要跨题。', 'OUT_OF_SCOPE'); return; }
 const region = anchor.closest<HTMLElement>('[data-exercise-part]');
 if (!region || region !== focus.closest('[data-exercise-part]')) throw new StationError('请只选同一道题的单个分区，不要跨题干、作答或反馈。', 'OUT_OF_SCOPE');
 for (let parent: Element | null = anchor; parent && parent !== card; parent = parent.parentElement) if (parent.tagName === 'DETAILS' && !(parent as HTMLDetailsElement).open) return;
 const part = region.dataset.exercisePart as ExerciseReferencePart;
 if (!(part in exercisePartLabels)) return;
 return { part, entryId: region.dataset.entryId || undefined, selection: selection.toString() };
}
