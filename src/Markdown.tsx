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
// Asset references stay React controls; trusted markup is added after sanitization.
export const Markdown = memo(function Markdown({ text, className = '', courseId, onAsset, referenceText }: { text: string; referenceText?: string; className?: string; courseId?: string; onAsset?: (courseId: string, assetId: string) => void }) {
  const segments = useMemo<Segment[]>(() => { const links = marked.lexer(referenceText ?? text).links; return contentSegments(text).flatMap<MarkdownSegment | {kind:'embed';value:EmbeddedContent}>(segment => segment.kind === 'embed' ? [segment] : assetReferenceSegments(segment.text)).map(segment => segment.kind === 'markdown' ? {kind:'markdown',html:fragmentHtml(segment.text,links)} : segment); }, [text, referenceText]);
  const render = (segment: Segment, index: number) => segment.kind === 'embed' ? <EmbeddedPreview key={index} value={segment.value}/> : segment.kind === 'markdown' ? <div className="markdown-fragment" key={index} dangerouslySetInnerHTML={{__html:segment.html}}/> : <button className="asset-reference" key={index} disabled={!courseId||!onAsset} title={courseId?'预览同课程本机资料':'请从来源文档打开这条资料引用'} onClick={()=>{if(courseId&&onAsset)onAsset(courseId,segment.id);}}>{segment.image?<Image size={14}/>:<FileCode2 size={14}/>}<span>{segment.label}</span><small>{segment.image?'查看本机图片':'查看资料'}</small></button>;
  return <div className={`markdown ${className}`}>{segments.map(render)}</div>;
});
