import { useState } from 'react';
import { exercisePartLabels, validateExerciseSelection } from './exerciseReferences';
import type { ExerciseReferenceSource } from './types';
export function ExerciseQuote({ source, onAdd }: { source: ExerciseReferenceSource; onAdd: (selection: string) => void }) {
 const [selected, setSelected] = useState(''), [error, setError] = useState('显示选段与规范原文不完全相同，请在下方原文中重选。');
 return <div className="assistant-quote-source"><p className="muted small">{source.title} · {exercisePartLabels[source.part]}。只引用此分区，不附带整份练习或隐藏答案。</p><textarea readOnly aria-label="练习分区规范原文" value={source.text} rows={10} onSelect={event => { const area = event.currentTarget; setSelected(area.value.slice(area.selectionStart, area.selectionEnd)); setError(''); }}/>{selected && <blockquote className="assistant-quote-selection">{selected}</blockquote>}{error && <p className="inline-error" role="alert">{error}</p>}<button className="primary-button" disabled={!selected} onClick={() => { try { validateExerciseSelection(source.text, selected); onAdd(selected); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } }}>加入本次引用</button></div>;
}
