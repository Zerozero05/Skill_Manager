import assert from 'node:assert/strict';
import test from 'node:test';
import { BoundedReadCache, classifyReadFailure, documentCacheKey, inventoryReadKey, readRequestKey } from '../src/read-cache.ts';

test('file refresh invalidates all generations of only its Skill inventory', () => {
  const cache = new BoundedReadCache<{ files: string[] }>();
  const first = inventoryReadKey('one', 1, 'content-a'), later = inventoryReadKey('one', 2, 'content-a');
  const unrelated = inventoryReadKey('two', 1, 'content-b');
  for (const key of [first, later, unrelated]) cache.set(key, { files: ['old.md'] });
  for (const key of cache.keys()) if (JSON.parse(key)[0] === 'one') cache.delete(key);
  assert.equal(cache.peek(first), undefined); assert.equal(cache.peek(later), undefined);
  assert.deepEqual(cache.peek(unrelated), { files: ['old.md'] });
  cache.set(first, { files: ['old.md', 'new-optional.md'] });
  assert.deepEqual(cache.peek(first)?.files, ['old.md', 'new-optional.md']);
});

test('display cache evicts by both size and recency without changing its returned values', () => {
  const cache = new BoundedReadCache<string>(2, 10, 8);
  assert(cache.set('a', 'a')); assert(cache.set('b', 'b')); cache.get('a');
  assert(cache.set('c', 'c')); assert.equal(cache.peek('b'), undefined); assert.equal(cache.peek('a'), 'a');
  assert.deepEqual([...cache.keys()], ['a', 'c']);
  assert.equal(cache.set('too-large', '123456789'), false); assert.equal(cache.statistics.entries, 2);
  cache.set('bytes', '123456'); assert(cache.statistics.bytes <= 10); cache.clear(); assert.equal(cache.statistics.bytes, 0);
});

test('cached document freshness expires at thirty seconds and does not renew on a read', () => {
  let now = 1; const cache = new BoundedReadCache<string>(2, 100, 100, () => now);
  cache.set('doc', 'unchanged'); now += 29_999; assert(cache.fresh('doc', 30_000)); cache.get('doc');
  now += 1; assert.equal(cache.fresh('doc', 30_000), false); assert.equal(cache.peek('doc'), 'unchanged');
});

test('equivalent concurrent reads share a promise and errors can be retried without cached failure', async () => {
  const cache = new BoundedReadCache<string>(); let calls = 0; let finish!: (value: string) => void;
  const reader = () => { calls++; return new Promise<string>(resolve => { finish = resolve; }); };
  const one = cache.load('doc', reader), two = cache.load('doc', reader); assert.equal(one, two);
  await Promise.resolve(); assert.equal(calls, 1); finish('value'); assert.equal(await two, 'value');
  await assert.rejects(cache.load('failure', async () => { throw Error('network'); }));
  assert.equal(cache.peek('failure'), undefined); assert.equal(await cache.load('failure', async () => 'retried'), 'retried');
});

test('a forced manifest refresh starts a new read and an older late reply cannot replace its evidence', async () => {
  const cache = new BoundedReadCache<{ canExport: boolean }>();
  let oldReply!: (value: { canExport: boolean }) => void, freshReply!: (value: { canExport: boolean }) => void;
  let reads = 0;
  const old = cache.load('same-selection', () => { reads++; return new Promise(resolve => { oldReply = resolve; }); });
  const fresh = cache.load('same-selection', () => { reads++; return new Promise(resolve => { freshReply = resolve; }); }, true);
  await Promise.resolve(); assert.equal(reads, 2); assert.notEqual(old, fresh);
  freshReply({ canExport: false }); assert.deepEqual(await fresh, { canExport: false });
  oldReply({ canExport: true }); await old;
  assert.deepEqual(cache.peek('same-selection'), { canExport: false });
});

test('pending deduplication bookkeeping is bounded while every original waiter can finish', async () => {
  const cache = new BoundedReadCache<number>(2, 100, 100); const resolvers: ((value: number) => void)[] = [];
  const waits = Array.from({ length: 10 }, (_, index) => cache.load(String(index), () => new Promise(resolve => { resolvers.push(resolve); })));
  await Promise.resolve(); assert.equal(cache.statistics.pending, 2); resolvers.forEach((resolve, index) => resolve(index));
  assert.deepEqual(await Promise.all(waits), Array.from({ length: 10 }, (_, index) => index)); assert(cache.statistics.entries <= 2);
});

test('document identity separates authorization, source, immutable commit, path and local byte changes', () => {
  const base = { readGeneration: 2, skillId: 'same', sourceId: 'source', repo: 'fixture/repo', branch: 'main', remotePath: 'one', commit: 'a'.repeat(40), version: 'latest', path: 'README.md', origin: 'github', localPath: 'D:/synthetic', localFingerprint: { 'SKILL.md': 'hash' } };
  const key = documentCacheKey(base); const cache = new BoundedReadCache<string>(); cache.set(key, 'private doc');
  for (const patch of [{ readGeneration: 3 }, { sourceId: 'other' }, { repo: 'fixture/other' }, { commit: 'b'.repeat(40) }, { path: 'references/other.md' }, { origin: 'local' }, { localFingerprint: { 'SKILL.md': 'changed' } }]) {
    const other = documentCacheKey({ ...base, ...patch }); assert.notEqual(other, key); assert.equal(cache.peek(other), undefined);
  }
  assert(!key.includes('token')); assert.notEqual(documentCacheKey({ ...base, readGeneration: null }), key);
});

test('manual document recheck changes the active request identity without fragmenting the shared document cache', () => {
  const key = documentCacheKey({ skillId: 'one', readGeneration: 2, commit: 'a'.repeat(40), path: 'README.md' });
  const cache = new BoundedReadCache<string>(); cache.set(key, 'same-context document');
  assert.notEqual(readRequestKey(key, 0), readRequestKey(key, 1));
  assert.equal(cache.peek(key), 'same-context document');
});

test('authorization and source failures are distinct from a recoverable same-context network interruption', () => {
  assert.equal(classifyReadFailure('GitHub returned 401'), 'auth'); assert.equal(classifyReadFailure('403 Forbidden'), 'auth');
  assert.equal(classifyReadFailure('文件不存在'), 'source'); assert.equal(classifyReadFailure('读取来源或访问凭据已变化'), 'auth');
  assert.equal(classifyReadFailure('network connection timed out'), 'network'); assert.equal(classifyReadFailure('429 rate limit'), 'network');
  assert.equal(classifyReadFailure('Unexpected result format'), 'other');
});
