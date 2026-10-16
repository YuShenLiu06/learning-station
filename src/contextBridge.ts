import type { ContextBridgeManifest } from './types';
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
// An absent or malformed old snapshot never becomes a verified empty list.
export function readContextBridgeManifest(value: unknown): ContextBridgeManifest | undefined {
 if (!record(value) || value.version !== 1 || !Array.isArray(value.entries) || !count(value.projectedChars) || !count(value.projectedBytes)) return;
 const entries: ContextBridgeManifest['entries'] = [];
 for (const item of value.entries) {
  if (!record(item) || typeof item.id !== 'string' || !['material','history','reference','turn','stable'].includes(String(item.kind)) || !['added','retained','updated','reloaded'].includes(String(item.status)) || item.sourceId !== undefined && typeof item.sourceId !== 'string' || typeof item.version !== 'string' || item.textHash !== undefined && typeof item.textHash !== 'string' || !count(item.includedChars) || typeof item.reason !== 'string') return;
  let range: NonNullable<ContextBridgeManifest['entries'][number]['range']> | undefined;
  if (item.range !== undefined) {
   if (!record(item.range) || !count(item.range.startChar) || !count(item.range.endChar) || item.range.endChar < item.range.startChar || typeof item.range.complete !== 'boolean' || !count(item.range.totalBytes)) return;
   range = { startChar: item.range.startChar, endChar: item.range.endChar, complete: item.range.complete, totalBytes: item.range.totalBytes };
  }
  entries.push({ id: item.id, kind: item.kind as ContextBridgeManifest['entries'][number]['kind'], status: item.status as ContextBridgeManifest['entries'][number]['status'], ...(item.sourceId === undefined ? {} : {sourceId:item.sourceId}), version: item.version, ...(item.textHash === undefined ? {} : {textHash:item.textHash}), includedChars: item.includedChars, ...(range ? {range} : {}), reason: item.reason });
 }
 return {version:1,entries,projectedChars:value.projectedChars,projectedBytes:value.projectedBytes};
}
