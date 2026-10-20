import type { ContextBridgeManifest } from './types';
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const count = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
// An absent or malformed old snapshot never becomes a verified empty list.
export function readContextBridgeManifest(value: unknown): ContextBridgeManifest | undefined {
 if (!record(value) || value.version !== 1 || !Array.isArray(value.entries) || !count(value.projectedChars) || !count(value.projectedBytes)) return;
 const entries: ContextBridgeManifest['entries'] = [];
 for (const item of value.entries) {
  if (!record(item) || typeof item.id !== 'string' || !['material','history','reference','turn','stable'].includes(String(item.kind)) || !['added','retained','updated','reloaded'].includes(String(item.status)) || item.sourceId !== undefined && typeof item.sourceId !== 'string' || typeof item.version !== 'string' || item.textHash !== undefined && typeof item.textHash !== 'string' || !count(item.includedChars) || typeof item.reason !== 'string') return;
  if (['path','fileName','fileStatus'].some(key => item[key] !== undefined && typeof item[key] !== 'string') || ['added','deleted'].some(key => item[key] !== undefined && item[key] !== null && !count(item[key])) || item.binary !== undefined && typeof item.binary !== 'boolean') return;
  let fixedVersion: ContextBridgeManifest['entries'][number]['fixedVersion'];
  if (item.fixedVersion !== undefined) {
   const fixed = item.fixedVersion;
   if (!record(fixed) || typeof fixed.courseId !== 'string' || typeof fixed.path !== 'string' || typeof fixed.checkpointCommitOid !== 'string' || !/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(fixed.checkpointCommitOid) || fixed.blobOid !== undefined && typeof fixed.blobOid !== 'string') return;
   fixedVersion = { courseId: fixed.courseId, checkpointCommitOid: fixed.checkpointCommitOid, path: fixed.path, ...(typeof fixed.blobOid === 'string' ? {blobOid:fixed.blobOid} : {}) };
  }
  const changes = { ...(typeof item.path === 'string' ? {path:item.path} : {}), ...(typeof item.fileName === 'string' ? {fileName:item.fileName} : {}), ...(typeof item.fileStatus === 'string' ? {fileStatus:item.fileStatus} : {}), ...(item.added === null || count(item.added) ? {added:item.added as number | null} : {}), ...(item.deleted === null || count(item.deleted) ? {deleted:item.deleted as number | null} : {}), ...(typeof item.binary === 'boolean' ? {binary:item.binary} : {}), ...(fixedVersion ? {fixedVersion} : {}) };
  let range: NonNullable<ContextBridgeManifest['entries'][number]['range']> | undefined;
  if (item.range !== undefined) {
   if (!record(item.range) || !count(item.range.startChar) || !count(item.range.endChar) || item.range.endChar < item.range.startChar || typeof item.range.complete !== 'boolean' || !count(item.range.totalBytes)) return;
   range = { startChar: item.range.startChar, endChar: item.range.endChar, complete: item.range.complete, totalBytes: item.range.totalBytes };
  }
  let locator: ContextBridgeManifest['entries'][number]['locator'];
  if (item.locator != null) {
   if (!record(item.locator) || !count(item.locator.startChar) || !count(item.locator.endChar) || item.includedChars === 0 || item.locator.endChar - item.locator.startChar !== item.includedChars) return;
   locator = { startChar: item.locator.startChar, endChar: item.locator.endChar };
  }
  entries.push({ ...changes, id: item.id, kind: item.kind as ContextBridgeManifest['entries'][number]['kind'], status: item.status as ContextBridgeManifest['entries'][number]['status'], ...(item.sourceId === undefined ? {} : {sourceId:item.sourceId}), version: item.version, ...(item.textHash === undefined ? {} : {textHash:item.textHash}), includedChars: item.includedChars, ...(range ? {range} : {}), ...(locator ? {locator} : {}), reason: item.reason });
 }
 return {version:1,entries,projectedChars:value.projectedChars,projectedBytes:value.projectedBytes};
}
