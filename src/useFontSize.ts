import { useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import { validFontSize } from './explorationPolicy';
export function useFontSize() {
  const [fontSize, setValue] = useState(16);
  const [error, setError] = useState('');
  const [status, setStatus] = useState('正在读取字号…');
  const record = useRef({ value: 16, saved: null as number | null, edited: false });
  const writes = useRef<Promise<unknown>>(Promise.resolve());
  async function load() {
    if (!nativeAvailable) { setStatus('开发预览字号仅保留在内存'); return; }
    try { const result = await call<{ fontSize: number }>('preferences_get'); if (!validFontSize(result.fontSize)) throw new Error('本机字号不在支持范围内。'); record.current.saved = result.fontSize; if (!record.current.edited) { record.current.value = result.fontSize; setValue(result.fontSize); } setError(''); setStatus('字号已从本机恢复'); } catch (error) { setError(error instanceof Error ? error.message : String(error)); setStatus('字号尚未读取成功'); }
  }
  async function flush() {
    if (!nativeAvailable || !record.current.edited || record.current.value === record.current.saved) return;
    const value = record.current.value;
    setStatus('正在保存字号…');
    const next = writes.current.catch(() => undefined).then(() => call<{ fontSize: number }>('preferences_save', { fontSize: value }));
    writes.current = next;
    try { await next; record.current.saved = value; setError(''); setStatus(record.current.value === value ? '字号已保存到本机' : '正在保存新字号…'); }
    catch (error) { const message = error instanceof Error ? error.message : String(error); setError(message); setStatus('字号尚未保存'); throw error; }
  }
  useEffect(() => { void load(); }, []);
  useEffect(() => { if (!record.current.edited) return; const timer = setTimeout(() => { void flush().catch(() => undefined); }, 250); return () => clearTimeout(timer); }, [fontSize]);
  return { fontSize, error, status, retry: load, flush, setFontSize(value: number) { if (!validFontSize(value)) return; record.current.value = value; record.current.edited = true; setValue(value); } };
}
