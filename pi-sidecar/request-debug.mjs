// Only sanitized data leaves the sending process. Never read credential files.
export const DEBUG_BODY_LIMIT = 256 * 1024;
const sensitive = key => /^(?:.*[-_])?(?:authorization|cookie|set-cookie|api[-_]?key|access[-_]?token|refresh[-_]?token|token|password|passwd|secret|credential)$/i.test(key) || /^(apiKey|accessToken|refreshToken|clientSecret)$/i.test(key);
export function redactDebug(value, apiKey = '', extraSecrets = []) {
  const secrets = new Set([...(apiKey ? [apiKey] : []),...extraSecrets].filter(Boolean));
  function gather(item) {
    if (!item || typeof item !== 'object') return;
    for (const [key, entry] of Object.entries(item)) {
      if (sensitive(key) && typeof entry === 'string' && entry) secrets.add(entry);
      if (typeof entry === 'string') {
        for(const match of entry.matchAll(/\b(?:Bearer|Basic)\s+([a-z0-9+/_=.:-]+)/gi))secrets.add(match[1]);
        for(const match of entry.matchAll(/(?:api[-_ ]?key|access[-_]?token|refresh[-_]?token|password|passwd|secret|token|cookie)\s*[:=]\s*([^\s,;"'<>}]+)/gi))secrets.add(match[1]);
        try { gather(JSON.parse(entry)); } catch {} }
      else gather(entry);
    }
  }
  gather(value);
  function text(input) {
    let out = input;
    for (const secret of [...secrets].sort((a,b) => b.length-a.length)) {
      for (const form of new Set([secret, JSON.stringify(secret).slice(1,-1), encodeURIComponent(secret)])) out = out.split(form).join('[REDACTED]');
    }
    out = out.replace(/\b(Bearer|Basic)\s+[a-z0-9+/_=.:-]+/gi, '$1 [REDACTED]');
    out = out.replace(/((?:api[-_]?key|access[-_]?token|refresh[-_]?token|password|passwd|secret|token|cookie)\s*[:=]\s*)([^\s,;"'<>}]+)/gi, '$1[REDACTED]');
    out = out.replace(/\b(?:sk-[a-z0-9_-]{8,}|ghp_[a-z0-9]+|eyJ[a-z0-9_-]+\.[a-z0-9_-]+\.[a-z0-9_-]+)\b/gi, '[REDACTED]');
    return out;
  }
  function visit(item) {
    if (typeof item === 'string') {
      try { const parsed=JSON.parse(item); if (parsed && typeof parsed==='object') return JSON.stringify(visit(parsed)); } catch {}
      return text(item);
    }
    if (Array.isArray(item)) return item.map(visit);
    if (item && typeof item === 'object') return Object.fromEntries(Object.entries(item).map(([key,entry]) => [text(key), sensitive(key) ? '[REDACTED]' : visit(entry)]));
    return item;
  }
  return visit(value);
}
export async function snapshotRequest(request, apiKey) {
  const headerSecrets=[...request.headers].filter(([key])=>!/^(content-type|accept|anthropic-version)$/i.test(key)).flatMap(([,value])=>[value,...value.split(/[;= ]/u).filter(part=>part.length>3)]);
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
  return {headers,complete:true,bodyBytes:bytes,body:safe};
}
