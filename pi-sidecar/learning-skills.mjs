export class LearningSkillError extends Error {
  constructor(code) { super(code); this.code = code; }
}
// Guidance is loaded through list/load tools, never expanded with a request.
export function requiredLearningSkills(_policy) { return []; }
export function resolveLearningSkills(input, _policy) {
  const snapshots = input ?? [];
  if (!Array.isArray(snapshots) || snapshots.length) throw new LearningSkillError('SKILL_MISSING');
  return { versions: [], system: '' };
}
