import { lazy, Suspense } from 'react';
const Content = lazy(()=>import('./DebugJsonContent'));
export function DebugJsonView(props: {value:unknown;label:string}) {
  return <Suspense fallback={<p role="status">正在载入 JSON 查看器…</p>}><Content {...props}/></Suspense>;
}
