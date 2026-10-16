export type MarkdownSegment = { kind: 'markdown'; text: string } | { kind: 'asset'; id: string; label: string; image: boolean };
export function assetReferenceSegments(text: string): MarkdownSegment[] {
  const result: MarkdownSegment[] = []; const pattern = /(```[\s\S]*?(?:```|$)|~~~[\s\S]*?(?:~~~|$)|`[^`\n]*`)|(!?)\[([^\]\n]{0,200})\]\(asset:([A-Za-z0-9_-]{1,128})\)/gu; let start = 0;
  for (const match of text.matchAll(pattern)) { if (match[1]) continue; const index = match.index!; if (index > 0 && text[index - 1] === '\\') continue; if (index > start) result.push({ kind: 'markdown', text: text.slice(start, index) }); result.push({ kind: 'asset', id: match[4], label: match[3] || '课程资料', image: match[2] === '!' }); start = index + match[0].length; }
  if (start < text.length || !result.length) result.push({ kind: 'markdown', text: text.slice(start) }); return result;
}
export function safeAssetImage(value?: string) { return typeof value === 'string' && value.length <= 7_000_000 && /^data:image\/(?:png|jpeg|gif|webp);base64,[A-Za-z0-9+/]+=*$/u.test(value) ? value : undefined; }
export function citationForAsset(asset: { id: string; fileName: string; mime: string }) { const label = asset.fileName.replace(/[\[\]<>\r\n]/gu, '_'); return `${asset.mime.startsWith('image/') ? '!' : ''}[${label}](asset:${asset.id})`; }
