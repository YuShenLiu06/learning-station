import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { unlink } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

test('native course views share chat, preserve legacy exercise ownership and freeze request document across navigation', async () => {
  const dom = new JSDOM('', { url: 'http://127.0.0.1/' }); globalThis.window = dom.window;
  const { mockIPC, clearMocks } = await import('@tauri-apps/api/mocks');
  const entry = { id: 'old_exercise', kind: 'exercise', content: 'Question', data: { exercises: [{ id: 'q_old', question: 'Old question?', answers: ['answer'] }] } };
  const course = { id: 'course_chat_a', courseId: 'course_a', sectionId: 'note_a1', version: 'v1', entries: [{ id: 'chat_a', kind: 'user', content: 'Course A shared chat' }], contextManifest: [], legacySessionIds: ['legacy_a2'] };
  const legacy = { id: 'legacy_a2', sectionId: 'note_a2', version: 'v1', entries: [entry], contextManifest: [] };
  const other = { ...course, id: 'course_chat_b', courseId: 'course_b', sectionId: 'note_b1', entries: [] };
  const sessions = new Map([course, legacy, other].map(s => [s.id, s]));
  const starts = [], saves = []; let unblock; const gate = new Promise(resolve => { unblock = resolve; });
  mockIPC(async (command, args = {}) => {
    if (command === 'start_session') return { session: structuredClone(args.sectionId === 'note_b1' ? other : course) };
    if (command === 'get_session') return structuredClone(sessions.get(args.id));
    if (command === 'open_document') return { id: 'exercise_old', path: 'exercise.md', metadata: { title: 'Legacy exercise', kind: 'exercise' }, body: '', version: 'v1' };
    if (command === 'get_exercise_document') return { session: structuredClone(legacy) };
    if (command === 'append_entry') { saves.push(args); const s = sessions.get(args.sessionId); s.entries.push({ ...args.entry, id: 'saved_user' }); return structuredClone(s); }
    if (command === 'start_generation') { starts.push(args); await gate; return {}; }
    if (command === 'get_run') return { id: args.runId, sessionId: course.id, status: 'succeeded', content: '', toolCalls: [], contextSnapshotVersion: 1, contextManifest: [] };
    throw Error(command);
  });
  const file = new URL(`./.course-session-${process.pid}.mjs`, import.meta.url);
  await build({ entryPoints: ['src/native.ts'], outfile: file.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm' });
  try {
    const { nativeCall } = await import(file.href);
    const a = await nativeCall('session_open', { sectionId: 'note_a1' });
    const b = await nativeCall('session_open', { sectionId: 'note_a2' });
    assert.equal(a.id, b.id); assert.equal(b.sectionId, 'note_a2'); assert.deepEqual(a.messages, b.messages);
    await nativeCall('section_open', { id: 'exercise_old' });
    const ex = await nativeCall('session_open', { sectionId: 'exercise_old' });
    assert.equal(ex.id, course.id); assert.equal(ex.exerciseSessionId, legacy.id); assert.equal(ex.exercises[0].sessionId, legacy.id);
    const saved = await nativeCall('message_save', { sectionId: 'exercise_old', documentId: 'exercise_old', sessionId: ex.id, question: 'Explain my attempt', selection: '', mode: 'explain', sendId: 'saved_send' });
    assert.equal(saves[0].sessionId, course.id); assert.equal(saves[0].entry.data.requestedContext.documentId, 'exercise_old');
    const pending = nativeCall('session_ask', { sectionId: 'exercise_old', documentId: 'exercise_old', sessionId: ex.id, question: 'Explain my attempt', userEntryId: saved.userEntryId, operationId: 'frozen_operation' });
    await new Promise(resolve => setImmediate(resolve));
    assert.notEqual((await nativeCall('session_open', { sectionId: 'note_b1' })).id, ex.id);
    unblock(); await pending;
    assert.equal(starts[0].sessionId, course.id); assert.equal(starts[0].documentId, 'exercise_old');
  } finally { clearMocks(); dom.window.close(); await unlink(file).catch(() => {}); }
});
