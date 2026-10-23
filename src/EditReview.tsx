import { useEffect, useMemo, useRef, useState } from 'react';
import { call, nativeAvailable } from './api';
import { editDiff } from './editDiff';

const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

type Proposal = { id: string; courseId: string; path: string; original: string; content: string; status: string; expectedVersion?: string };
type Props = { courseId: string; documentId: string; isDirty: (id: string) => boolean; isLoaded: (id: string) => boolean; onAccepted: (id: string) => Promise<void> };
const documentId = (path: string) => /^documents\/([^/]+)\.md$/u.exec(path)?.[1] || '';

export function EditReview(props: Props) {
  const [items, setItems] = useState<Proposal[]>([]);
  const [selected, setSelected] = useState('');
  const [busy, setBusy] = useState(false);
  const diffRoot = useRef<HTMLPreElement>(null);
  const [hunk, setHunk] = useState(0);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const latest = useRef(props); latest.current = props;
  useEffect(() => {
    if (!nativeAvailable || !props.courseId) return;
    let stopped = false;
    const cache = new Map<string, { stamp: string; proposal: Proposal }>();
    let timer: ReturnType<typeof setTimeout>;
    setItems([]); setSelected(''); setMessage(''); setError('');
    async function poll() {
      try {
        const list = await call<{ id: string; stamp: string }[]>('edit_proposals', { courseId: props.courseId });
        for (const item of list) {
          if (cache.get(item.id)?.stamp !== item.stamp) {
            const proposal = await call<Proposal>('edit_proposal', { courseId: props.courseId, id: item.id });
            cache.set(item.id, { stamp: item.stamp, proposal });
          }
        }
        if (!stopped) setItems(list.map(item => cache.get(item.id)!.proposal).filter(p => ['pending', 'unknown', 'accepting'].includes(p.status)));
      } catch (e) { if (!stopped) setError(errorText(e)); }
      finally { if (!stopped) timer = setTimeout(() => { void poll(); }, 1000); }
    }
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [props.courseId]);
  const current = items.find(p => documentId(p.path) === props.documentId);
  const proposal = items.find(p => p.id === selected) || current;
  const lines = useMemo(() => proposal ? editDiff(proposal.original, proposal.content) : [], [proposal]);
  const hunks = lines.flatMap((line, i) => line.kind !== 'same' && (i === 0 || lines[i - 1].kind === 'same') ? [i] : []);
  function navigate(delta: number) {
    const index = (hunk + delta + hunks.length) % hunks.length;
    setHunk(index);
    const line = diffRoot.current?.children[hunks[index]] as HTMLElement | undefined;
    line?.scrollIntoView({ block: 'nearest' });
  }
  const dirty = proposal ? props.isDirty(documentId(proposal.path)) : false;
  async function decide(accept: boolean) {
    if (!proposal || busy) return;
    const id = documentId(proposal.path);
    if (accept && latest.current.isDirty(id)) { setError('当前文件有未保存编辑。草稿和建议均已保留，请先处理草稿。'); return; }
    setBusy(true); setError('');
    try {
      if (accept && id && !latest.current.isLoaded(id)) {
        // An unopened document may still have a durable draft from a previous
        // launch. A failed read is never evidence of a clean editor.
        const [section, draft] = await Promise.all([
          call<{ body: string }>('section_open', { id }),
          call<{ data: { body?: string } } | null>('draft_get', { key: `section:${id}` }),
        ]);
        if (draft?.data.body !== undefined && draft.data.body !== section.body) throw new Error('该文件有未保存的本机草稿。请先打开并处理草稿；建议和正文均保留。');
      }
      if (accept && latest.current.isDirty(id)) throw new Error('当前文件有未保存编辑。请先处理草稿；建议和正文均保留。');
      const receipt = await call<{ persisted: boolean; status?: string }>('decide_edit_proposal', { courseId: proposal.courseId, id: proposal.id, accept });
      if (receipt.persisted) {
        if (id) await latest.current.onAccepted(id);
        setMessage(id ? '修改已保存到正文。' : `修改已保存：courses/${proposal.courseId}/materials/${proposal.path}。可从课程资料目录用对应程序打开。`);
      } else if (receipt.status === 'rejected') setMessage('已拒绝本次建议，正文未修改。');
      else throw new Error('结果尚未确认，请核对原建议，勿重复写入。');
      setItems(old => old.filter(p => p.id !== proposal.id)); setSelected('');
    } catch (e) { setError(errorText(e)); }
    finally { setBusy(false); }
  }
  if (!items.length && !message && !error) return null;
  return <section className="edit-review" aria-label="AI 修改审阅">
    <div className="edit-review-bar"><strong>AI 修改待确认 · {items.length} 个文件</strong>{items.length > 0 && <select aria-label="选择待审阅文件" value={proposal?.id || ''} onChange={e => setSelected(e.target.value)}><option value="">选择待审阅文件</option>{items.map(p => <option key={p.id} value={p.id}>{p.path}</option>)}</select>}</div>
    {proposal && <><p>建议已保留，正文未修改。可停止生成后继续审阅；超时或关闭也不会丢失建议。{dirty && ' 当前文件有未保存编辑；草稿保留，暂不能接受。'}</p>
      <div className="edit-review-actions"><button disabled={busy || dirty || proposal.status !== 'pending'} onClick={() => { void decide(true); }}>接受修改</button><button disabled={busy || proposal.status !== 'pending'} onClick={() => { void decide(false); }}>拒绝修改</button></div>
      {proposal.status !== 'pending' && <p role="alert">写入结果需要核对。旧稿与新稿仍保留，请勿重复执行。<button disabled={busy} onClick={() => { void decide(true); }}>核对写入结果</button></p>}
      <div className="edit-review-actions"><span>{hunks.length} 处变化</span><button disabled={!hunks.length} onClick={() => navigate(-1)}>上一处</button><button disabled={!hunks.length} onClick={() => navigate(1)}>下一处</button></div><details open key={proposal.id}><summary>查看 Diff · − 原文 / + 建议</summary><pre ref={diffRoot} className="edit-diff">{lines.map((line, i) => <span key={i} className={`diff-${line.kind}`}>{line.kind === 'added' ? '+ ' : line.kind === 'removed' ? '− ' : '  '}{line.text}{'\n'}</span>)}</pre></details></>}
    {message && <p role="status">{message}<button onClick={() => setMessage('')}>关闭提示</button></p>}
    {error && <p role="alert">{error}<button onClick={() => setError('')}>关闭提示</button></p>}
  </section>;
}
