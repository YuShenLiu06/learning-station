import { Markdown } from './Markdown';
import { needsReviewSource } from './EditReview';
import type { EditReviewState, QuestionChange, ReviewQuestion } from './EditReview';

function ReviewText({ text }: { text: string }) {
  return needsReviewSource(text) ? <pre className="review-source"><code>{text}</code></pre> : <Markdown text={text}/>;
}
function Question({ question, label }: { question: ReviewQuestion; label?: string }) {
  return <div className="exercise-review-version">
    {label && <strong className="review-change-label">{label}</strong>}
    <p className="muted small">{question.format === 'exact' ? '标准答案核对' : '开放作答'}</p>
    <ReviewText text={question.question}/>
    <div className="exercise-review-reference"><strong>参考答案</strong><ReviewText text={question.answers.join(' / ')}/><strong>解析</strong><ReviewText text={question.explanation || '尚未提供解析。'}/></div>
    {!!question.tags?.length && <p className="muted small">标签：{question.tags.join('、')}</p>}
    {!!question.sourceIds?.length && <details><summary>材料来源（{question.sourceIds.length}）</summary><Markdown text={question.sourceIds.map(id => `- [[${id}]]`).join('\n')}/></details>}
    {question.rubric && <details><summary>评分标准</summary>{question.rubric.criteria.map(c => <p key={c.id}>{c.label} · {c.maxPoints} 分：{c.description}</p>)}</details>}
  </div>;
}
export function ExerciseReview({ review }: { review: EditReviewState }) {
  const proposal = review.proposal!;
  const changes = proposal.exercise!.changes;
  // Validated by Rust at proposal creation; JSON is never rendered as document prose.
  const original = (JSON.parse(proposal.original) as { exercises: ReviewQuestion[] }).exercises;
  const rows: { question?: ReviewQuestion; change?: QuestionChange }[] = original.map(question => ({question, change: changes.find(c => c.before?.id === question.id)}));
  rows.push(...changes.filter(c => !c.before).map(change => ({change})));
  return <section ref={review.diffRoot} className="exercise-review" aria-label="逐题修改差异">
    {review.dirty && <p className="review-draft-warning">请先处理未保存的 Markdown 草稿，原文和建议均保留。</p>}
    {rows.map(({question,change}, index) => {
      const pending = change && ['pending','accepting'].includes(change.status);
      const shown = !change || change.status === 'rejected' ? question : change.after;
      if (!pending && !shown) return null;
      return <article key={change?.id || question!.id} className="exercise-review-question">
        <header><strong>第 {index + 1} 题</strong>{pending && <span>{!change.before ? '新增题目' : !change.after ? '删除题目' : '修改题目'}</span>}{change?.status === 'accepted' && <span>已保存</span>}</header>
        {pending ? <>
          {change.before && <div className="review-change review-removed"><span className="review-change-sign">−</span><Question question={change.before} label="原题"/></div>}
          {change.after && <div className="review-change review-added"><span className="review-change-sign">+</span><Question question={change.after} label="建议题目"/></div>}
          <div className="exercise-review-actions">{change.status === 'accepting' ? <button className="review-primary" disabled={review.busy} onClick={() => void review.decideQuestion(change.id, true)}>核对此题保存结果</button> : <><button className="review-secondary" disabled={review.busy} onClick={() => void review.decideQuestion(change.id, false)}>拒绝此题修改</button><button className="review-primary" disabled={review.busy || review.dirty || review.readOnly} onClick={() => void review.decideQuestion(change.id, true)}>{!change.before ? '接受新增题目' : !change.after ? '接受删除题目' : '接受此题修改'}</button></>}</div>
        </> : shown && <Question question={shown}/>}
      </article>;
    })}
  </section>;
}
