import type { ContextInjection, RequestedContext } from './types';
const validId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value);
const bytes = (value: string) => new TextEncoder().encode(value).length;
export function freezeRequestedContext(documentId: string, selection: string, injections: ContextInjection[] = []): RequestedContext {
 return { documentId, selection, injections: injections.map(item => ({ ...item })) };
}
// This is the persisted request intent, never a substitute for a Run's actual manifest.
export function readRequestedContext(value: unknown, entrySelection = ''): RequestedContext | undefined {
 if (!value || typeof value !== 'object' || Array.isArray(value)) return;
 const data = value as Record<string, unknown>;
 if (typeof data.documentId !== 'string' || data.documentId && !validId(data.documentId) || typeof data.selection !== 'string' || bytes(data.selection) > 30000 || data.selection && data.selection !== entrySelection || !Array.isArray(data.injections) || data.injections.length > 8) return;
 const injections: ContextInjection[] = [], seen = new Set<string>();
 for (const raw of data.injections) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
  const item = raw as Record<string, unknown>;
  if (!validId(item.documentId) || seen.has(item.documentId) || typeof item.expectedVersion !== 'string' || !item.expectedVersion || item.expectedVersion.length > 256 || item.selection !== undefined && (typeof item.selection !== 'string' || Array.from(item.selection).length > 8000)) return;
  seen.add(item.documentId);
  injections.push({ documentId: item.documentId, expectedVersion: item.expectedVersion, ...(item.selection === undefined ? {} : { selection: item.selection as string }) });
 }
 return { documentId: data.documentId, selection: data.selection, injections };
}
