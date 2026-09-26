import test from 'node:test';
import assert from 'node:assert/strict';
import { usageState, refreshWaitSeconds } from '../src/lib/usage-state.mjs';
const connected = { state: 'connected', error: null, snapshot: { fetched_at: 1000 } };
test('source failures and expired freshness are distinct without clearing usage', () => {
  assert.equal(usageState(null).label, 'Loading');
  assert.equal(usageState({ state: 'disconnected' }).label, 'Disconnected');
  assert.equal(usageState(connected, { now: 2000 }).label, 'Connected');
  assert.equal(usageState(connected, { now: 331001 }).label, 'Stale');
  assert.equal(usageState(connected, { now: 2000, hasError: true, errorCode: 'network' }).label, 'Stale');
  for (const [code, label] of [['permission_denied','Permission needed'], ['schema','Unsupported'], ['endpoint_unavailable','Unsupported'], ['reauth_required','Reconnect'], ['rate_limited','Rate limited']]) {
    assert.equal(usageState(connected, { now: 2000, errorCode: code }).label, label);
  }
  assert.equal(usageState(connected, { errorCode: 'cached_response' }).label, 'Stale');
  assert.equal(usageState({ state: 'connected', snapshot: null }, { errorCode: 'cached_response' }).label, 'Awaiting fresh usage');
  for (const code of ['network', 'secure_storage']) assert.equal(usageState({ state: 'connected', snapshot: null, error: { code } }).label, 'Request failed');
  assert.equal(connected.snapshot.fetched_at, 1000);
});
test('manual retry uses actual backend deadline and rounds upward', () => {
  assert.equal(refreshWaitSeconds(2501, 1000), 2);
  assert.equal(refreshWaitSeconds(1, 1000), 0);
  assert.equal(refreshWaitSeconds(null, 1000), 0);
  assert.equal(refreshWaitSeconds(7200000, 0), 7200);
  assert.equal(usageState(connected, { now: 2000, hasError: true, errorCode: 'cooldown' }).label, 'Connected');
});
