import { useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import { defaultAssistantWidth, validAssistantWidth } from './assistantWidth';
export function useAssistantWidth() {
  const [width, setValue] = useState(defaultAssistantWidth);
  const [error, setError] = useState('');
  const record = useRef({ value: defaultAssistantWidth, saved: null as number | null, edited: false });
  const writes = useRef<Promise<unknown>>(Promise.resolve());
  useEffect(() => { if (nativeAvailable) void call<{ assistantWidth?: number }>('preferences_get').then(value => { const saved = value.assistantWidth ?? defaultAssistantWidth; if (!validAssistantWidth(saved)) throw new Error('已保存的面板宽度无效。'); record.current.saved = saved; if (!record.current.edited) { record.current.value = saved; setValue(saved); } }).catch(error => setError(error instanceof Error ? error.message : String(error))); }, []);
  async function flush() {
    if (!nativeAvailable || !record.current.edited) return;
    const value = record.current.value;
    const next = writes.current.catch(() => undefined).then(async () => { if (value !== record.current.saved) { await call('preferences_save', { assistantWidth: value }); record.current.saved = value; } }); writes.current = next;
    try { await next; setError(''); }
    catch (error) { setError(error instanceof Error ? error.message : String(error)); throw error; }
  }
  useEffect(() => { if (!record.current.edited) return; const timer = setTimeout(() => { void flush().catch(() => undefined); }, 250); return () => clearTimeout(timer); }, [width]);
  return { width, error, flush, setWidth(value: number) { if (!validAssistantWidth(value)) return; record.current.value = value; record.current.edited = true; setValue(value); } };
}
