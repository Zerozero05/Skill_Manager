import test from 'node:test';
import assert from 'node:assert/strict';
import { captureSectionFolds, restoreSectionFolds } from '../src/folds.ts';

type FoldFixture = { open: boolean; dataset: Record<string, string>; summary: { dataset: Record<string, string>; textContent: string }; querySelector: (selector: string) => unknown };
function fold(label: string, open = false, foldKey?: string, outlineKey?: string): FoldFixture {
  const summary = { dataset: { outlineLabel: label, ...(outlineKey ? { outlineKey } : {}) }, textContent: `${label} 共 8 项 · 已选 3 项` };
  return { open, dataset: foldKey ? { foldKey } : {}, summary, querySelector(selector) { assert.equal(selector, ':scope > summary'); return summary; } };
}
function root(...folds: FoldFixture[]): HTMLElement {
  return { querySelectorAll(selector: string) { assert.equal(selector, 'details'); return folds; } } as unknown as HTMLElement;
}

test('capture prefers stable fold keys over outline keys and changing display labels', () => {
  const required = fold('必要文件', true, 'files-required', 'old-required');
  const optional = fold('可选包内文件', false, undefined, 'files-optional-package');
  assert.deepEqual(captureSectionFolds(root(required, optional)), ['open:key:files-required', 'closed:key:files-optional-package']);
  required.summary.dataset.outlineLabel = '必要文件（新计数）';
  assert.deepEqual(captureSectionFolds(root(required)), ['open:key:files-required']);
});

test('stable key restore is exact, takes precedence over legacy labels, and runs once', () => {
  const required = fold('必要文件', false, 'files-required'); const restored = new Set<string>();
  restoreSectionFolds(root(required), ['open:key:files-required', 'closed:必要文件'], restored);
  assert.equal(required.open, true);
  required.open = false;
  restoreSectionFolds(root(required), ['open:key:files-required'], restored);
  assert.equal(required.open, false, 'a later mutation must not undo the user toggle');
  const other = fold('必要文件', false, 'files-required-other');
  restoreSectionFolds(root(other), ['open:key:files-required'], new Set()); assert.equal(other.open, false);
});

test('known group label migrations restore previous labels without rewriting state', () => {
  const required = fold('必要文件', false, 'files-required');
  const optional = fold('可选包内文件', true, 'files-optional-package');
  const environment = fold('环境引用', false, 'environment');
  const initial = ['open:必要运行文件', 'closed:可选说明与附加文件', 'open:环境与参考信息', 'unknown:future-state']; const copy = [...initial];
  restoreSectionFolds(root(required, optional, environment), initial, new Set());
  assert.equal(required.open, true); assert.equal(optional.open, false); assert.equal(environment.open, true); assert.deepEqual(initial, copy);
});

test('repeated reference labels retain defaults and only explicit stable keys restore one', () => {
  const first = fold('管理来源与用途', false, 'reference:first:management');
  const second = fold('管理来源与用途', true, 'reference:second:management');
  restoreSectionFolds(root(first, second), ['open:管理来源与用途', 'closed:key:reference:second:management'], new Set());
  assert.equal(first.open, false); assert.equal(second.open, false);
  const single = fold('引用 1 处', true, 'file:one.md:references');
  restoreSectionFolds(root(single), ['closed:引用 1 处'], new Set()); assert.equal(single.open, true);
  assert.deepEqual(captureSectionFolds(root(fold('管理来源与用途'), fold('管理来源与用途', true))), []);
});

test('conflicting saved states or duplicate keys never choose an arbitrary section', () => {
  const first = fold('必要文件', false, 'files-required'); const duplicate = fold('必要文件', true, 'files-required');
  restoreSectionFolds(root(first, duplicate), ['open:key:files-required'], new Set());
  assert.equal(first.open, false); assert.equal(duplicate.open, true); assert.deepEqual(captureSectionFolds(root(first, duplicate)), []);
  restoreSectionFolds(root(first), ['open:key:files-required', 'closed:key:files-required', 'open:必要文件'], new Set()); assert.equal(first.open, false);
  restoreSectionFolds(root(first), ['open:必要运行文件', 'closed:必要文件'], new Set()); assert.equal(first.open, false);
});

test('late mounted sections restore independently while ambiguous legacy aliases keep defaults', () => {
  const late = fold('可选包内文件', false, 'files-optional-package'); const restored = new Set<string>();
  restoreSectionFolds(root(), ['open:key:files-optional-package'], restored);
  restoreSectionFolds(root(late), ['open:key:files-optional-package'], restored); assert.equal(late.open, true);
  const oldGroup = fold('必要运行文件', true, 'legacy-group'); const newGroup = fold('必要文件', false, 'files-required');
  restoreSectionFolds(root(oldGroup, newGroup), ['open:必要运行文件'], new Set());
  assert.equal(newGroup.open, false);
});

test('unique unkeyed legacy sections preserve their exact label state', () => {
  const comparison = fold('比较依据', true); const blank = fold(''); blank.summary.textContent = '';
  assert.deepEqual(captureSectionFolds(root(comparison, blank)), ['open:比较依据']);
  restoreSectionFolds(root(comparison), ['closed:比较依据'], new Set()); assert.equal(comparison.open, false);
});
