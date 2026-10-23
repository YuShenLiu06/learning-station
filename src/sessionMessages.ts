import { readToolActivity } from './toolActivities.ts';
import { readFocus } from './focus.ts';
import { readRequestedContext } from './requestedContext.ts';
import { readContextBridgeManifest } from './contextBridge.ts';
import type { Message } from './types';

type Entry = { id: string; kind: string; content: string; createdAt: string; selection?: string; data?: Record<string, unknown> };
// Only persisted entries are displayed as records. Model prose never becomes a save acknowledgement.
export function sessionMessages(entries: Entry[]): Message[] {
  const users = new Set(entries.filter(entry => entry.kind === 'user').map(entry => entry.id));
  const answered = new Set(entries.filter(entry => entry.kind === 'assistant').map(entry => entry.data?.runId));
  const receipts = entries.filter(entry => entry.kind === 'note' && entry.data?.origin === 'pi-tool-activity')
    .map(entry => ({ entry, tool: readToolActivity(entry.data?.toolActivity) }));
  const boundRuns = new Set(receipts.filter(({tool}) => tool?.userEntryId && users.has(tool.userEntryId)).map(({tool}) => tool!.runId));
  const orphanRuns = new Set<string>();
  const orphanEntries = new Map<string, string>();
  // Retain unresolvable old receipts at their recorded position, clearly unassigned.
  // Do not silently attach them to a nearby question or collect them at the bottom.
  for (const { entry, tool } of receipts) {
    if (!tool || boundRuns.has(tool.runId) || answered.has(tool.runId) || orphanRuns.has(tool.runId)) continue;
    orphanRuns.add(tool.runId); orphanEntries.set(entry.id, tool.runId);
  }
  return entries.filter(entry => orphanEntries.has(entry.id) || entry.kind === 'user' || entry.kind === 'assistant' || (entry.kind === 'note' && entry.data?.origin === 'pi-tool')).map(entry => ({
    id: entry.id,
    toolRunId: orphanEntries.get(entry.id),
    clientMessageId: entry.kind === 'user' && typeof entry.data?.clientMessageId === 'string' ? entry.data.clientMessageId : undefined,
    runId: entry.kind === 'assistant' && typeof entry.data?.runId === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(entry.data.runId) ? entry.data.runId : undefined,
    role: entry.kind === 'note' ? 'note' : entry.kind as 'user' | 'assistant',
    content: orphanEntries.has(entry.id) ? '' : entry.content,
    createdAt: entry.createdAt,
    requestedContext: entry.kind === 'user' ? readRequestedContext(entry.data?.requestedContext, entry.selection || '', entry.data?.citationContext) : undefined,

    context: entry.kind === 'assistant' && Array.isArray(entry.data?.contextManifest) ? { text: '', focus: readFocus(entry.data.focus), contextBridgeManifest: readContextBridgeManifest(entry.data.contextBridgeManifest), manifest: entry.data.contextManifest as NonNullable<Message['context']>['manifest'], historyManifest: Array.isArray(entry.data.historyManifest) ? entry.data.historyManifest as NonNullable<Message['context']>['manifest'] : [], assistantReferenceManifest: Array.isArray(entry.data.assistantReferenceManifest) ? entry.data.assistantReferenceManifest as NonNullable<Message['context']>['manifest'] : undefined, exerciseReferenceManifest: Array.isArray(entry.data.exerciseReferenceManifest) ? entry.data.exerciseReferenceManifest as NonNullable<Message['context']>['manifest'] : undefined, warnings: Array.isArray(entry.data.contextWarnings) ? entry.data.contextWarnings.filter((value): value is string => typeof value === 'string') : [], truncated: entry.data.contextTruncated === true } : undefined,
    needsConfirmation: entry.kind === 'note' && entry.data?.needsConfirmation === true,
  }));
}
