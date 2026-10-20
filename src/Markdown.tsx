import { memo, useMemo } from 'react';
import { FileCode2, Image } from 'lucide-react';
import { sanitizeMarkdownTokens } from './sanitize';
import { EmbeddedPreview } from './EmbeddedContent';
import { contentSegments, type EmbeddedContent } from './embedPolicy';
import { assetReferenceSegments, type MarkdownSegment } from './assetReferences';
import { marked } from 'marked';
type Segment = {kind:'markdown';html:string} | {kind:'asset';id:string;label:string;image:boolean} | {kind:'embed';value:EmbeddedContent};
function fragmentHtml(text: string, links: ReturnType<typeof marked.lexer>['links']) {
  const lexer = new marked.Lexer(); lexer.tokens.links = {...links};
  return sanitizeMarkdownTokens(lexer.lex(text));
}
// Mapping is over original UTF-16 boundaries. Marked normalizes CRLF/CR before lexing.
function blocks(text: string) {
  let normalized = ''; const offsets = [0];
  for (let i = 0; i < text.length;) { const c = text[i++]; if (c === '\r') { if (text[i] === '\n') i++; normalized += '\n'; } else normalized += c; offsets.push(i); }
  const tokens = marked.lexer(normalized); let cursor = 0, aligned = true;
  const links = tokens.links;
  return tokens.map(token => {
    const start = cursor; cursor += token.raw.length;
    const exact = aligned = aligned && normalized.slice(start,cursor) === token.raw;
    const special = contentSegments(token.raw).find(s=>s.kind === 'embed');
    const assets = assetReferenceSegments(token.raw);
    const segments: Segment[] = special?.kind === 'embed' ? [special] : assets.some(s=>s.kind === 'asset') ? assets.map(s=>s.kind === 'markdown' ? {kind:'markdown',html:fragmentHtml(s.text,links)} : s) : [{kind:'markdown',html:sanitizeMarkdownTokens([token])}];
    return {start: exact ? offsets[start] : undefined, end: exact ? offsets[cursor] : undefined, segments};
  });
}
// Asset references stay React controls; trusted range attributes are added after sanitization.
export const Markdown = memo(function Markdown({ text, className = '', courseId, onAsset, focus = false }: { text: string; className?: string; courseId?: string; focus?: boolean; onAsset?: (courseId: string, assetId: string) => void }) {
  const segments = useMemo<Segment[]>(() => { const links = marked.lexer(text).links; return contentSegments(text).flatMap<MarkdownSegment | {kind:'embed';value:EmbeddedContent}>(segment => segment.kind === 'embed' ? [segment] : assetReferenceSegments(segment.text)).map(segment => segment.kind === 'markdown' ? {kind:'markdown',html:fragmentHtml(segment.text,links)} : segment); }, [text]);
  const focused = useMemo(()=>focus ? blocks(text).filter(b=>b.segments.some(s=>s.kind !== 'markdown' || s.html.trim())) : [],[focus,text]);
  const render = (segment: Segment, index: number) => segment.kind === 'embed' ? <EmbeddedPreview key={index} value={segment.value}/> : segment.kind === 'markdown' ? <div className="markdown-fragment" key={index} dangerouslySetInnerHTML={{__html:segment.html}}/> : <button className="asset-reference" key={index} disabled={!courseId||!onAsset} title={courseId?'预览同课程本机资料':'请从来源文档打开这条资料引用'} onClick={()=>{if(courseId&&onAsset)onAsset(courseId,segment.id);}}>{segment.image?<Image size={14}/>:<FileCode2 size={14}/>}<span>{segment.label}</span><small>{segment.image?'查看本机图片':'查看资料'}</small></button>;
  return <div className={`markdown ${className}`}>{focus ? focused.map((block,index)=><div key={index} data-focus-start={block.start} data-focus-end={block.end} className={index === 0 ? `markdown ${className}` : undefined}>{block.segments.map(render)}</div>) : segments.map(render)}</div>;
});
