import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { runLearningAgent } from '../harness.mjs';
import { requiredLearningSkills } from '../learning-skills.mjs';
export function captureTestSkills(policy) {
  return requiredLearningSkills(policy).map(({id,scope}) => {
    const content=readFileSync(new URL(`../../skills/${id}/SKILL.md`,import.meta.url),'utf8');
    return {id,scope,content,sha256:createHash('sha256').update(content).digest('hex')};
  });
}
export function runLearningAgentWithSkills(request,...args) {
  const policy=request.toolPolicy ?? {mode:'legacy',allowedTools:[]};
  return runLearningAgent({...request,learningSkills:request.learningSkills??captureTestSkills(policy)},...args);
}
