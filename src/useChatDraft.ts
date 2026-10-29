import { useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import type { UiDraft } from './types';

type Draft = { question: string; loaded: boolean; edited: boolean; error?: string; conflict?: string };
// Chat input follows the stable conversation, independently of the reading page.
export function useChatDraft(sessionId: string) {
  const drafts = useRef(new Map<string, Draft>());
  const [, redraw] = useState(0);
  const refresh = () => redraw(value => value + 1);
  const pending = useRef(new Map<string, Promise<void>>());
  async function load(id: string) {
    if (!id || pending.current.has(id)) return;
    const draft = drafts.current.get(id) || { question: '', loaded: !nativeAvailable, edited: false };
    drafts.current.set(id, draft);
    if (draft.loaded) return;
    const job = (async () => {
      try {
        const saved = await call<UiDraft | null>('draft_get', { key: `chat:${id}` });
        if (!draft.edited) draft.question = saved?.data.question || '';
        else if (saved && saved.data.question && saved.data.question !== draft.question) { draft.conflict = saved.data.question; throw new Error('发现不同的已保存聊天草稿，请选择要保留的输入。'); }
        else if (saved?.data.question === draft.question) draft.edited = false;
        draft.loaded = true; draft.error = undefined;
      } catch (error) { draft.error = error instanceof Error ? error.message : String(error); }
      finally { pending.current.delete(id); refresh(); }
    })();
    pending.current.set(id, job); await job;
  }
  async function flush(id?: string) {
    for (const [key, draft] of drafts.current) {
      if (id && id !== key || !draft.edited) continue;
      if (!draft.loaded) throw new Error(draft.error || '聊天草稿尚未读取，输入仍保留。');
      const question = draft.question;
      try {
        if (nativeAvailable) await call('draft_save', { key: `chat:${key}`, data: { question } });
        if (draft.question === question) draft.edited = false;
      } catch (error) {
        // An absent acknowledgement may follow a successful write. The next
        // attempt must first read its authoritative version, never replay blindly.
        draft.loaded = false; draft.error = error instanceof Error ? error.message : String(error); refresh(); throw error;
      }
    }
  }
  useEffect(() => { void load(sessionId); }, [sessionId]);
  const current = drafts.current.get(sessionId);
  useEffect(() => {
    if (!sessionId || !current?.loaded || !current.edited) return;
    const timer = setTimeout(() => { void flush(sessionId).catch(error => { current.error = String(error); refresh(); }); }, 600);
    return () => clearTimeout(timer);
  }, [sessionId, current?.question, current?.loaded]);
  useEffect(() => () => { void flush(sessionId).catch(error => { const draft = drafts.current.get(sessionId); if (draft) draft.error = String(error); refresh(); }); }, [sessionId]);
  return {
    question: current?.question || '', loaded: !!current?.loaded, error: current?.error, conflict: current?.conflict, flush,
    retry: async () => { if (!current?.loaded) return load(sessionId); try { await flush(sessionId); current.error = undefined; } catch (error) { current.error = String(error); } refresh(); },
    resolve(useSaved: boolean) { if (!current) return; if (useSaved) current.question = current.conflict || ''; current.conflict = undefined; current.loaded = true; current.edited = true; current.error = undefined; refresh(); },
    setQuestion(question: string | ((current: string) => string), id = sessionId) {
      const draft = drafts.current.get(id) || { question: '', loaded: !nativeAvailable, edited: false };
      draft.question = typeof question === 'function' ? question(draft.question) : question; draft.edited = true; drafts.current.set(id, draft); refresh();
      if (id !== sessionId) void flush(id).catch(error => { draft.error = String(error); refresh(); });
    },
  };
}
