import test from 'node:test';
import assert from 'node:assert/strict';
import { normalizeEvent, reduceEvent } from './codex-events.mjs';
const event = (type, turnId, second, extra = {}) => normalizeEvent({
  type: 'event_msg', timestamp: `2026-09-26T00:00:${String(second).padStart(2, '0')}Z`,
  payload: { type, turn_id: turnId, ...extra },
});

test('projects only allowlisted metadata and never transcript/error content', () => {
  const e = event('task_complete', 'run-1', 3, { last_agent_message: 'private', error: { message: 'private' } });
  assert.deepEqual(Object.keys(e).sort(), ['status', 'timestamp', 'turnId', 'type']);
  assert.equal(e.status, 'failed');
  assert.ok(!JSON.stringify(e).includes('private'));
});
test('start and completion are explicit; silence has no transition', () => {
  let state = reduceEvent(null, event('task_started', 'run-1', 1));
  assert.equal(state.status, 'running');
  assert.strictEqual(reduceEvent(state, null), state);
  state = reduceEvent(state, event('task_complete', 'run-1', 2));
  assert.equal(state.status, 'completed');
});
test('late previous-run completion and duplicate start cannot corrupt a new run', () => {
  let state = reduceEvent(null, event('task_started', 'old', 1));
  state = reduceEvent(state, event('task_started', 'new', 2));
  assert.strictEqual(reduceEvent(state, event('task_complete', 'old', 3)), state);
  state = reduceEvent(state, event('task_complete', 'new', 4));
  assert.strictEqual(reduceEvent(state, event('task_started', 'new', 5)), state);
  assert.strictEqual(reduceEvent(state, event('task_started', 'old', 1)), state);
});
test('unknown events and malformed identity/time cannot fabricate a state', () => {
  assert.equal(event('agent_message', 'run-1', 1), null);
  assert.equal(event('task_started', '', 1), null);
  assert.equal(normalizeEvent({type:'event_msg', timestamp:'invalid', payload:{type:'task_started',turn_id:'x'}}), null);
});

test('explicit completion can restore terminal evidence without a captured start', () => {
  assert.equal(reduceEvent(null, event('task_complete', 'run-1', 1)).status, 'completed');
});
