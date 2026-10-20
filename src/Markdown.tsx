import { memo, useMemo } from 'react';
import { FileCode2, Image } from 'lucide-react';
import { sanitizeMarkdown } from './sanitize';
import { EmbeddedPreview } from './EmbeddedContent';
import { contentSegments } from './embedPolicy';
import { assetReferenceSegments, type MarkdownSegment } from './assetReferences';
import { marked } from 'marked';
// Asset references are React controls, never arbitrary links or automatic media requests.
export const Markdown = memo(function Markdown({ text, className = '', courseId, onAsset, focus = false }: { text: string; className?: string; courseId?: string; focus?: boolean; onAsset?: (courseId: string, assetId: string) => void }) {
  const segments = useMemo(() => contentSegments(text).flatMap<MarkdownSegment | {kind:'embed';value:import('./embedPolicy').EmbeddedContent}>(segment => segment.kind === 'embed' ? [segment] : assetReferenceSegments(segment.text)).map(segment => segment.kind === 'markdown' ? { ...segment, html: sanitizeMarkdown(segment.text) } : segment), [text]);
  if (focus) { let offset = 0; return <div className={`markdown ${className}`}>{marked.lexer(text).map((token, index) => { const start = offset; offset += token.raw.length; return <div key={index} data-focus-start={start} data-focus-end={offset}><Markdown text={token.raw} className={index === 0 ? className : ''} courseId={courseId} onAsset={onAsset}/></div>; })}</div>; }
  return <div className={`markdown ${className}`}>{segments.map((segment,index)=>segment.kind==='embed'?<EmbeddedPreview key={index} value={segment.value}/>:segment.kind==='markdown'?<div className="markdown-fragment" key={index} dangerouslySetInnerHTML={{__html:segment.html}}/>:<button className="asset-reference" key={index} disabled={!courseId||!onAsset} title={courseId?'预览同课程本机资料':'请从来源文档打开这条资料引用'} onClick={()=>{if(courseId&&onAsset)onAsset(courseId,segment.id);}}>{segment.image?<Image size={14}/>:<FileCode2 size={14}/>}<span>{segment.label}</span><small>{segment.image?'查看本机图片':'查看资料'}</small></button>)}</div>;
});
