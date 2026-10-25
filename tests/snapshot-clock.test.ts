import assert from 'node:assert/strict';
import test from 'node:test';
import { isStaleReadSnapshot } from '../src/snapshot-clock.ts';

test('a late same-revision reply cannot restore an older credential cache namespace', () => {
  assert.equal(isStaleReadSnapshot({ stateRevision: 10, readGeneration: 2 }, 10, 3), true);
  assert.equal(isStaleReadSnapshot({ stateRevision: 11, readGeneration: 2 }, 10, 3), true);
  assert.equal(isStaleReadSnapshot({ stateRevision: 10, readGeneration: 4 }, 10, 3), false);
});

test('unknown read generations are not zero and do not erase the accepted watermark', () => {
  assert.equal(isStaleReadSnapshot({ stateRevision: 10, readGeneration: null }, 10, 3), false);
  assert.equal(isStaleReadSnapshot({ stateRevision: 10 }, 10, 3), false);
  assert.equal(isStaleReadSnapshot({ stateRevision: 10, readGeneration: 2 }, 10, 3), true);
});

test('state revision ordering remains authoritative across independent read-generation changes', () => {
  assert.equal(isStaleReadSnapshot({ stateRevision: 9, readGeneration: 4 }, 10, 3), true);
  assert.equal(isStaleReadSnapshot({ stateRevision: 11, readGeneration: 3 }, 10, 3), false);
  assert.equal(isStaleReadSnapshot({ stateRevision: 0, readGeneration: 0 }, -1), false);
});
