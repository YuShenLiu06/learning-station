import { createHash } from 'node:crypto';

export class LearningSkillError extends Error {
  constructor(code) { super(code); this.code = code; }
}
export function requiredLearningSkills(policy) {
  const id = { question: 'learning-explore', practice: 'learning-exercises', review: 'learning-review' }[policy.mode];
  const items = id ? [{ id, scope: 'mode' }] : [];
  if (policy.allowedTools.includes('generate_exercises') && !items.some(item => item.id === 'learning-exercises')) {
    items.push({ id: 'learning-exercises', scope: 'tool:generate_exercises' });
  }
  return items;
}
export function resolveLearningSkills(input, policy) {
  const expected = requiredLearningSkills(policy);
  const snapshots = input ?? [];
  if (!Array.isArray(snapshots) || snapshots.length !== expected.length) throw new LearningSkillError('SKILL_MISSING');
  for (let i = 0; i < expected.length; i++) {
    const value = snapshots[i], spec = expected[i];
    if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).sort().join(',') !== 'content,id,scope,sha256'
      || value.id !== spec.id || value.scope !== spec.scope || typeof value.content !== 'string' || !value.content.trim() || Buffer.byteLength(value.content) > 16 * 1024
      || value.content.includes('\0') || typeof value.sha256 !== 'string' || !/^[0-9a-f]{64}$/u.test(value.sha256)
      || createHash('sha256').update(value.content).digest('hex') !== value.sha256) throw new LearningSkillError('SKILL_INVALID');
    const header = /^---\nname: ([a-z0-9-]+)\ndescription: ([^\n]+)\n---\n([^]*\S[^]*)$/u.exec(value.content);
    if (!header || header[1] !== spec.id || !header[2].trim()) throw new LearningSkillError('SKILL_INVALID');
  }
  const versions = snapshots.map(({ id, sha256, scope }) => ({ id, sha256, scope }));
  const block = value => `本次程序主动展开的学习指导（${value.id}，SHA-256 ${value.sha256}；仅适用于${value.scope}）：\n${value.content}`;
  return {
    versions,
    system: snapshots.filter(value => value.scope === 'mode').map(block).join('\n\n'),
    exerciseGuidance: snapshots.filter(value => value.scope === 'tool:generate_exercises').map(block).join('\n\n'),
  };
}
