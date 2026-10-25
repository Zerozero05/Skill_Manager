import assert from 'node:assert/strict';
import test from 'node:test';
import { sameRuntimeReference, runtimeIssuesForReference, unattachedRuntimeIssues } from '../src/runtime-presentation.ts';
import type { RuntimeIssue } from '../src/runtime.ts';
import { runtimeAttentionCount } from '../src/runtime.ts';

const issue: RuntimeIssue = { category: 'external', severity: 'warning', source: 'SKILL.md', target: 'D:/shared/AGENTS.md', reason: '原始扫描原因须保留。' };

test('runtime records attach only to the complete source and target, retaining the original reason', () => {
  const references = [{ source: 'SKILL.md', target: 'D:\\shared\\AGENTS.md' }];
  assert.equal(sameRuntimeReference(issue, references[0]), true);
  assert.deepEqual(runtimeIssuesForReference([issue, { ...issue }], references[0]), [issue]);
  assert.deepEqual(unattachedRuntimeIssues([issue], references), []);
  assert.equal(runtimeIssuesForReference([issue], references[0])[0].reason, '原始扫描原因须保留。');
});

test('same filenames and different citing documents remain independent scan records', () => {
  const checks = [issue, { ...issue, source: 'references/guide.md' }, { ...issue, target: 'D:/other/AGENTS.md' }, { ...issue, target: 'D:/shared/agents.md' }];
  assert.deepEqual(unattachedRuntimeIssues(checks, [issue]), checks.slice(1));
  assert.deepEqual(runtimeIssuesForReference(checks, issue), [issue]);
});

test('different categories and reasons at the same complete reference are preserved', () => {
  const other: RuntimeIssue = { ...issue, category: 'history', severity: 'info', reason: '历史用途与可用性分别记录。' };
  assert.deepEqual(runtimeIssuesForReference([issue, other, issue], issue), [issue, other]);
  assert.deepEqual(unattachedRuntimeIssues([issue, other], []), [issue, other]);
});

test('ordinary external warning notes stay available without presenting every valid reference as a problem', () => {
  const explanatory = runtimeIssuesForReference([issue], issue);
  assert.equal(runtimeAttentionCount(explanatory), 0);
  assert.equal(explanatory[0].reason, issue.reason);
  const uncertain: RuntimeIssue = { ...issue, category: 'uncertain', reason: '真正不确定的包内引用。' };
  assert.equal(runtimeAttentionCount(runtimeIssuesForReference([issue, uncertain], issue)), 1);
});
