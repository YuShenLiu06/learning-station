import { lazy, Suspense } from 'react';
const Content = lazy(()=>import('./DebugJsonContent'));
export type DebugViewProps = {value?:unknown;text?:string;format?:'json'|'text'|'diff';label:string};
export function DebugJsonView(props: DebugViewProps) {
  return <Suspense fallback={<p role="status">正在载入只读查看器…</p>}><Content {...props}/></Suspense>;
}
