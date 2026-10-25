import assert from 'node:assert/strict';
import test from 'node:test';
import { acknowledgeCloseRound, BRIDGE_RETRY_DELAYS, bridgeMessage, isBridgeFailure, pendingCloseRound, readCloseBounds, RecoverableSubscription, retryBridgeRead, selectionWriteConfirmed, shouldPeekSelection, shouldPollWorkspace } from '../src/bridge-recovery.ts';
import { BoundedReadCache, classifyReadFailure } from '../src/read-cache.ts';

const disconnected = () => new Error('runtime error: failed to send message to the webview');

test('bridge reads retry only twice at the fixed delays and return the recovered value', async () => {
  let calls = 0;
  const delays: number[] = [];
  const value = await retryBridgeRead(async () => { if (++calls < 3) throw disconnected(); return 'snapshot'; }, { wait: async ms => { delays.push(ms); } });
  assert.equal(value, 'snapshot'); assert.equal(calls, 3);
  assert.deepEqual(delays, [...BRIDGE_RETRY_DELAYS]);
});

test('unavailable IPC stops after its budget and does not retry source or permission failures', async () => {
  let calls = 0;
  await assert.rejects(retryBridgeRead(async () => { calls++; throw disconnected(); }, { wait: async () => undefined }), /webview/);
  assert.equal(calls, 3);
  calls = 0;
  await assert.rejects(retryBridgeRead(async () => { calls++; throw new Error('403 forbidden'); }, { wait: async () => { assert.fail('permission failures must not retry'); } }), /403/);
  assert.equal(calls, 1);
});

test('leaving the owner cancels retries before another request starts', async () => {
  let active = true, calls = 0;
  await assert.rejects(retryBridgeRead(async () => { calls++; throw disconnected(); }, { active: () => active, wait: async () => { active = false; } }), /已停止/);
  assert.equal(calls, 1);
});

test('shared reads retain another live waiter while cancelling a departed owner', async () => {
  const cache = new BoundedReadCache<string>();
  let firstAlive = true, calls = 0;
  const first = cache.load('shared', active => retryBridgeRead(async () => { if (++calls === 1) throw disconnected(); return 'current'; }, { active, wait: async () => { firstAlive = false; } }), false, () => firstAlive);
  const second = cache.load('shared', async () => { assert.fail('must reuse the first request'); }, false, () => true);
  assert.equal(first, second);
  assert.deepEqual(await Promise.all([first, second]), ['current', 'current']);
  assert.equal(calls, 2);
});

test('a shared read stops retries after every waiter has departed', async () => {
  const cache = new BoundedReadCache<string>();
  let alive = true, calls = 0;
  const read = cache.load('gone', active => retryBridgeRead(async () => { calls++; throw disconnected(); }, { active, wait: async () => { alive = false; } }), false, () => alive);
  await assert.rejects(read, /已停止/);
  assert.equal(calls, 1); assert.equal(cache.peek('gone'), undefined);
});

test('a forced refresh does not lend its new owner to an obsolete retry', async () => {
  const cache = new BoundedReadCache<string>();
  let oldAlive = true, oldCalls = 0, continueOld!: () => void, entered!: () => void;
  const delayed = new Promise<void>(resolve => { entered = resolve; });
  const old = cache.load('same', active => retryBridgeRead(async () => { oldCalls++; throw disconnected(); }, { active, wait: () => new Promise<void>(resolve => { continueOld = resolve; entered(); }) }), false, () => oldAlive);
  await delayed; oldAlive = false;
  const fresh = cache.load('same', async () => 'fresh', true, () => true);
  continueOld();
  await assert.rejects(old, /已停止/);
  assert.equal(await fresh, 'fresh'); assert.equal(oldCalls, 1); assert.equal(cache.peek('same'), 'fresh');
});

test('subscription failure is isolated and a deliberate recovery catches up once', async () => {
  let available = false, attempts = 0, ready = 0, unavailable = 0, released = 0;
  const subscription = new RecoverableSubscription(async () => { attempts++; if (!available) throw disconnected(); return () => { released++; }; }, { wait: async () => undefined, onReady: () => { ready++; }, onUnavailable: () => { unavailable++; } });
  await subscription.recover();
  assert.equal(attempts, 3); assert.equal(unavailable, 1); assert.equal(ready, 0); assert.equal(subscription.connected, false);
  available = true;
  await Promise.all([subscription.recover(), subscription.recover()]);
  assert.equal(attempts, 4); assert.equal(ready, 1); assert.equal(subscription.connected, true);
  await subscription.recover(); assert.equal(attempts, 4);
  subscription.dispose(); assert.equal(released, 1);
});

test('late listener registration is released after teardown without announcing readiness', async () => {
  let finish!: (stop: () => void) => void, released = 0, ready = 0;
  const registration = new Promise<() => void>(resolve => { finish = resolve; });
  const subscription = new RecoverableSubscription(() => registration, { onReady: () => { ready++; } });
  const started = subscription.recover(); subscription.dispose(); finish(() => { released++; });
  await started;
  assert.equal(released, 1); assert.equal(ready, 0); assert.equal(subscription.connected, false);
});

test('degraded snapshots only poll a visible live owner with working ordinary IPC', () => {
  const state = { alive: true, visible: true, owned: true, ipcAvailable: true, connected: false };
  assert.equal(shouldPeekSelection(state), true);
  for (const key of ['alive', 'visible', 'owned', 'ipcAvailable'] as const) assert.equal(shouldPeekSelection({ ...state, [key]: false }), false, key);
  assert.equal(shouldPeekSelection({ ...state, connected: true }), false);
});

test('lost selection acknowledgements are checked from state without replaying writes', () => {
  const before = { id: 'draft-one', revision: 3, selectedIds: [] as string[] };
  const after = { ...before, revision: 4, selectedIds: ['file:README.md'] };
  assert.equal(selectionWriteConfirmed(before, after, { operation: 'toggle', resourceId: 'file:README.md', included: true }), true);
  assert.equal(selectionWriteConfirmed(before, before, { operation: 'toggle', resourceId: 'file:README.md', included: true }), false);
  assert.equal(selectionWriteConfirmed(before, { ...after, id: 'next-draft' }, { operation: 'set', selectedIds: after.selectedIds }), false);
  assert.equal(selectionWriteConfirmed(before, after, { operation: 'set', selectedIds: ['file:README.md', 'file:README.md'] }), true);
  assert.equal(selectionWriteConfirmed(after, { ...after, revision: 5, selectedIds: [] }, { operation: 'clear', draftId: before.id, expectedRevision: 3 }), false);
  assert.equal(selectionWriteConfirmed(after, { ...after, revision: 5, selectedIds: [] }, { operation: 'clear', draftId: before.id, expectedRevision: 4 }), true);
});

test('bridge failures are classified without masking file, source, or authentication errors', () => {
  assert.equal(isBridgeFailure(disconnected()), true);
  assert.equal(classifyReadFailure(disconnected()), 'bridge');
  assert.equal(classifyReadFailure('401 unauthorized'), 'auth');
  assert.equal(classifyReadFailure('404 not found'), 'source');
  assert.equal(classifyReadFailure('network timed out'), 'network');
  assert.equal(bridgeMessage(disconnected()).includes('重新核对'), true);
  assert.equal(bridgeMessage('specific file error'), 'specific file error');
});

test('a close ACK which never arrived is sent again only after proving the same pending round', async () => {
  const round = { closeId: 'close-one', round: 'final' };
  const order: string[] = [];
  await acknowledgeCloseRound(round, async () => { order.push('ack'); if (order.length === 1) throw disconnected(); }, async () => { order.push('read'); return { closeRequest: round }; }, () => true);
  assert.deepEqual(order, ['ack', 'read', 'ack']);
});

test('an applied ACK with a lost reply is not sent again or changed to a negative ACK', async () => {
  let sent = 0, reads = 0;
  await acknowledgeCloseRound({ closeId: 'close-one' }, async () => { sent++; throw disconnected(); }, async () => { reads++; return { closeRequest: null }; }, () => true);
  assert.equal(sent, 1); assert.equal(reads, 1);
});

test('a superseded close round or expired window never receives a retried ACK', async () => {
  const initial = { closeId: 'close-one', round: 'initial' };
  for (const replacement of [{ closeId: 'close-two', round: 'initial' }, { closeId: 'close-one', round: 'final' }]) {
    let sent = 0;
    await acknowledgeCloseRound(initial, async () => { sent++; throw disconnected(); }, async () => ({ closeRequest: replacement }), () => true);
    assert.equal(sent, 1); assert.equal(pendingCloseRound(initial, { closeRequest: replacement }), false);
  }
  let alive = true, reads = 0;
  await assert.rejects(acknowledgeCloseRound(initial, async () => { alive = false; throw disconnected(); }, async () => { reads++; return { closeRequest: initial }; }, () => alive), /webview/);
  assert.equal(reads, 0);
});

test('recovery preserves a failed flush ACK result and exhausts only one pending-proof resend', async () => {
  const round = { closeId: 'close-one', round: 'initial' };
  const failed = { ...round, ok: false, error: 'synthetic save failure' };
  const sent: unknown[] = [];
  await assert.rejects(acknowledgeCloseRound(round, async () => { sent.push(failed); throw disconnected(); }, async () => ({ closeRequest: round }), () => true), /webview/);
  assert.deepEqual(sent, [failed, failed]);
});

test('hidden windows poll only degraded closing controls with working IPC', () => {
  const healthy = { alive: true, visible: true, ipcAvailable: true, degraded: false, closeDegraded: false };
  assert.equal(shouldPollWorkspace(healthy), false);
  assert.equal(shouldPollWorkspace({ ...healthy, degraded: true }), true);
  assert.equal(shouldPollWorkspace({ ...healthy, visible: false, degraded: true }), false);
  assert.equal(shouldPollWorkspace({ ...healthy, visible: false, degraded: true, closeDegraded: true }), true);
  assert.equal(shouldPollWorkspace({ ...healthy, visible: false, degraded: true, closeDegraded: true, ipcAvailable: false }), false);
  assert.equal(shouldPollWorkspace({ ...healthy, degraded: true, closeDegraded: true, alive: false }), false);
});

test('unavailable native bounds exhaust their read budget and omit replacement geometry', async () => {
  let calls = 0;
  const delays: number[] = [];
  const bounds = await readCloseBounds(async () => { calls++; throw disconnected(); }, { wait: async ms => { delays.push(ms); } });
  assert.equal(bounds, undefined); assert.equal(calls, 3); assert.deepEqual(delays, [...BRIDGE_RETRY_DELAYS]);
});

test('native bounds recover without swallowing other errors or an owner leaving', async () => {
  const expected = { x: 10, y: 20, width: 1200, height: 800, maximized: false };
  let calls = 0;
  assert.deepEqual(await readCloseBounds(async () => { if (++calls === 1) throw disconnected(); return expected; }, { wait: async () => undefined }), expected);
  await assert.rejects(readCloseBounds(async () => { throw new Error('window permission denied'); }), /permission denied/);
  let active = true;
  await assert.rejects(readCloseBounds(async () => { throw disconnected(); }, { active: () => active, wait: async () => { active = false; } }), /已停止/);
});

test('closing controls keep polling connected visible or hidden windows only for their close lifecycle', () => {
  const state = { alive: true, visible: true, ipcAvailable: true, degraded: false, closeDegraded: false, closing: true };
  assert.equal(shouldPollWorkspace(state), true);
  assert.equal(shouldPollWorkspace({ ...state, visible: false }), true);
  assert.equal(shouldPollWorkspace({ ...state, closing: false }), false);
  assert.equal(shouldPollWorkspace({ ...state, ipcAvailable: false }), false);
  assert.equal(shouldPollWorkspace({ ...state, alive: false }), false);
});
