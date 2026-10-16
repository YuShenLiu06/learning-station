import { useCallback, useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import type { UiDraft } from './types';
import { needsDraftChoice } from './draftPolicy';

type Input = { answer: string; reason: string; loaded: boolean; edited: boolean; error?: string };
const inputs = new Map<string, Input>();

function write(key: string, input: Input) {
  if (!nativeAvailable) return Promise.resolve();
  if (!input.loaded) return Promise.reject(new Error('练习草稿尚未读取成功，当前输入仍保留。'));
  return call('draft_save', { key, data: { question: input.answer, body: input.reason } }).then(result => { input.error = undefined; return result; }).catch(error => { input.error = error instanceof Error ? error.message : String(error); throw error; });
}
export function hasExerciseInput() { return [...inputs.values()].some(input => input.edited); }
export async function flushExerciseDrafts() {
  await Promise.all([...inputs.entries()].filter(([, input]) => input.edited).map(([key, input]) => write(key, input)));
}

export function useExerciseDraft(id: string, submittedAnswer: string, confirmedCause = '', causeUpdatedAt?: string) {
  const key = `exercise:${id}`;
  let input = inputs.get(key);
  if (!input) { input = { answer: submittedAnswer, reason: confirmedCause, loaded: !nativeAvailable, edited: false }; inputs.set(key, input); }
  const record = input;
  const [answer, updateAnswer] = useState(record.answer);
  const [reason, updateReason] = useState(record.reason);
  const [loaded, setLoaded] = useState(record.loaded);
  const [error, setError] = useState(record.error || '');
  const [conflict, setConflict] = useState<{ answer: string; reason: string } | null>(null);
  const alive = useRef(true);
  const restore = useCallback(async (force = false) => {
    if (!nativeAvailable || (record.loaded && !force)) return;
    record.loaded = false;
    setLoaded(false);
    try {
      const saved = await call<UiDraft | null>('draft_get', { key });
      const previous = saved ? { answer: saved.data.question ?? '', reason: saved.data.body ?? '' } : null;
      if (previous && needsDraftChoice(record, previous, record.edited)) {
        if (alive.current) { setConflict(previous); setError('发现不同的旧草稿，请先选择保留哪份。'); }
        return;
      }
      if (!record.edited && saved) { record.answer = saved.data.question ?? submittedAnswer; record.reason = causeUpdatedAt && Date.parse(saved.updatedAt) < Date.parse(causeUpdatedAt) ? confirmedCause : saved.data.body ?? confirmedCause; }
      record.loaded = true;
      if (alive.current) { updateAnswer(record.answer); updateReason(record.reason); setLoaded(true); setError(''); }
    } catch (error) { if (alive.current) setError(error instanceof Error ? error.message : String(error)); }
  }, [key, record, submittedAnswer, confirmedCause, causeUpdatedAt]);
  useEffect(() => { alive.current = true; void restore(); return () => { alive.current = false; if (record.edited && record.loaded) void write(key, record).catch(() => undefined); }; }, [key, record, restore]);
  useEffect(() => {
    if (!loaded || !record.edited) return;
    const timer = setTimeout(() => { void write(key, record).then(() => { if (alive.current) setError(''); }).catch(error => { if (alive.current) setError(error instanceof Error ? error.message : String(error)); }); }, 600);
    return () => clearTimeout(timer);
  }, [answer, reason, key, loaded, record]);
  return {
    answer, reason, error, conflict, retry: () => restore(true),
    resolveConflict(useSaved: boolean) {
      if (useSaved && conflict) { record.answer = conflict.answer; record.reason = conflict.reason; record.edited = false; updateAnswer(record.answer); updateReason(record.reason); }
      record.loaded = true; setLoaded(true); setConflict(null); setError('');
    },
    setAnswer(value: string) { record.answer = value; record.edited = true; updateAnswer(value); },
    setReason(value: string) { record.reason = value; record.edited = true; updateReason(value); },
  };
}
