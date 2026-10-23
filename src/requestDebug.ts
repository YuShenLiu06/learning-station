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
export function sourceRound(record:DebugRecord,source:DebugSource):string {
  if(source.userEntryId)return source.userEntryId===record.identity.userEntryId?'本轮':'历史轮次';
  if(source.runId)return source.runId===record.identity.runId?'本轮':'历史轮次';
  return '轮次未定位';
}
export function sourceRole(record:DebugRecord,source:DebugSource):string {
  if(source.path[0]==='system')return 'system';
  if(source.path[0]!=='messages'||typeof source.path[1]!=='number')return '';
  const messages=(record.body as {messages?:{role?:string}[]})?.messages;
  return messages?.[source.path[1]]?.role??'';
}

export function isCurrentQuestion(record:DebugRecord,source:DebugSource):boolean {
  if(source.registrationKey!=='question')return false;
  if(source.userEntryId)return source.userEntryId===record.identity.userEntryId;
  if(source.runId)return source.runId===record.identity.runId;
  return (record.sources??[]).filter(s=>s.registrationKey==='question').length===1;
}
export function sourceReadingContent(record:DebugRecord,source:DebugSource):string {
  const raw=sourceValue(record,source);
  if(source.origin==='bridge'&&typeof raw==='string')try{
    const value=JSON.parse(raw) as {type?:string;frames?:{kind?:string;text?:unknown;unifiedDiff?:unknown}[]};
    if(value.type==='automatic_material_projection'&&Array.isArray(value.frames)&&value.frames.length) {
      const parts=value.frames.map(frame=>frame.kind==='full'?frame.text:frame.kind==='diff'?frame.unifiedDiff:undefined);
      if(parts.every((part):part is string=>typeof part==='string'))return parts.join('\n\n');
    }
  }catch{}
  return sourceContent(record,source);
}
export type DebugContentSection={title:string;text:string;format:'text'|'diff'};
export type DebugPresentation={facts:[string,string][];sections:DebugContentSection[];notice?:string};
export function sourcePresentation(record:DebugRecord,source:DebugSource):DebugPresentation|null {
 if(source.origin!=='bridge')return null;
 const raw=sourceValue(record,source);let value:Record<string,unknown>;
 try{value=(typeof raw==='string'?JSON.parse(raw):raw) as Record<string,unknown>;}catch{
  if(typeof raw==='string'&&source.registrationKey==='focus')return {facts:[],sections:[{title:'实发正文',text:raw,format:'text'}]};
  return null;
 }
 if(!value||typeof value!=='object'||Array.isArray(value))return null;
 const facts:[string,string][]=[];
 const field=(label:string,key:string)=>{if(typeof value[key]==='string'&&value[key])facts.push([label,value[key] as string]);};
 const range=(r:unknown)=>{if(!r||typeof r!=='object')return '';const v=r as Record<string,unknown>;return typeof v.startChar==='number'&&typeof v.endChar==='number'?`[${v.startChar}, ${v.endChar})${v.complete===true?' · 完整':v.complete===false?' · 部分':''}`:'';};
 if(value.type==='automatic_material_projection'){
  field('文件','path');field('冻结版本（旧协议）','version');
  const covered=range(value.range);if(covered)facts.push(['正文范围',covered]);
  const sections:DebugContentSection[]=[];
  for(const frame of Array.isArray(value.frames)?value.frames:[]){
   if(frame.kind==='full'&&typeof frame.text==='string')sections.push({title:'全文 · full',text:frame.text,format:'text'});
   if(frame.kind==='diff'&&typeof frame.unifiedDiff==='string')sections.push({title:'增量 · diff',text:frame.unifiedDiff,format:'diff'});
  }
  facts.push(['所属轮次',sourceRound(record,source)+(sourceRound(record,source)==='历史轮次'?' · 该基准仍在本次实际请求中':'')]);
  return {facts,sections,notice:sections.length?undefined:'此片段未记录全文或 diff，不能据此认定正文已发送或沿用。'};
 }
 if(source.name==='learning-station-focus'){
  field('当前文件','path');field('冻结 Git commit','commitHash');field('视图','view');
  const paragraphs=Array.isArray(value.paragraphs)?value.paragraphs:[];
  const covered=paragraphs.map(range).filter(Boolean);if(covered.length)facts.push(['可见范围',covered.join('，')]);
  const sections=paragraphs.filter(p=>typeof p.text==='string').map((p,index)=>({title:'可见正文'+(paragraphs.length>1?' '+(index+1):''),text:p.text as string,format:'text' as const}));
  return {facts,sections,notice:'此处是实际记录的聚焦位置；真实引用和正文基准分别以对应实发片段为准。'};
 }
 return null;
}

// Only actual wire fragments with explicit registration and turn identity qualify.
// A tool continuation belongs to the same saved user turn; never diff against the
// previous HTTP request (which may have been evicted or be a compaction request).
export function currentContextSources(record: DebugRecord): DebugSource[] {
  return orderedSources(record).filter(source => source.origin === 'bridge'
    && !!source.registrationKey && sourceRound(record, source) === '本轮');
}
export function currentContextSnapshot(record: DebugRecord) {
  return currentContextSources(record).map(source => ({...source, value: sourceValue(record, source)}));
}
// Whole messages retain tool calls/results, images and provider-specific blocks
// even when the optional text provenance capture could not map those fields.
export function completeContextSources(record: DebugRecord): DebugSource[] {
  const body = record.body as {system?: unknown; messages?: {role?: string}[]} | null;
  const sources: DebugSource[] = [];
  if (body?.system !== undefined) sources.push({path:['system'], origin:'unknown', name:'system'});
  for (const [index, message] of (body?.messages ?? []).entries()) {
    const mapped = (record.sources ?? []).filter(source => source.path[0] === 'messages' && source.path[1] === index);
    const names = [...new Set(mapped.map(source => source.name))];
    const owner = mapped.length === 1 ? mapped[0] : undefined;
    sources.push({...owner,path:['messages',index],start:undefined,end:undefined, origin:owner?.origin ?? (message.role === 'assistant' ? 'model' : ['tool','function'].includes(message.role ?? '') ? 'tool' : 'unknown'),
      name:message.role + (names.length ? ' · ' + names.join(' / ') : ' · 来源未定位')});
  }
  return sources;
}
