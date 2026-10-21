// Only sanitized data leaves the sending process. Never read credential files.
export const DEBUG_BODY_LIMIT = 256 * 1024;
const sensitive = key => /(?:authorization|cookie|api[-_]?key|token|password|passwd|secret|credential)$/i.test(key.replace(/([a-z])([A-Z])/g,'$1_$2'));
const assignment=/(?:^|[^a-z0-9_])["']?(?:api[-_ ]?key|[a-z_-]{0,48}token|[a-z_-]{0,48}password|passwd|[a-z_-]{0,48}secret|cookie)["']?\s*[:=]\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^\s,;"'<>}]+))/gi;
export function redactDebug(value, apiKey = '', extraSecrets = []) {
  const secrets=new Set([...(apiKey?[apiKey]:[]),...extraSecrets].filter(Boolean));
  function scan(input) {
    const normalized=input.replace(/\\"/g,'"');
    for(const match of normalized.matchAll(assignment)) {const raw=match[1]??match[2]??match[3];if(raw) {secrets.add(raw);try{secrets.add(JSON.parse('"'+raw+'"'));}catch{}}}
    for(const match of normalized.matchAll(/(?:authorization["']?\s*[:=]\s*["']?)([^\r\n"']+)/gi)) {
      const raw=match[1].trim();secrets.add(raw);for(const part of raw.split(/\s+/).slice(1))if(part)secrets.add(part);
    }
    for(const match of normalized.matchAll(/cookie["']?\s*[:=]\s*["']?([^\r\n"']+)/gi)) {
      for(const pair of match[1].split(';')){const at=pair.indexOf('=');if(at>=0){const token=pair.slice(at+1).trim();if(token)secrets.add(token);}}
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
  const text=input=>(pattern?input.replace(pattern,'[REDACTED]'):input).replace(/\b(?:sk-[a-z0-9_-]{8,}|ghp_[a-z0-9]+|eyJ[a-z0-9_-]+\.[a-z0-9_-]+\.[a-z0-9_-]+)\b/gi,'[REDACTED]');
  function visit(item) {
    if(typeof item==='string'){try{const nested=JSON.parse(item);if(nested&&typeof nested==='object')return JSON.stringify(visit(nested));}catch{}return text(item);}
    if(Array.isArray(item))return item.map(visit);
    if(item&&typeof item==='object')return Object.fromEntries(Object.entries(item).map(([key,entry])=>[text(key),sensitive(key)?'[REDACTED]':visit(entry)]));
    return item;
  }
  return visit(value);
}
export async function snapshotRequest(request, apiKey) {
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
  const safe=redactDebug(parsed,apiKey,headerSecrets);
  if(Buffer.byteLength(JSON.stringify(safe))>2*DEBUG_BODY_LIMIT)return {headers,complete:false,bodyBytes:bytes,omitted:'整个请求体：脱敏后超过 512 KiB 展示上限',body:null};
  return {headers,complete:true,bodyBytes:bytes,body:safe};
}
