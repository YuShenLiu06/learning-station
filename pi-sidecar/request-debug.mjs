// Only sanitized data leaves the sending process. Never read credential files.
export const DEBUG_BODY_LIMIT = 256 * 1024;
export const DEBUG_RECORD_LIMIT = 3 * 1024 * 1024;
const sensitive = key => /(?:authorization|cookie|api[-_]?key|token|password|passwd|secret|credential)$/i.test(key.replace(/([a-z])([A-Z])/g,'$1_$2'));
const assignment=/(?:^|[^a-z0-9_])["']?(?:api[-_ ]?key|[a-z_-]{0,48}token|[a-z_-]{0,48}password|passwd|[a-z_-]{0,48}secret|cookie)["']?\s*[:=]\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^\s,;"'<>}]+))/gi;
export function redactDebug(value, apiKey = '', extraSecrets = []) {
  const secrets=new Set([...(apiKey?[apiKey]:[]),...extraSecrets].filter(Boolean));
  function scan(input) {
    const normalized=input.replace(/\\"/g,'"');
    for(const match of normalized.matchAll(assignment)) {const raw=match[1]??match[2]??match[3];if(raw) {secrets.add(raw);try{secrets.add(JSON.parse('"'+raw+'"'));}catch{}}}
    for(const match of normalized.matchAll(/(?:authorization|proxy-authorization|cookie|set-cookie)["']?\s*[:=]\s*([^\r\n]+)/gi)) {
      const raw=match[1].trim();if(raw)secrets.add(raw);
      for(const quoted of raw.matchAll(/["']([^"']+)["']/g))secrets.add(quoted[1]);
      for(const part of raw.split(/[;,]/)){const at=part.indexOf('=');const value=(at>=0?part.slice(at+1):part.replace(/^\S+\s+/, '')).trim().replace(/^["']|["']$/g,'');if(value)secrets.add(value);}
    }
    for(const match of normalized.matchAll(/\b(?:Bearer|Basic)\s+([a-z0-9+/_=.:-]+)/gi))secrets.add(match[1]);
  }
  function gather(item) {
    if(typeof item==='string'){scan(item);try{const nested=JSON.parse(item);if(nested&&typeof nested==='object')gather(nested);}catch{}return;}
    if(item&&typeof item==='object')for(const [key,entry]of Object.entries(item)){
      if(sensitive(key)&&typeof entry==='string'&&entry){secrets.add(entry);if(/cookie/i.test(key))scan('Cookie: '+entry);if(/authorization/i.test(key))scan('Authorization: '+entry);}
      gather(entry);
    }
  }
  gather(value);
  const forms=[...new Set([...secrets].flatMap(secret=>[secret,JSON.stringify(secret).slice(1,-1),encodeURIComponent(secret)]))].filter(Boolean).sort((a,b)=>b.length-a.length);
  // One pass: inserted markers are never scanned for shorter secrets.
  const pattern=forms.length?new RegExp(forms.map(secret=>secret.replace(/[.*+?^${}()|[\]\\]/g,'\\$&')).join('|'),'g'):null;
  const text=input=>{const headersHidden=input.replace(/\b(?:authorization|proxy-authorization|cookie|set-cookie)\s*[:=]\s*[^\r\n]*/gi,'认证信息: [REDACTED]');return (pattern?headersHidden.replace(pattern,'[REDACTED]'):headersHidden).replace(/\b(?:sk-[a-z0-9_-]{8,}|ghp_[a-z0-9]+|eyJ[a-z0-9_-]+\.[a-z0-9_-]+\.[a-z0-9_-]+)\b/gi,'[REDACTED]');};
  function visit(item) {
    if(typeof item==='string'){try{const nested=JSON.parse(item);if(nested&&typeof nested==='object')return JSON.stringify(visit(nested));}catch{}return text(item);}
    if(Array.isArray(item))return item.map(visit);
    if(item&&typeof item==='object')return Object.fromEntries(Object.entries(item).map(([key,entry])=>[text(key),sensitive(key)?'[REDACTED]':visit(entry)]));
    return item;
  }
  return visit(value);
}
export function bindSources(body, trace={}) {
  const fragments=[];
  function visit(value,path,system=false,role=null) {
    if(typeof value==='string') {
      const candidates=(trace.native??[]).filter(item=>item.value===value&&value);
      if(candidates.length===1){const {value:ignored,...source}=candidates[0];fragments.push({path,...source});return;}
      if(system) {
        const matches=(trace.systems??[]).flatMap(source=>{const at=value.indexOf(source.value);return source.value&&at>=0&&value.indexOf(source.value,at+1)<0?[{path,start:at,end:at+source.value.length,origin:source.origin,name:source.name,sourcePath:source.sourcePath}]:[];});
        if(matches.length){let cursor=0;for(const item of matches.sort((a,b)=>a.start-b.start)){if(item.start<cursor)continue;if(item.start>cursor)fragments.push({path,start:cursor,end:item.start,origin:'unknown',name:'未注册 / 未映射片段'});fragments.push(item);cursor=item.end;}if(cursor<value.length)fragments.push({path,start:cursor,end:value.length,origin:'unknown',name:'未注册 / 未映射片段'});return;}
      }
      const origin=role==='assistant'?'model':['tool','function'].includes(role)?'tool':'unknown';
      fragments.push({path,origin,name:origin==='model'?'模型回复（未定位唯一原生条目）':origin==='tool'?'工具返回（未定位唯一原生条目）':'未知 / 未追踪',sourcePath:'unmapped'});return;
    }
    if(Array.isArray(value))value.forEach((item,index)=>visit(item,[...path,index],system,role));
    else if(value&&typeof value==='object')for(const [key,item]of Object.entries(value))if(key==='content'||key==='text'||key==='system')visit(item,[...path,key],system,role);
  }
  if(body.system!==undefined)visit(body.system,['system'],true);
  for(const [index,message]of (body.messages??[]).entries())visit(message.content,['messages',index,'content'],['system','developer'].includes(message.role),message.role);
  return fragments;
}
export async function snapshotRequest(request, apiKey, debug={}) {
  const headerSecrets=[...request.headers].filter(([key])=>/auth|api[-_]?key|credential|cookie|secret|token|password/i.test(key)).flatMap(([,value])=>[value,...value.split(/[;= ]/u).filter(part=>part.length>3)]);
  const headers = redactDebug(Object.fromEntries([...request.headers].map(([key,value]) => [key, /^(content-type|accept|anthropic-version)$/i.test(key) ? value : '[REDACTED]'])),apiKey,headerSecrets);
  const size = Number(request.headers.get('content-length'));
  if (size > DEBUG_BODY_LIMIT) return { headers, complete:false, bodyBytes:size, omitted:'整个请求体：超过 256 KiB 捕获上限', body:null };
  // Read a bounded clone; the original Request is passed unchanged to fetch.
  const reader = request.clone().body?.getReader();
  const chunks=[]; let bytes=0;
  if (reader) for (;;) {
    const next=await reader.read(); if(next.done)break;
    bytes+=next.value.byteLength;
    if(bytes>DEBUG_BODY_LIMIT){ void reader.cancel().catch(()=>{}); return {headers,complete:false,bodyBytes:bytes,omitted:'整个请求体：至少超过 256 KiB 捕获上限',body:null}; }
    chunks.push(next.value);
  }
  const body=Buffer.concat(chunks).toString('utf8');
  let parsed; try { parsed=JSON.parse(body); } catch { return {headers,complete:false,bodyBytes:bytes,omitted:'非 JSON 请求体，未显示',body:null}; }
  let trace={};try{trace=debug.sources?.()??{};}catch{}
  // Do not copy/redact the whole session history into debug metadata: retain only
  // exact candidate strings present in this bounded, final wire payload.
  const texts=[];const collect=value=>{if(typeof value==='string')texts.push(value);else if(value&&typeof value==='object')Object.values(value).forEach(collect);};collect(parsed);
  const values=new Set(texts);
  const projectionRegistrations=trace.registrations??null;
  trace={native:(trace.native??[]).filter(item=>values.has(item.value)),systems:(trace.systems??[]).filter(item=>typeof item.value==='string'&&item.value&&texts.some(text=>text.includes(item.value)))};
  const safe=redactDebug({body:parsed,registrations:debug.registrations??null,projectionRegistrations,trace},apiKey,headerSecrets);
  safe.sources=bindSources(safe.body,safe.trace);
  delete safe.trace;
  if(Buffer.byteLength(JSON.stringify(safe.body))>2*DEBUG_BODY_LIMIT)return {headers,complete:false,bodyBytes:bytes,omitted:'整个请求体：脱敏后超过 512 KiB 展示上限',body:null};
  if(Buffer.byteLength(JSON.stringify(safe))>DEBUG_RECORD_LIMIT) {
    safe.registrations=null;safe.projectionRegistrations=null;
    safe.snapshotOmitted='注册快照超过单记录 3 MiB 预算；未截取前缀或反推补造';
  }
  if(Buffer.byteLength(JSON.stringify(safe))>DEBUG_RECORD_LIMIT)return {headers,complete:false,bodyBytes:bytes,omitted:'整个记录超过 3 MiB 捕获上限',body:null};
  return {headers,complete:true,bodyBytes:bytes,...safe};
}
