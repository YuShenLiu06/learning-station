export type DebugRecord = {
  sequence: number; previousSequence?: number; time: string; phase: string; protocol: string; url: string;
  method: string; headers: Record<string,string>; body: unknown; complete: boolean; omitted?: string; bodyBytes: number;
  status: string; runOutcome?: string;
  registrations?: {key:string;lifetime:string;position:string;value?:unknown;omitted?:string}[] | null;
  sources?: {path:(string|number)[];start?:number;end?:number;origin:string;name:string;registrationKey?:string;description?:string;sourcePath?:string}[];
  identity: { retry?: boolean; courseId?: string; sessionId: string; runId: string; requestId?: string; userEntryId?: string; attempt: string; model: string; operation?: string };
};
export type DebugSnapshot = { enabled: boolean; records: DebugRecord[]; dropped: number };
export const emptyDebug: DebugSnapshot = {enabled:false,records:[],dropped:0};
// Display only the captured payload; parse JSON message strings for readable Focus/citation/Changes.
export function payloadText(record: DebugRecord): string {
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
export function readableContent(value: unknown, path=''): string {
  if(typeof value==='string') {
    try{const nested=JSON.parse(value);if(nested&&typeof nested==='object')return readableContent(nested,path);}catch{}
    return `${path ? path+'\n' : ''}${value}`;
  }
  if(value&&typeof value==='object'&&Object.keys(value).length)return Object.entries(value).map(([key,item])=>readableContent(item,path?path+'/'+key:key)).join('\n\n');
  return `${path}\n${JSON.stringify(value)}`;
}
export function requestText(record: DebugRecord): string {
  if(!record.complete)return `不完整：${record.omitted}`;
  const body=record.body as {system?:unknown;messages?:{role?:string;content?:unknown}[]};
  const parts:string[]=[];
  if(body?.system!==undefined)parts.push('system\n'+readableContent(body.system));
  for(const [index,message]of (body?.messages??[]).entries())parts.push(`messages/${index} · ${message.role??'未知角色'}\n${readableContent(message.content)}`);
  return parts.join('\n\n')||'暂无可识别的 messages/system，请查看完整请求。';
}
export function sourceContent(record:DebugRecord, source:NonNullable<DebugRecord['sources']>[number]):string {
  let value:unknown=record.body;for(const key of source.path)value=(value as Record<string|number,unknown>)?.[key];
  if(typeof value==='string'&&source.start!==undefined)value=value.slice(source.start,source.end);
  return readableContent(value);
}
export function requestDiff(before: DebugRecord | undefined, after: DebugRecord): string {
  if(!before)return after.complete ? `${after.previousSequence ? `基线 #${after.previousSequence} 已从有界内存移除。` : ''}无前次记录，展示完整请求。\n${payloadText(after)}` : `无前次记录。当前请求不完整。\n${requestText(after)}`;
  if(before.identity.sessionId!==after.identity.sessionId)return '不同会话，无法比较。';
  if(!before.complete||!after.complete)return '基线或当前请求不完整，无法提供完整 diff。';
  const a=payloadText(before).split('\n'), b=payloadText(after).split('\n');
  let start=0;while(start<a.length&&start<b.length&&a[start]===b[start])start++;
  if(start===a.length&&start===b.length)return '请求正文未变化。';
  let endA=a.length,endB=b.length;while(endA>start&&endB>start&&a[endA-1]===b[endB-1]){endA--;endB--;}
  return [`--- 请求 #${before.sequence}`,`+++ 请求 #${after.sequence}`,`@@ -${start+1},${endA-start} +${start+1},${endB-start} @@`,...a.slice(start,endA).map(line=>'- '+line),...b.slice(start,endB).map(line=>'+ '+line)].join('\n');
}
