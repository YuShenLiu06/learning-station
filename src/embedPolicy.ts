import { marked } from 'marked';
export type EmbeddedContent = { kind: 'html'; source: string; content: string } | { kind: 'web' | 'video'; source: string };
export const MAX_HTML_CHARS = 200_000;
export function safeRemoteUrl(source: string): string | undefined {
  try {
    const url = new URL(source);
    if (source.length > 2048 || url.protocol !== 'https:' || url.username || url.password || url.port && url.port !== '443') return;
    const host = url.hostname.toLowerCase();
    if (!host.includes('.') || /^(?:\d+\.){3}\d+$/.test(host) || host.includes(':') || /(?:^|\.)(?:localhost|local|internal|test|invalid)$/.test(host)) return;
    return url.href;
  } catch { return; }
}
export function parseEmbed(text: string): EmbeddedContent | undefined {
  try {
    const value = JSON.parse(text);
    if (!value || typeof value !== 'object' || Object.keys(value).some(key => !['kind','source','content'].includes(key)) || typeof value.source !== 'string') return;
    if (value.kind === 'html' && typeof value.content === 'string' && value.content.length <= MAX_HTML_CHARS && value.source.length <= 200) return value;
    if (['web','video'].includes(value.kind) && !('content' in value) && safeRemoteUrl(value.source)) return value;
  } catch { /* Invalid blocks remain visible as source text. */ }
}
export function embedMarkdown(value: EmbeddedContent): string {
  // JSON escapes newlines; a fence longer than any source fence prevents breakout.
  const json = JSON.stringify(value);
  const fence = '`'.repeat(Math.max(3, ...Array.from(json.matchAll(/`+/g), match => match[0].length + 1)));
  return `\n\n${fence}learning-embed\n${json}\n${fence}\n`;
}
export function contentSegments(text: string): ({ kind:'markdown'; text:string } | { kind:'embed'; value:EmbeddedContent })[] {
  const segments: ({ kind:'markdown'; text:string } | { kind:'embed'; value:EmbeddedContent })[] = [];
  for (const token of marked.lexer(text)) {
    const value = token.type === 'code' && token.lang === 'learning-embed' ? parseEmbed(token.text) : token.type === 'code' && token.lang?.toLowerCase() === 'html' && token.text.length <= MAX_HTML_CHARS ? {kind:'html' as const,source:'AI 演示.html',content:token.text} : undefined;
    if (value) segments.push({kind:'embed',value});
    else { const last = segments.at(-1); if (last?.kind === 'markdown') last.text += token.raw; else segments.push({kind:'markdown',text:token.raw}); }
  }
  return segments;
}
