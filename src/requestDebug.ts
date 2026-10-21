export type DebugRecord = {
  sequence: number; previousSequence?: number; time: string; phase: string; protocol: string; url: string;
  method: string; headers: Record<string,string>; body: unknown; complete: boolean; omitted?: string; bodyBytes: number;
  status: string; runOutcome?: string;
  identity: { retry?: boolean; courseId?: string; sessionId: string; runId: string; requestId?: string; userEntryId?: string; attempt: string; model: string; operation?: string };
};
export type DebugSnapshot = { enabled: boolean; records: DebugRecord[]; dropped: number };
export const emptyDebug: DebugSnapshot = {enabled:false,records:[],dropped:0};
// Display only the captured payload; parse JSON message strings for readable Focus/citation/Changes.
export function requestText(record: DebugRecord): string {
  if(!record.complete)return `不完整：${record.omitted}`;
  const parts: string[]=[];
  function walk(value: unknown,path: string) {
    if(typeof value==='string') {
      try { const nested=JSON.parse(value);if(nested&&typeof nested==='object'){walk(nested,path+' (JSON)');return;} }catch{}
      parts.push(`${path}\n${value}`);return;
    }
    if(value&&typeof value==='object')for(const [key,item]of Object.entries(value))walk(item,`${path}/${key}`);
    else parts.push(`${path}\n${JSON.stringify(value)}`);
  }
  walk(record.body,'payload'); return parts.join('\n\n');
}
export function requestDiff(before: DebugRecord | undefined, after: DebugRecord): string {
  if(!before)return after.previousSequence ? `基线 #${after.previousSequence} 已从有界内存移除；无法比较。` : '此会话的首条实际请求，没有 diff 基线。';
  if(before.identity.sessionId!==after.identity.sessionId)return '不同会话，无法比较。';
  if(!before.complete||!after.complete)return '基线或当前请求不完整，无法提供完整 diff。';
  const a=requestText(before).split('\n'), b=requestText(after).split('\n');
  let start=0;while(start<a.length&&start<b.length&&a[start]===b[start])start++;
  if(start===a.length&&start===b.length)return '请求正文未变化。';
  let endA=a.length,endB=b.length;while(endA>start&&endB>start&&a[endA-1]===b[endB-1]){endA--;endB--;}
  return [`--- 请求 #${before.sequence}`,`+++ 请求 #${after.sequence}`,`@@ -${start+1},${endA-start} +${start+1},${endB-start} @@`,...a.slice(start,endA).map(line=>'- '+line),...b.slice(start,endB).map(line=>'+ '+line)].join('\n');
}
