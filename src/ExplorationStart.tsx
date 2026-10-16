import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react';
import { ArrowUp, Compass, LoaderCircle } from 'lucide-react';
import { call, nativeAvailable } from './api';
import { composerKeyAction } from './keyboard';
import type { UiDraft } from './types';
export type ExplorationStartHandle = { flush: () => Promise<void> };
export const ExplorationStart = forwardRef<ExplorationStartHandle, {
 entryId: string;
 onStart: (question: string, requestId: string, entryId: string) => Promise<{ created: boolean; complete: boolean }>;
}>(function ExplorationStart({ entryId, onStart }, ref) {
 const key = `exploration-launch:${entryId}`;
 const [question, setQuestion] = useState('');
 const [requestId, setRequestId] = useState('');
 const [busy, setBusy] = useState(false);
 const [error, setError] = useState('');
 const [saved, setSaved] = useState<UiDraft | null>(null);
 const [loaded, setLoaded] = useState(!nativeAvailable);
 const starting = useRef(false), composing = useRef(false), commitFrame = useRef(0);
 const edited = useRef(false), mounted = useRef(true);
 const state = useRef({ question: '', requestId: '', loaded: !nativeAvailable });
 const reading = useRef<Promise<void> | null>(null);
 function change(question: string, requestId = state.current.requestId) {
  state.current = { ...state.current, question, requestId };
  if (mounted.current) { setQuestion(question); setRequestId(requestId); }
 }
 function load(): Promise<void> {
  if (reading.current) return reading.current;
  reading.current = (async () => {
   state.current.loaded = false;
   if (mounted.current) setLoaded(false);
   try {
    const value = await call<UiDraft | null>('draft_get', { key });
    // Each visit has its own key. Never read or clear the legacy launch draft.
    if (value && (value.data.question || value.data.requestId)) {
     if (mounted.current) { setSaved(value); setError('此编号已有探索草稿，请先对照保留。'); }
     throw new Error('探索草稿尚未确认，已有内容保留。');
    }
    state.current.loaded = true;
    if (mounted.current) { setLoaded(true); setError(''); }
   } catch (error) {
    if (mounted.current) setError(error instanceof Error ? error.message : String(error));
    throw error;
   } finally { reading.current = null; }
  })();
  return reading.current;
 }
 async function flush() {
  if (!nativeAvailable || !edited.current) return;
  if (!state.current.loaded) await load();
  const value = state.current;
  await call('draft_save', { key, data: { question: value.question } });
 }
 useImperativeHandle(ref, () => ({ flush }), []);
 useEffect(() => {
  mounted.current = true;
  if (nativeAvailable) void load().catch(() => {});
  return () => { mounted.current = false; cancelAnimationFrame(commitFrame.current); };
 }, []);
 useEffect(() => {
  if (!loaded || !nativeAvailable || !edited.current) return;
  const timer = setTimeout(() => { void flush().catch(error => { if (mounted.current) setError(`探索草稿尚未备份：${error instanceof Error ? error.message : String(error)}`); }); }, 500);
  return () => clearTimeout(timer);
 }, [question, requestId, loaded]);
 async function start() {
  const value = state.current;
  if (starting.current || !value.question.trim() || !value.loaded) return;
  starting.current = true;
  // The visit ID is also the idempotent creation ID; it survives in the draft key.
  const id = value.requestId || entryId, text = value.question;
  change(text, id); edited.current = true; setBusy(true); setError('');
  try {
   await flush();
   const result = await onStart(text, id, entryId);
   if (result.created) { edited.current = false; change('', ''); }
  } catch (error) {
   if (mounted.current) setError(`${error instanceof Error ? error.message : String(error)} 原问题与本次编号保留；重试会核对同一份笔记，不会另建。`);
  } finally { starting.current = false; if (mounted.current) setBusy(false); }
 }
 return <section className="exploration-start"><div className="exploration-symbol"><Compass size={31} strokeWidth={1.4}/></div><span className="eyebrow">FOLLOW YOUR QUESTION</span><h1>今天想弄明白什么？</h1><p>从一个问题开始，边问边整理自己的理解。</p><div className="composer exploration-search"><textarea aria-label="想探索的问题" value={question} disabled={busy || !!requestId} onChange={event => { edited.current = true; change(event.target.value); }} rows={3} maxLength={4000} placeholder="例如：一个概念为什么会这样？" onCompositionStart={() => { composing.current = true; cancelAnimationFrame(commitFrame.current); }} onCompositionEnd={() => { commitFrame.current = requestAnimationFrame(() => { composing.current = false; }); }} onKeyDown={event => { const action = composerKeyAction(event.nativeEvent, composing.current); if (action === 'native') return; event.preventDefault(); if (action === 'send') void start(); }}/><div className="composer-bottom"><span>自动保存在“探索”文件夹 · 普通 Markdown 笔记</span><button className="send-button" aria-label={requestId ? '继续本次探索' : '开始探索'} disabled={busy || !loaded || !question.trim()} onClick={() => { void start(); }}>{busy ? <LoaderCircle size={18} className="spin"/> : <ArrowUp size={18}/>}</button></div></div>{requestId && <p className="muted small">沿用同一创建编号，先核对这次保存结果。</p>}{error && <div className="inline-error" role="alert">{error}{saved ? <><details><summary>查看找到的旧问题</summary><p>{saved.data.question || '（空）'}</p></details><button onClick={() => { edited.current = true; change(saved.data.question || '', entryId); state.current.loaded = true; setSaved(null); setLoaded(true); setError(''); }}>恢复旧草稿</button><button onClick={() => { state.current.loaded = true; setSaved(null); setLoaded(true); setError(''); }}>保留当前问题并备份</button></> : !loaded && <button onClick={() => { void load().catch(() => {}); }}>重试读取草稿</button>}</div>}</section>;
});
