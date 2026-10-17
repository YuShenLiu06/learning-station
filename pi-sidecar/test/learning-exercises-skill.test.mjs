import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { Value } from 'typebox/value';
import { createLearningPlugin, validateCall } from '../learning-plugin.mjs';

// The packaged guidance is a live contract: its fenced JSON samples must satisfy the real
// generate_exercises tool schema, not merely mention field names.
const content = readFileSync(new URL('../../skills/learning-exercises/SKILL.md', import.meta.url), 'utf8');
const samples = [...content.matchAll(/```json\n([^]*?)```/gu)].map((match) => JSON.parse(match[1]));
const toolInputs = samples.filter((sample) => Object.hasOwn(sample, 'requirements'));
const practiceOutputs = samples.filter((sample) => !Object.hasOwn(sample, 'requirements'));
const parameters = createLearningPlugin({ requestTool: async () => {}, allowedTools: ['generate_exercises'] }).tools[0].parameters;

test('SKILL.md tool-input samples pass the real generate_exercises TypeBox schema', () => {
  assert.equal(toolInputs.length, 2);
  assert.deepEqual(toolInputs.map((sample) => sample.exercises[0].format).sort(), ['exact', 'self']);
  for (const input of toolInputs) {
    assert.deepEqual(Object.keys(input).sort(), ['exercises', 'requirements', 'summary']);
    // Real schema check against the TypeBox parameters the SDK registers, plus the sidecar's own gate.
    assert.equal(Value.Check(parameters, input), true);
    assert.doesNotThrow(() => validateCall('skill-sample-contract', 'generate_exercises', structuredClone(input)));
    // The check is not vacuous: an undeclared field or a missing requirement must fail it.
    assert.equal(Value.Check(parameters, { ...input, extra: 'denied' }), false);
    assert.equal(Value.Check(parameters, { summary: input.summary, exercises: input.exercises }), false);
  }
});

test('SKILL.md practice output samples stay a distinct product shape for Rust validation', () => {
  // The JS side has no final-product validator: practice output is validated by Rust through the
  // validate_output tool request. Here we pin the model-facing shape Rust receives (exercises.rs
  // validate) and prove the pair cannot be merged into one JSON.
  assert.equal(practiceOutputs.length, 2);
  assert.deepEqual(practiceOutputs.map((sample) => sample.exercises[0].format).sort(), ['exact', 'self']);
  for (const output of practiceOutputs) {
    assert.deepEqual(Object.keys(output).sort(), ['exercises', 'summary']);
    assert.equal(Value.Check(parameters, output), false, 'practice output must not double as tool input');
    for (const exercise of output.exercises) {
      assert.deepEqual(Object.keys(exercise).sort().filter((key) => !['rubric'].includes(key)), ['answers', 'explanation', 'format', 'question', 'sourceIds', 'tags']);
      assert.ok(exercise.sourceIds.length > 0, 'samples declare demo sourceIds');
    }
  }
  const selfOutput = practiceOutputs.find((sample) => sample.exercises[0].format === 'self');
  assert.ok(selfOutput.exercises[0].rubric.criteria.length >= 1);
  // Each tool input pairs with its practice output: the exercise payloads are identical.
  for (const output of practiceOutputs) {
    const input = toolInputs.find((sample) => sample.exercises[0].format === output.exercises[0].format);
    assert.deepEqual(input.exercises, output.exercises);
  }
});
