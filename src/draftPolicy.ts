export type ExerciseInput = { answer: string; reason: string };
export function needsDraftChoice(local: ExerciseInput, saved: ExerciseInput, edited: boolean) {
  return edited && (local.answer !== saved.answer || local.reason !== saved.reason);
}
export function unbackedSectionHasInput(loaded: boolean | undefined, body: string, savedBody: string, question: string) {
  return loaded === false && (body !== savedBody || question.length > 0);
}

// Compare only fields owned by the requested draft key, never the active view.
export function draftInputsDiffer(key: string, local: { body?: string; question?: string; importTitle?: string; importBody?: string }, saved: { body?: string; question?: string; importTitle?: string; importBody?: string }) {
  const fields = key === 'workspace' ? ['importTitle', 'importBody'] as const : ['body', 'question'] as const;
  return fields.some(field => (local[field] ?? '') !== (saved[field] ?? ''));
}

export function questionForSection(drafts: Record<string, string>, sectionId: string) {
  return drafts[sectionId] ?? '';
}
