import { useEffect, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import { defaultSidebarWidth, validSidebarWidth } from './sidebarWidth';
export function useSidebarWidth() {
  const [width, setValue] = useState(defaultSidebarWidth);
  const [error, setError] = useState('');
  const state = useRef({ value: defaultSidebarWidth, saved: defaultSidebarWidth, edited: false, loaded: !nativeAvailable });
  const writes = useRef<Promise<unknown>>(Promise.resolve());
  async function load() {
    try {
      const result = await call<{sidebarWidth?:number}>('preferences_get');
      const value = result.sidebarWidth ?? defaultSidebarWidth;
      if (!validSidebarWidth(value)) throw new Error('已保存的目录宽度无效。');
      state.current.loaded = true; state.current.saved = value;
      if (!state.current.edited) { state.current.value = value; setValue(value); }
      setError('');
      if (state.current.edited) await flush();
    } catch (error) { setError(error instanceof Error ? error.message : String(error)); throw error; }
  }
  useEffect(() => { if (nativeAvailable) void load().catch(() => undefined); }, []);
  async function flush() {
    if (!nativeAvailable || !state.current.edited) return;
    if (!state.current.loaded) throw new Error('目录宽度尚未读取成功，当前调整仍保留。');
    const value = state.current.value;
    const next = writes.current.catch(() => undefined).then(async () => {
      if (value !== state.current.saved) { await call('preferences_save', {sidebarWidth:value}); state.current.saved = value; }
    }); writes.current = next;
    try { await next; setError(''); } catch (error) { setError(error instanceof Error ? error.message : String(error)); throw error; }
  }
  useEffect(() => { if (!state.current.edited) return; const timer=setTimeout(()=>{void flush().catch(error=>setError(error instanceof Error?error.message:String(error)));},250);return()=>clearTimeout(timer); },[width]);
  return {width,error,flush,async retry(){await load();await flush();},setWidth(value:number){if(!validSidebarWidth(value))return;state.current.value=value;state.current.edited=true;setValue(value);}};
}
