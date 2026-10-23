import { useEffect, useMemo, useRef, useState } from 'react';
import { ArrowDown, ArrowUp, ArrowUpRight, FileText } from 'lucide-react';
import { marked } from 'marked';
import { call, nativeAvailable } from './api';
import { diffParts } from './editDiff';
import { Markdown } from './Markdown';
import { sanitizeMarkdown } from './sanitize';

const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
export type EditProposal = { id: string; courseId: string; path: string; original: string; content: string; status: string; expectedVersion?: string };
type Props = { courseId: string; documentId: string; isDirty: (id: string) => boolean; isLoaded: (id: string) => boolean; onAccepted: (id: string) => Promise<void>; onOpen: (id: string) => void | Promise<void>; notify: (message: string) => void };
export const proposalDocumentId = (path: string) => /^documents\/([^/]+)\.md$/u.exec(path)?.[1] || '';
export const proposalFileName = (path: string) => path.split('/').at(-1) || '文本文件';

// Some Markdown constructs lose essential source information in the reading
// renderer (targets/definitions/HTML), or would create an embedded preview.
// Changed versions of those constructs must remain inert, exact text.
function needsReviewSource(text: string) {
  let source = !sanitizeMarkdown(text).replace(/<[^>]*>/gu, '').trim();
  marked.walkTokens(marked.lexer(text), token => {
    if (['def', 'html', 'link', 'image'].includes(token.type)
      || token.type === 'code' && ['html', 'learning-embed'].includes((token.lang || '').toLowerCase())) source = true;
  });
  return source;
}

export function useEditReview(props: Props) {
  const [items, setItems] = useState<EditProposal[]>([]);
  const [selectedRaw, setSelectedRaw] = useState('');
  const [jump, setJump] = useState('');
  const [busy, setBusy] = useState(false);
  const diffRoot = useRef<HTMLDivElement>(null);
  const [hunk, setHunk] = useState(0);
  const [error, setError] = useState('');
  const latest = useRef(props); latest.current = props;
  // A late poll must not put a just-decided proposal back into the document.
  const settled = useRef(new Set<string>());
  useEffect(() => {
    setItems([]); setSelectedRaw(''); setError(''); settled.current.clear();
    if (!nativeAvailable || !props.courseId) return;
    let stopped = false;
    const cache = new Map<string, { stamp: string; proposal: EditProposal }>();
    let timer: ReturnType<typeof setTimeout>;
    async function poll() {
      try {
        const list = await call<{ id: string; stamp: string }[]>('edit_proposals', { courseId: props.courseId });
        for (const item of list) {
          if (cache.get(item.id)?.stamp !== item.stamp) {
            const proposal = await call<EditProposal>('edit_proposal', { courseId: props.courseId, id: item.id });
            cache.set(item.id, { stamp: item.stamp, proposal });
          }
        }
        if (!stopped) setItems(list.map(item => cache.get(item.id)!.proposal).filter(p => !settled.current.has(p.id) && ['pending', 'unknown', 'accepting'].includes(p.status)));
      } catch (e) { if (!stopped) setError(errorText(e)); }
      finally { if (!stopped) timer = setTimeout(() => { void poll(); }, 1000); }
    }
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [props.courseId]);
  const raw = items.find(p => p.id === selectedRaw);
  const readOnly = !!raw && !!proposalDocumentId(raw.path);
  const proposal = raw || items.find(p => proposalDocumentId(p.path) === props.documentId);
  const blocks = useMemo(() => {
    if (!proposal) return [];
    // Keep code fences, lists and tables intact; never split their Markdown
    // delimiters across independently rendered fragments.
    const tokenize = (text: string) => marked.lexer(text).map(token => token.raw);
    const grouped: ReturnType<typeof diffParts> = [];
    for (const block of diffParts(tokenize(proposal.original), tokenize(proposal.content))) {
      const previous = grouped.at(-1);
      if (previous?.kind === block.kind) previous.text += block.text;
      else grouped.push({ ...block });
    }
    return grouped.map((block, index) => {
      const neighbors = [grouped[index - 1], grouped[index + 1]];
      const indistinguishable = neighbors.some(other => other && other.kind !== 'same' && other.kind !== block.kind && other.text !== block.text && sanitizeMarkdown(other.text) === sanitizeMarkdown(block.text));
      return { ...block, source: block.kind !== 'same' && (needsReviewSource(block.text) || indistinguishable) };
    });
  }, [proposal]);
  const hunks = blocks.flatMap((block, i) => block.kind !== 'same' && (i === 0 || blocks[i - 1].kind === 'same') ? [i] : []);
  useEffect(() => { setHunk(0); setError(''); }, [proposal?.id]);
  function navigate(delta: number) {
    if (!hunks.length) return;
    const index = (hunk + delta + hunks.length) % hunks.length;
    setHunk(index);
    diffRoot.current?.querySelectorAll<HTMLElement>('[data-review-hunk]')[index]?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }
  useEffect(() => {
    if (!jump || proposal?.id !== jump) return;
    const frame = requestAnimationFrame(() => { diffRoot.current?.scrollIntoView({ block: 'start' }); setJump(''); });
    return () => cancelAnimationFrame(frame);
  }, [jump, proposal?.id]);
  async function locate(p: EditProposal) {
    const owner = latest.current.courseId;
    const id = proposalDocumentId(p.path);
    setSelectedRaw('');
    if (id) {
      try { await latest.current.onOpen(id); }
      catch {
        if (latest.current.courseId !== owner) return;
        // Failed navigation must never offer a second route around draft guards.
        setSelectedRaw(p.id);
      }
    } else setSelectedRaw(p.id);
    if (latest.current.courseId === owner) setJump(p.id);
  }
  const dirty = proposal ? props.isDirty(proposalDocumentId(proposal.path)) : false;
  async function decide(accept: boolean) {
    if (!proposal || busy || (accept && readOnly && proposal.status === 'pending')) return;
    const id = proposalDocumentId(proposal.path);
    if (accept && proposal.status === 'pending' && latest.current.isDirty(id)) { setError('还有未保存的编辑。请先处理草稿，建议和原文均已保留。'); return; }
    setBusy(true); setError('');
    try {
      if (accept && proposal.status === 'pending' && id && !latest.current.isLoaded(id)) {
        const [section, draft] = await Promise.all([
          call<{ body: string }>('section_open', { id }),
          call<{ data: { body?: string } } | null>('draft_get', { key: `section:${id}` }),
        ]);
        if (draft?.data.body !== undefined && draft.data.body !== section.body) throw new Error('该文件有未保存的本机草稿。请先打开并处理草稿；建议和正文均保留。');
      }
      if (accept && proposal.status === 'pending' && latest.current.isDirty(id)) throw new Error('当前文件有未保存编辑。请先处理草稿；建议和正文均保留。');
      const receipt = await call<{ persisted: boolean; status?: string }>('decide_edit_proposal', { courseId: proposal.courseId, id: proposal.id, accept });
      if (receipt.persisted) {
        if (id) await latest.current.onAccepted(id);
        latest.current.notify(id ? '修改已保存到正文。' : `修改已保存：${proposalFileName(proposal.path)}。可从课程资料目录用对应程序打开。`);
      } else if (receipt.status === 'rejected') latest.current.notify('已拒绝本次建议，正文未修改。');
      else throw new Error('结果尚未确认，请核对原建议，勿重复写入。');
      settled.current.add(proposal.id);
      setItems(old => old.filter(p => p.id !== proposal.id)); setSelectedRaw('');
    } catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  }
  return { items, proposal, raw: !!raw, readOnly, blocks, hunks, hunk, diffRoot, dirty, busy, error, navigate, locate, decide, closeRaw: () => setSelectedRaw(''), dismissError: () => setError('') };
}
export type EditReviewState = ReturnType<typeof useEditReview>;

export function EditReviewContent({ review, onAsset, hideTitle }: { review: EditReviewState; hideTitle?: boolean; onAsset?: (courseId: string, assetId: string) => void }) {
  if (!review.proposal) return null;
  return <div ref={review.diffRoot} className={`inline-edit-review${hideTitle ? ' hide-redundant-title' : ''}`} aria-label="正文修改差异">
    {review.dirty && <p className="review-draft-warning" role="status">还有未保存的编辑。以下对照 AI 读取的原文；你的草稿仍在编辑器中，请先处理后再接受。</p>}
    {review.blocks.map((block, index) => block.kind === 'same'
      ? <Markdown key={index} text={block.text} referenceText={review.proposal!.original} courseId={review.proposal!.courseId} onAsset={onAsset}/>
      : <div key={index} className={`review-change review-${block.kind}`} data-review-hunk={review.hunks.includes(index) ? '' : undefined}>
        <span className="review-change-sign" aria-label={block.kind === 'removed' ? '删除原文' : '新增建议'}>{block.kind === 'removed' ? '−' : '+'}</span>
        <div><span className="review-change-label">{block.kind === 'removed' ? '原文' : '建议'}</span>{block.source ? <pre className="review-source" aria-label="准确源码"><code>{block.text}</code></pre> : <Markdown text={block.text} referenceText={block.kind === 'added' ? review.proposal!.content : review.proposal!.original} courseId={review.proposal!.courseId} onAsset={onAsset}/>}</div>
      </div>)}
  </div>;
}
export function EditReviewDock({ review, editing, onRead }: { review: EditReviewState; editing?: boolean; onRead?: () => void }) {
  if (!review.proposal) return null;
  const unknown = review.proposal.status !== 'pending';
  return <div className="review-dock-wrap">
    {(review.error || unknown) && <p className="review-recovery" role="alert">{review.error || '写入结果需要核对，旧稿和新稿均保留。'}{review.error && <button onClick={review.dismissError}>关闭提示</button>}</p>}
    <section className="review-dock" aria-label="本文件修改审阅">
      <div className="review-dock-copy"><strong>{unknown ? '核对这页修改' : '审阅这页修改'}</strong><small>{review.busy ? '正在核对保存结果…' : review.readOnly ? '文件未打开，可查看保留内容或拒绝建议' : review.dirty ? '先处理未保存的编辑' : '接受后保存正文'}</small></div>
      <div className="review-navigation"><button aria-label="上一处修改" disabled={!review.hunks.length || editing} onClick={() => review.navigate(-1)}><ArrowUp size={15}/></button><span>{review.hunks.length ? `${review.hunk + 1} / ${review.hunks.length}` : '无差异'}</span><button aria-label="下一处修改" disabled={!review.hunks.length || editing} onClick={() => review.navigate(1)}><ArrowDown size={15}/></button></div>
      <div className="review-dock-actions">{editing && <button className="review-secondary" onClick={onRead}>查看差异</button>}{unknown ? <button className="review-primary" disabled={review.busy} onClick={() => { void review.decide(true); }}>核对写入结果</button> : <><button className="review-secondary" disabled={review.busy} onClick={() => { void review.decide(false); }}>拒绝</button>{!review.readOnly && <button className="review-primary" disabled={review.busy || review.dirty} onClick={() => { void review.decide(true); }}>接受{review.hunks.length ? `这 ${review.hunks.length} 处` : '修改'}</button>}</>}</div>
    </section>
  </div>;
}
export function EditReviewLinks({ review, title }: { review: EditReviewState; title: (p: EditProposal) => string }) {
  if (!review.items.length && !review.error) return null;
  return <div className="review-file-links" aria-label="待审阅文件">
    {review.items.map(p => <button className="review-file-link" key={p.id} onClick={() => review.locate(p)}><FileText size={17}/><span><strong>{title(p)}</strong><small>{p.status === 'pending' ? '修改待确认 · 查看正文差异' : '写入待核对 · 查看保留的内容'}</small></span><ArrowUpRight size={15}/></button>)}
    {!review.items.length && review.error && <p className="inline-error" role="alert">修改建议暂未读到：{review.error}<button onClick={review.dismissError}>关闭提示</button></p>}
  </div>;
}
