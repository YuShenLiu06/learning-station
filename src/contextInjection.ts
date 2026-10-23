import { cloneCitation, validateCitation } from './citation.ts';
import type { ContextInjection } from './types';
import { StationError } from './types';

export function addContextInjection(items: ContextInjection[], item: ContextInjection, assistantCount = 0) {
  if (item.selection !== undefined) validateCitation(item.selection, item.locator);
  const next = items.filter(value => value.documentId !== item.documentId);
  if (next.length + assistantCount >= 8) throw new StationError('每次最多注入 8 份材料，请先移除一份。', 'CONTEXT_LIMIT');
  return [...next, cloneCitation(item)];
}
export const freezeInjections = (items: ContextInjection[]) => items.map(cloneCitation);
