import assert from 'node:assert/strict';
import test from 'node:test';
import { buildExternalMappings } from '../src/external-mapping.ts';
import { groupExternalReferences, referencePresentationIssues } from '../src/external.ts';

test('GitHub choice overrides local-first presentation without replacing its original path', () => {
  const result = buildExternalMappings([{ id: 'one', readable: true, canonicalPath: 'D:/synthetic/file.json', effectiveSource: 'local', remote: { repositoryPath: 'shared/file.json', commit: 'fixed' } }], 'github');
  assert.deepEqual(result, [{ id: 'one', source: 'github', path: 'D:/synthetic/file.json', repositoryPath: 'shared/file.json', remoteCommit: 'fixed' }]);
});

test('batch preview retains local priority unless GitHub was explicitly requested', () => {
  assert.equal(buildExternalMappings([{ id: 'one', readable: true, effectiveSource: 'local' }])[0].source, 'local');
});

test('a single exact mapping preselects the fixed repository path for a missing local file', () => {
  assert.deepEqual(buildExternalMappings([{ id: 'one', readable: false, candidates: [{ exact: true, repositoryPath: 'skills/one/assets/config.json', commit: 'fixed' }] }])[0], { id: 'one', source: 'github', path: '', repositoryPath: 'skills/one/assets/config.json', remoteCommit: 'fixed' });
});

test('same-name suggestions do not preselect a repository path even if there is only one', () => {
  const result = buildExternalMappings([{ id: 'one', readable: false, remote: { commit: 'fixed' }, candidates: [{ exact: false, repositoryPath: 'elsewhere/config.json', commit: 'fixed' }] }], 'github')[0];
  assert.equal(result.repositoryPath, '');
  assert.equal(result.source, 'github');
});

test('command and current Skill directory aliases need no binding preview, package files remain editable', () => {
  assert.deepEqual(buildExternalMappings([{ id: 'command', readable: false, referenceKind: 'command' }, { id: 'alias', readable: true, referenceKind: 'skill_alias' }, { id: 'file', readable: true, referenceKind: 'package_file' }]).map(item => item.id), ['file']);
});

const commit = 'a'.repeat(40);
const identity = { localPath: 'D:/Skills/one', repo: 'owner/repository', remotePath: 'skills/one', remoteCommit: commit };
function reference(id: string, change: Record<string, unknown> = {}) {
  return { id, canonicalPath: 'D:/Skills/one/scripts/run.ps1', kind: 'file', referenceKind: 'package_file', required: false, bindingStale: false, status: 'available', readable: true, ...change };
}

test('canonical local identity groups multiple references without changing their roles or IDs', () => {
  const first = reference('first', { required: true }), second = reference('second', { role: 'history' });
  const result = groupExternalReferences([first, second], [{ path: 'scripts/run.ps1', localAvailable: true }], identity);
  assert.deepEqual(result.byFile.get('scripts/run.ps1'), [first, second]);
  assert.equal(result.standalone.length, 0);
  assert.equal(result.fileByReference.get('second'), 'scripts/run.ps1');
  assert.equal(second.required, false);
});

test('Windows canonical device prefixes and case normalize while directory aliases remain standalone', () => {
  const result = groupExternalReferences([reference('file', { canonicalPath: '\\\\?\\d:\\skills\\ONE\\scripts\\RUN.ps1' }), reference('directory', { kind: 'directory', referenceKind: 'skill_alias' })], [{ path: 'scripts/run.ps1', localAvailable: true }], identity);
  assert.deepEqual([...result.fileByReference.keys()], ['file']);
  assert.equal(result.standalone[0].id, 'directory');
});

test('fixed commit and case-sensitive full GitHub path prove a remote-only file identity', () => {
  const remote = { status: 'available', repo: 'OWNER/Repository', commit, repositoryPath: 'skills/one/scripts/run.ps1' };
  const result = groupExternalReferences([reference('remote', { canonicalPath: null, remote })], [{ path: 'scripts/run.ps1', localAvailable: false, remoteAvailable: true }], identity);
  assert.equal(result.fileByReference.get('remote'), 'scripts/run.ps1');
  for (const change of [{ commit: 'main' }, { commit: 'b'.repeat(40) }, { repositoryPath: 'skills/one/scripts/RUN.ps1' }, { repo: 'other/repository' }, { status: 'error' }]) {
    const invalid = groupExternalReferences([reference('remote', { canonicalPath: null, remote: { ...remote, ...change } })], [{ path: 'scripts/run.ps1', remoteAvailable: true }], identity);
    assert.equal(invalid.standalone.length, 1);
  }
});

test('same basename, same bytes and ZIP destination alone never prove a file identity', () => {
  const result = groupExternalReferences([reference('outside', { canonicalPath: 'D:/Elsewhere/run.ps1', packagePath: 'scripts/run.ps1', hash: 'same' })], [{ path: 'scripts/run.ps1', localAvailable: true }], identity);
  assert.equal(result.byFile.size, 0);
  assert.equal(result.standalone.length, 1);
});

test('unknown inventory availability, missing local entry and invalid relative paths stay visible', () => {
  for (const file of [{ path: 'scripts/run.ps1' }, { path: 'scripts/run.ps1', localAvailable: true, missing: true }, { path: '../one/scripts/run.ps1', localAvailable: true }]) {
    assert.equal(groupExternalReferences([reference('file')], [file], identity).standalone.length, 1);
  }
});

test('conflicting proven local and remote targets are not silently assigned to either row', () => {
  const item = reference('conflict', { remote: { status: 'available', repo: identity.repo, commit, repositoryPath: 'skills/one/other/run.ps1' } });
  const result = groupExternalReferences([item], [{ path: 'scripts/run.ps1', localAvailable: true }, { path: 'other/run.ps1', remoteAvailable: true }], identity);
  assert.equal(result.byFile.size, 0);
  assert.equal(result.standalone[0], item);
});

test('attached exceptions remain discoverable while a history role alone is not an issue', () => {
  const file = { path: 'scripts/run.ps1', localAvailable: true };
  const item = reference('file', { role: 'history' });
  assert.deepEqual(referencePresentationIssues(item, file, identity), []);
  assert.deepEqual(referencePresentationIssues({ ...item, bindingStale: true, status: 'needs_review' }, file, identity), ['引用或绑定已变化', '需要处理来源']);
  assert.deepEqual(referencePresentationIssues({ ...item, remote: { status: 'available', repo: identity.repo, commit: 'b'.repeat(40), repositoryPath: 'skills/one/scripts/run.ps1' } }, file, identity), ['远端来源与清单不同']);
});

test('remote counterpart never hides a different local physical resource selected for export', () => {
  const item = reference('outside', { canonicalPath: 'D:/Shared/run.ps1', effectiveSource: 'local', remote: { status: 'available', repo: identity.repo, commit, repositoryPath: 'skills/one/scripts/run.ps1' } });
  const file = { path: 'scripts/run.ps1', localAvailable: true, remoteAvailable: true };
  const result = groupExternalReferences([item], [file], identity);
  assert.equal(result.byFile.size, 0);
  assert.equal(result.standalone[0], item);
  assert.deepEqual(referencePresentationIssues(item, file, identity), ['本地来源与清单不同']);
});

test('manual pinned GitHub mismatch stays an explicit issue even with available local bytes', () => {
  const item = reference('manual', { remote: { status: 'needs_selection', repo: identity.repo, commit, repositoryPath: 'skills/one/scripts/run.ps1' } });
  const file = { path: 'scripts/run.ps1', localAvailable: true };
  assert.equal(groupExternalReferences([item], [file], identity).fileByReference.get('manual'), file.path);
  assert.deepEqual(referencePresentationIssues(item, file, identity), ['GitHub 来源需要核对']);
});
