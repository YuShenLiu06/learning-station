import type { ContextInjection } from './types';
import { StationError } from './types';

export function addContextInjection(items: ContextInjection[], item: ContextInjection) {
  if (item.selection !== undefined && Array.from(item.selection).length > 8000) throw new StationError('每份选段最多 8000 字符，请缩小选区。', 'CONTEXT_LIMIT');
  const next = items.filter(value => value.documentId !== item.documentId);
  if (next.length >= 8) throw new StationError('每次最多注入 8 份材料，请先移除一份。', 'CONTEXT_LIMIT');
  return [...next, { ...item }];
}
export const freezeInjections = (items: ContextInjection[]) => items.map(item => ({ ...item }));
