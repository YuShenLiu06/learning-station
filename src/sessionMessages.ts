import type { Message, Source } from './types';

type Entry = { id: string; kind: string; content: string; createdAt: string; data?: Record<string, unknown> };
// Only persisted entries are displayed as records. Model prose never becomes a save acknowledgement.
export function sessionMessages(entries: Entry[], sources: Source[]): Message[] {
  return entries.filter(entry => entry.kind === 'user' || entry.kind === 'assistant' || (entry.kind === 'note' && entry.data?.origin === 'pi-tool')).map(entry => ({
    id: entry.id,
    role: entry.kind === 'note' ? 'note' : entry.kind as 'user' | 'assistant',
    content: entry.content,
    createdAt: entry.createdAt,
    sources: entry.kind === 'assistant' ? sources : undefined,
    needsConfirmation: entry.kind === 'note' && entry.data?.needsConfirmation === true,
  }));
}
