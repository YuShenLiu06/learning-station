import { memo, useEffect, useMemo, useState } from 'react';
import { FileCode2 } from 'lucide-react';
import { sanitizeMarkdownTokens } from './sanitize';
import { EmbeddedPreview } from './EmbeddedContent';
import { contentSegments, type EmbeddedContent } from './embedPolicy';
import { assetReferenceSegments, safeAssetImage, type MarkdownSegment } from './assetReferences';
import { call } from './api';
import type { CourseAssetPreview } from './types';
import { marked } from 'marked';
type Segment = {kind:'markdown';html:string} | {kind:'asset';id:string;label:string;image:boolean} | {kind:'embed';value:EmbeddedContent};
function AssetImage({ courseId, id, label, onAsset }: { courseId?: string; id: string; label: string; onAsset?: (courseId: string, assetId: string) => void }) {
  const [result, setResult] = useState<{ key: string; src?: string; error?: string }>();
  const [retry, setRetry] = useState(0);
  const key = `${courseId}:${id}`;
  useEffect(() => {
    let active = true;
    if (courseId) void call<CourseAssetPreview>('asset_read', { courseId, assetId: id }).then(value => {
      const src = safeAssetImage(value.dataUrl);
      if (active) setResult({ key, src, error: src ? undefined : '图片格式无法预览' });
    }).catch(error => { if (active) setResult({ key, error: error instanceof Error ? error.message : String(error) }); });
    return () => { active = false; };
  }, [courseId, id, key, retry]);
  const value = result?.key === key ? result : undefined;
  return <figure className="asset-inline-image">{value?.src ? <img src={value.src} alt={label} /> : <span role="status">{!courseId ? '请从来源课程打开图片' : value?.error || '正在读取图片…'}</span>}<figcaption>{label}{courseId && onAsset && <button type="button" className="text-reference" onClick={() => onAsset(courseId, id)}>查看图片</button>}{value?.error && <button type="button" className="text-reference" onClick={() => setRetry(value => value + 1)}>重试</button>}</figcaption></figure>;
}
export function ImageAttachments({ text, courseId }: { text: string; courseId?: string }) {
  const images = assetReferenceSegments(text).filter(segment => segment.kind === 'asset' && segment.image);
  if (!images.length) return null;
  return <div className="image-attachments" aria-label="已附图片">{images.map((segment, index) => segment.kind === 'asset' && <AssetImage key={`${segment.id}:${index}`} courseId={courseId} id={segment.id} label={segment.label} />)}</div>;
}
function fragmentHtml(text: string, links: ReturnType<typeof marked.lexer>['links']) {
  const lexer = new marked.Lexer(); lexer.tokens.links = {...links};
  return sanitizeMarkdownTokens(lexer.lex(text));
}
// Asset references stay React controls; trusted markup is added after sanitization.
export const Markdown = memo(function Markdown({ text, className = '', courseId, onAsset, referenceText }: { text: string; referenceText?: string; className?: string; courseId?: string; onAsset?: (courseId: string, assetId: string) => void }) {
  const segments = useMemo<Segment[]>(() => { const links = marked.lexer(referenceText ?? text).links; return contentSegments(text).flatMap<MarkdownSegment | {kind:'embed';value:EmbeddedContent}>(segment => segment.kind === 'embed' ? [segment] : assetReferenceSegments(segment.text)).map(segment => segment.kind === 'markdown' ? {kind:'markdown',html:fragmentHtml(segment.text,links)} : segment); }, [text, referenceText]);
  const render = (segment: Segment, index: number) => segment.kind === 'embed' ? <EmbeddedPreview key={index} value={segment.value}/> : segment.kind === 'markdown' ? <div className="markdown-fragment" key={index} dangerouslySetInnerHTML={{__html:segment.html}}/> : segment.image ? <AssetImage key={index} courseId={courseId} id={segment.id} label={segment.label} onAsset={onAsset}/> : <button className="asset-reference" key={index} disabled={!courseId||!onAsset} title={courseId?'预览同课程本机资料':'请从来源文档打开这条资料引用'} onClick={()=>{if(courseId&&onAsset)onAsset(courseId,segment.id);}}><FileCode2 size={14}/><span>{segment.label}</span><small>查看资料</small></button>;
  return <div className={`markdown ${className}`}>{segments.map(render)}</div>;
});
