import { readRequestedContext } from './requestedContext.ts';
import type { Message } from './types';

type Entry = { id: string; kind: string; content: string; createdAt: string; selection?: string; data?: Record<string, unknown> };
// Only persisted entries are displayed as records. Model prose never becomes a save acknowledgement.
export function sessionMessages(entries: Entry[]): Message[] {
  return entries.filter(entry => entry.kind === 'user' || entry.kind === 'assistant' || (entry.kind === 'note' && entry.data?.origin === 'pi-tool')).map(entry => ({
    id: entry.id,
    clientMessageId: entry.kind === 'user' && typeof entry.data?.clientMessageId === 'string' ? entry.data.clientMessageId : undefined,
    runId: entry.kind === 'assistant' && typeof entry.data?.runId === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(entry.data.runId) ? entry.data.runId : undefined,
    role: entry.kind === 'note' ? 'note' : entry.kind as 'user' | 'assistant',
    content: entry.content,
    createdAt: entry.createdAt,
    requestedContext: entry.kind === 'user' ? readRequestedContext(entry.data?.requestedContext, entry.selection || '') : undefined,

    context: entry.kind === 'assistant' && Array.isArray(entry.data?.contextManifest) ? { text: '', manifest: entry.data.contextManifest as NonNullable<Message['context']>['manifest'], historyManifest: Array.isArray(entry.data.historyManifest) ? entry.data.historyManifest as NonNullable<Message['context']>['manifest'] : [], warnings: Array.isArray(entry.data.contextWarnings) ? entry.data.contextWarnings.filter((value): value is string => typeof value === 'string') : [], truncated: entry.data.contextTruncated === true } : undefined,
    needsConfirmation: entry.kind === 'note' && entry.data?.needsConfirmation === true,
  }));
}
