import assert from 'node:assert/strict';
import test from 'node:test';
import { exportResourceCounts, requiredExportPaths, runtimeAttentionCount, uniqueRuntimeIssues, type RuntimeIssue } from '../src/runtime.ts';

test('historical and conditional notes do not inflate package attention, identical reports deduplicate', () => {
  const issue: RuntimeIssue = { category: 'package_required', severity: 'error', source: 'SKILL.md', target: 'scripts/run.py', reason: 'Missing executable' };
  const issues: RuntimeIssue[] = [issue, { ...issue }, { category: 'history', severity: 'info', source: 'references/sources.md', target: 'D:/old/evidence.md', reason: 'Historical source' }, { category: 'external', severity: 'info', source: 'SKILL.md', target: 'Edge', reason: 'Only needed for browser tasks' }];
  assert.equal(uniqueRuntimeIssues(issues).length, 3);
  assert.equal(runtimeAttentionCount(issues), 1);
});

test('ZIP preview counts one physical resource for repeated required references and local precedence', () => {
  const references = [
    { id: 'one', packagePath: 'external/shared.md', required: true, effectiveSource: 'github' },
    { id: 'two', packagePath: 'external/shared.md', required: true, effectiveSource: 'github' },
    { id: 'package', packagePath: 'SKILL.md', required: true, effectiveSource: 'github' },
    { id: 'optional', packagePath: 'external/optional.md', required: false, effectiveSource: 'github' },
  ];
  const files = [{ path: 'SKILL.md', localAvailable: true }];
  assert.deepEqual(exportResourceCounts(['SKILL.md'], [], files, references, []), { externalRequired: 1, remoteCount: 1 });
  assert.deepEqual(exportResourceCounts(['SKILL.md'], [], files, references, ['optional']), { externalRequired: 1, remoteCount: 2 });
});

test('export default ignores old persisted optional runtime choices and includes only present required paths', () => {
  const result = { requiredFiles: ['SKILL.md', 'scripts/run.py', 'scripts/run.py', 'absent.py'], files: [{ path: 'SKILL.md', runtime: true }, { path: 'scripts/run.py', runtime: true }, { path: 'README.md', runtime: true }, { path: 'examples.md', remoteAvailable: true }] };
  assert.deepEqual(requiredExportPaths(result), ['SKILL.md', 'scripts/run.py']);
});

test('uncertain package reads remain visible as actionable rather than silently classified as optional', () => {
  assert.equal(runtimeAttentionCount([{ category: 'uncertain', severity: 'warning', source: 'scripts/run.py', target: 'open(config)', reason: 'Dynamic path needs review' }]), 1);
});

test('export preview scope agrees with the minimal package even when optional README references an image', () => {
  assert.deepEqual(requiredExportPaths({ requiredFiles: ['SKILL.md', 'assets/readme.png'], exportRequiredFiles: ['SKILL.md'], files: [{ path: 'SKILL.md' }, { path: 'assets/readme.png' }] }), ['SKILL.md']);
});
