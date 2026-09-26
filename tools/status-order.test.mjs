import test from 'node:test';
import assert from 'node:assert/strict';
import { createStatusGate } from '../src/lib/status-order.mjs';
test('late focus response cannot restore a disconnected account', async () => {
  const accept = createStatusGate();
  let rendered;
  const apply = status => { if (accept(status)) rendered = status; };
  const oldResponse = Promise.resolve({ revision: 1, state: 'connected', account: 'old' });
  apply({ revision: 2, state: 'disconnected', account: null });
  apply(await oldResponse);
  assert.equal(rendered.state, 'disconnected');
  assert.equal(rendered.account, null);
  apply({ revision: 3, state: 'connected', account: 'new' });
  apply({ revision: 2, state: 'disconnected', account: null });
  assert.equal(rendered.account, 'new');
});
