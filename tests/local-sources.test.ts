import test from 'node:test';
import assert from 'node:assert/strict';
import { localPathKey, uniqueCandidates, localCandidates, rangePaths } from '../src/local-sources.ts';

test('preview deduplicates Windows path spellings and preserves Unicode names and descriptions', () => {
  assert.equal(localPathKey('\\\\?\\D:\\Skills\\论文\\'), 'd:/skills/论文');
  const items = uniqueCandidates([{ name: 'a', localPath: 'D:\\Skills\\论文' }, { name: 'a', localPath: 'd:/skills/论文', description: '中文用途' }, { name: 'b', localPath: 'D:/Other/b' }]);
  assert.equal(items.length, 2);
  assert.equal(localCandidates(items, '中文用途').length, 1);
  assert.equal(localCandidates(items, 'other').length, 1);
  assert.equal(localCandidates(items, 'missing').length, 0);
});

test('range selection applies to filtered list without discarding choices outside that list', () => {
  assert.deepEqual(rangePaths(['a','b','c'], ['other'], 'c', 'a', true, true), ['other','a','b','c']);
  assert.deepEqual(rangePaths(['a','b','c'], ['other','a','b','c'], 'c', 'b', false, true), ['other','a']);
  assert.deepEqual(rangePaths(['a','c'], [], 'c', 'hidden', true, true), ['c']);
});
