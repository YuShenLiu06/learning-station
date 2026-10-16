import { useCallback, useEffect, useRef, useState } from 'react';
import { assertExplorationScope } from './explorationPolicy';
import { call, nativeAvailable } from './api';
import type { LearningSession, RequestContext, UiDraft } from './types';
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
export function useIndependentExploration(enabled: boolean) {
  const [session, setSession] = useState<LearningSession | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [question, updateQuestion] = useState('');
  const [draftLoaded, setDraftLoaded] = useState(false);
  const [draftError, setDraftError] = useState('');
  const [conflict, setConflict] = useState<string | null>(null);
  const [recovery, setRecovery] = useState<{ status: string; content: string; message?: string }>();
  const input = useRef({ key: '', question: '', loaded: false, edited: false });
  const opening = useRef(false);
  const restoreDraft = useCallback(async (id: string) => {
    const key = `exploration:${id}`; input.current.key = key; input.current.loaded = false; setDraftLoaded(false);
    if (!nativeAvailable) { input.current.loaded = true; setDraftLoaded(true); return; }
    try {
      const saved = await call<UiDraft | null>('draft_get', { key });
      const previous = saved?.data.question ?? '';
      if (saved && input.current.edited && previous !== input.current.question) { setConflict(previous); setDraftError('发现不同的独立探索旧草稿，请先选择。'); return; }
      if (saved && !input.current.edited) { input.current.question = previous; updateQuestion(previous); }
      input.current.loaded = true; setDraftLoaded(true); setDraftError('');
    } catch (error) { setDraftError(message(error)); }
  }, []);
  const open = useCallback(async () => {
    if (opening.current) return; opening.current = true; setLoading(true); setError('');
    try {
      const result = await call<{ session: LearningSession; context: RequestContext; lastRunId?: string }>('exploration_open', { associateCurrentSection: false });
      assertExplorationScope(result, false);
      setSession(result.session); await restoreDraft(result.session.id);
      if (result.lastRunId && nativeAvailable) { const run = await call<{ status: string; content: string; error?: { message?: string } | string }>('run_get', { id: result.lastRunId }); if (run.status !== 'succeeded') setRecovery({ status: run.status, content: run.content, message: typeof run.error === 'string' ? run.error : run.error?.message }); }
    } catch (error) { setError(message(error)); }
    finally { opening.current = false; setLoading(false); }
  }, [restoreDraft]);
  useEffect(() => { if (enabled && !session) void open(); }, [enabled, session, open]);
  async function flush() {
    const value = input.current; if (!value.edited) return;
    if (!value.key || !value.loaded) throw new Error('独立探索草稿尚未读取成功，当前输入仍保留。');
    if (!nativeAvailable) return;
    try { await call('draft_save', { key: value.key, data: { question: value.question } }); setDraftError(''); } catch (error) { setDraftError(message(error)); throw error; }
  }
  useEffect(() => { if (!draftLoaded || !input.current.edited) return; const timer = setTimeout(() => { void flush().catch(() => undefined); }, 600); return () => clearTimeout(timer); }, [question, draftLoaded]);
  return { session, setSession, loading, error, recovery, question, draftError, conflict, retry: open, flush,
    retryDraft: () => session ? restoreDraft(session.id) : open(),
    resolveDraft(useSaved: boolean) { if (useSaved && conflict !== null) { input.current.question = conflict; updateQuestion(conflict); } input.current.loaded = true; input.current.edited = true; setDraftLoaded(true); setConflict(null); setDraftError(''); },
    setQuestion(value: string) { input.current.question = value; input.current.edited = true; updateQuestion(value); },
  };
}
