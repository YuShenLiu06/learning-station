export type DebugRecord = {
  sequence: number; previousSequence?: number; time: string; phase: string; protocol: string; url: string;
  method: string; headers: Record<string,string>; body: unknown; complete: boolean; omitted?: string; bodyBytes: number;
  status: string; runOutcome?: string;
  registrations?: {key:string;description?:string;lifetime?:string;position:string|number;value?:unknown;omitted?:string}[] | null;
  projectionRegistrations?: unknown[] | null; snapshotOmitted?:string;
  sources?: {path:(string|number)[];start?:number;end?:number;origin:string;name:string;registrationKey?:string;description?:string;runId?:string;userEntryId?:string;sourcePath?:string}[];
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
export function readableContent(value: unknown, path='', decode=true): string {
  if(typeof value==='string') {
    if(decode)try{const nested=JSON.parse(value);if(nested&&typeof nested==='object')return readableContent(nested,path,false);}catch{}
    return `${path ? path+'\n' : ''}${value}`;
  }
  if(value&&typeof value==='object'&&Object.keys(value).length)return Object.entries(value).map(([key,item])=>readableContent(item,path?path+'/'+key:key,false)).join('\n\n');
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
  return readableContent(sourceValue(record,source),'',source.origin==='bridge');
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

export type DebugSource = NonNullable<DebugRecord['sources']>[number];
export function orderedSources(record: DebugRecord): DebugSource[] {
  return [...(record.sources??[])].sort((a,b)=>{
    for(let i=0;i<Math.min(a.path.length,b.path.length);i++){
      const x=a.path[i],y=b.path[i]; if(x===y)continue;
      if(typeof x==='number'&&typeof y==='number')return x-y;
      if(x==='system')return -1;if(y==='system')return 1;
      return String(x).localeCompare(String(y));
    }
    return a.path.length-b.path.length||(a.start??0)-(b.start??0);
  });
}
export function sourceValue(record: DebugRecord, source: DebugSource): unknown {
  let value:unknown=record.body;for(const key of source.path)value=(value as Record<string|number,unknown>)?.[key];
  return typeof value==='string'&&source.start!==undefined?value.slice(source.start,source.end):value;
}
export function debugQuestion(record: DebugRecord): string {
  const question=record.registrations?.find(item=>item.key==='question')?.value;
  if(typeof question==='string'&&question)return question;
  const current=orderedSources(record).filter(s=>s.registrationKey==='question'&&(!s.userEntryId||s.userEntryId===record.identity.userEntryId)).at(-1);
  const value=current&&sourceValue(record,current);return typeof value==='string'?value:'';
}
export function debugPhase(record: DebugRecord): string {
  return record.phase==='tool_continuation'?'工具后续':record.phase==='initial'?(record.identity.retry?'重试':'首次发送'):'后续 / 压缩 / 校正';
}
export function sourceLabel(source: DebugSource): string {
  return ({pi:'Pi 原生',bridge:source.registrationKey?'桥 · 注册来源':'桥 · 后续拼装',user:'用户消息',tool:'工具返回',model:'模型回复',unknown:'未知 / 未映射'} as Record<string,string>)[source.origin]??source.origin;
}

export function isCurrentQuestion(record:DebugRecord,source:DebugSource):boolean {
  if(source.registrationKey!=='question')return false;
  if(source.userEntryId)return source.userEntryId===record.identity.userEntryId;
  if(source.runId)return source.runId===record.identity.runId;
  return (record.sources??[]).filter(s=>s.registrationKey==='question').length===1;
}
