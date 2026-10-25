import test from 'node:test';
import assert from 'node:assert/strict';
import { activateWorkspacePane, closeWorkspacePane, createHomePane, markdownHeadingAnchor, navigationState, nextPaneId, openWorkspacePane, outlineId, paneAnchorIndex, paneAnchorScrollTop, paneScrollScale, reorderWorkspacePanes, restorePane, restoreWindow, splitRatioForPosition, splitViewForPanes, visibleWorkspacePaneIds, workspaceForStorage, workspaceShortcut, type WorkspacePane, type WindowWorkspace } from '../src/workspace.ts';

const detail = (id: string, skillId = id): WorkspacePane => ({ id, kind: 'detail', title: `Skill ${skillId}`, skillId, state: {} });
const sample = (): WindowWorkspace => ({ label: 'main', panes: [createHomePane(), detail('a'), detail('b')], activePaneId: 'b' });

test('persisted workspace excludes documents, credentials, results and executable drafts recursively', () => {
  const input = { revision: 18, apiToken: 'private', windows: [{ label: 'main', activePaneId: 'export-a', panes: [{ id: 'export-a', kind: 'export', title: '导出', skillId: 'a', markdown: 'private document', preview: { canApply: true }, state: { section: 'files', scrollTop: 50, outlineExpanded: true, token: 'secret', selectedAttachments: ['private'], targetPath: 'D:/secret', result: { submitted: true } } }] }], transfers: [{ payload: { credentials: 'never persist' } }] };
  const result = workspaceForStorage(input); const serialized = JSON.stringify(result);
  assert.equal(result.revision, 18); assert.equal(result.windows[0].activePaneId, 'export-a');
  assert.deepEqual(result.windows[0].panes[1].state, { section: 'files', scrollTop: 50, outlineExpanded: true, reviewRequired: true });
  for (const forbidden of ['private', 'secret', 'canApply', 'selectedAttachments', 'targetPath', 'submitted', 'transfers']) assert(!serialized.includes(forbidden), forbidden);
});

test('restart preserves reading navigation and marks operation pages for fresh review', () => {
  assert.deepEqual(restorePane({ id: 'doc', kind: 'detail', skillId: 'a', state: { documentVersion: 'installed', documentPath: 'references/setup.md', section: 'docs', scrollTop: 288 } }), { id: 'doc', kind: 'detail', skillId: 'a', state: { documentPath: 'references/setup.md', section: 'docs', documentVersion: 'installed', scrollTop: 288 } });
  for (const kind of ['operation', 'runtime', 'export', 'preview', 'associate', 'receipts', 'audit', 'local', 'github']) assert.equal(restorePane({ id: kind, kind, state: { reviewRequired: false } })?.state.reviewRequired, true, kind);
  assert.equal(restorePane({ id: 'task', kind: 'task', state: { taskId: 'result-1' } })?.state.taskId, 'result-1');
});

test('invalid metadata cannot remove the pinned Home or restore unknown executable kinds', () => {
  const window = restoreWindow({ label: 'child', activePaneId: 'gone', panes: [{ id: 'unsafe', kind: 'execute' }, { id: 'dup', kind: 'detail' }, { id: 'dup', kind: 'runtime' }, { id: 'second-home', kind: 'home' }, { id: 'third-home', kind: 'home' }], bounds: { width: 10, height: 20, x: -1920, y: 0, secret: 'none' } });
  assert.deepEqual(window.panes.map(pane => pane.id), ['second-home', 'dup']); assert.equal(window.activePaneId, 'second-home');
  assert.deepEqual(window.bounds, { width: 640, height: 480, x: -1920, y: 0 });
  assert.deepEqual(navigationState({ scrollTop: Infinity, expandedSections: ['runtime', 'runtime', 44], unrelated: true }), { expandedSections: ['runtime'] });
  assert.equal(workspaceForStorage({ windows: [{ label: 'main' }, { label: 'main' }] }).windows.length, 1);
});

test('expanded fold keys roundtrip whole up to 4096 UTF-16 units and retain the entry cap', () => {
  const key = `file:D:/materials/${'文件 😀'.repeat(130)}:references`;
  const limit = '😀'.repeat(2048);
  const state = navigationState({ expandedSections: [`  ${key}  `, key, limit, '😀'.repeat(2049), '', 25] });
  assert.deepEqual(state.expandedSections, [key, limit]);
  const saved = workspaceForStorage({ windows: [{ label: 'main', panes: [{ id: 'long-fold', kind: 'detail', state }], activePaneId: 'long-fold' }] });
  assert.deepEqual(workspaceForStorage(saved).windows[0].panes[1].state.expandedSections, [key, limit]);
  assert.equal(navigationState({ expandedSections: Array.from({ length: 120 }, (_, index) => `fold-${index}`) }).expandedSections?.length, 100);
});

test('detail reuse preserves drafts and separate export sessions remain separate', () => {
  const original = sample(); const reused = openWorkspacePane(original, { ...detail('new-a', 'a'), state: { section: 'files' } });
  assert.equal(reused.activePaneId, 'a'); assert.equal(reused.panes.length, 3); assert.strictEqual(reused.panes[1], original.panes[1]);
  const exported = openWorkspacePane(reused, { id: 'export-1', kind: 'export', skillId: 'a', state: {} });
  const second = openWorkspacePane(exported, { id: 'export-2', kind: 'export', skillId: 'a', state: {} });
  assert.equal(second.panes.length, 5); assert.equal(second.activePaneId, 'export-2'); assert.deepEqual(original, sample());
});

test('close selects a neighbor without closing Home or losing another active page', () => {
  const original = sample(); assert.strictEqual(closeWorkspacePane(original, 'home'), original);
  assert.equal(closeWorkspacePane(original, 'a').activePaneId, 'b'); assert.equal(closeWorkspacePane(original, 'b').activePaneId, 'a');
  const one = closeWorkspacePane(closeWorkspacePane(original, 'b'), 'a'); assert.equal(one.activePaneId, 'home');
  assert.deepEqual(one.panes.map(pane => pane.id), ['home']);
});

test('reordering cannot move Home and does not mutate pane sessions', () => {
  const window = sample(); const original = structuredClone(window);
  assert.deepEqual(reorderWorkspacePanes(window.panes, 'b', 'home').map(pane => pane.id), ['home', 'b', 'a']);
  assert.deepEqual(reorderWorkspacePanes(window.panes, 'home', 'b').map(pane => pane.id), ['home', 'a', 'b']);
  assert.deepEqual(reorderWorkspacePanes(window.panes, 'a').map(pane => pane.id), ['home', 'b', 'a']);
  assert.deepEqual(window, original);
});

test('tab navigation wraps and shortcuts preserve editing undo, system keys and active confirmations', () => {
  const panes = sample().panes;
  assert.equal(nextPaneId(panes, 'b'), 'home'); assert.equal(nextPaneId(panes, 'home', true), 'b');
  const key = { key: 'Tab', ctrlKey: true, altKey: false, metaKey: false, shiftKey: false };
  assert.equal(workspaceShortcut(key), 'next'); assert.equal(workspaceShortcut({ ...key, shiftKey: true }), 'previous');
  assert.equal(workspaceShortcut({ ...key, key: 'W' }), 'close');
  for (const event of [{ ...key, key: 'z' }, { ...key, altKey: true }, { ...key, isComposing: true }, { ...key, defaultPrevented: true }, { ...key, key: 'w', shiftKey: true }]) assert.equal(workspaceShortcut(event), undefined);
  assert.equal(workspaceShortcut(key, true), undefined);
});

test('scoped heading anchors distinguish duplicate Chinese headings and multiple Skills', () => {
  const ids = [outlineId('detail:a', '必要运行文件', 0), outlineId('detail:a', '必要运行文件', 1), outlineId('detail:b', '必要运行文件', 0), outlineId('detail:a', '!!!', 2)];
  assert.equal(new Set(ids).size, 4); for (const id of ids) assert(!/[\s"<>#]/.test(id));
  assert(ids[0].includes('必要运行文件')); assert(ids[3].endsWith('section'));
  assert.notEqual(outlineId(':', 'section', 0), outlineId('_3A', 'section', 0));
});

test('semantic outline anchors survive unrelated entries and distinguish document identities', () => {
  const before = outlineId('detail:a', '必要运行文件', 0, 'runtime-required');
  const after = [outlineId('detail:a', '比较依据', 0, 'comparison'), outlineId('detail:a', '必要运行文件', 0, 'runtime-required')];
  assert.equal(before, after[1]);
  const doc = 'document:references/workflow.md:fixedcommit';
  assert.notEqual(outlineId('detail:a', '使用', 0, doc), outlineId('detail:a', '使用', 1, doc));
  assert.notEqual(outlineId('detail:a', '使用', 0, doc), outlineId('detail:a', '使用', 0, 'document:README.md:fixedcommit'));
  assert.notEqual(outlineId('detail:a', '使用', 0, doc), outlineId('detail:a', '使用', 0, 'document:references/workflow.md:newcommit'));
});

test('pane navigation normalizes CSS zoom and reserves measured sticky coverage exactly once', () => {
  for (const zoom of [0.8, 1, 1.15, 1.5, 2]) {
    const scale = paneScrollScale(750 * zoom, 750);
    const targetRect = 50 + (800 - 200) * zoom;
    const top = paneAnchorScrollTop(200, targetRect, 50, scale, 40 * zoom, 2000);
    assert(Math.abs(top - 744) < 0.00001, `${zoom}: ${top}`);
  }
  assert.equal(paneScrollScale(0, 0), 1);
  assert.equal(paneScrollScale(500, 0), 1);
  assert.equal(paneAnchorScrollTop(0, 10, 10, 1, 40, 1000), 0);
  assert.equal(paneAnchorScrollTop(100, 2000, 0, 1, 40, 500), 500);
});

test('document heading fragments keep readable links and distinguish repeated headings', () => {
  assert.equal(markdownHeadingAnchor('必要文件与说明'), '必要文件与说明');
  assert.equal(markdownHeadingAnchor('Setup / Usage (Windows)'), 'setup--usage-windows');
  assert.equal(markdownHeadingAnchor('使用'), '使用');
  assert.equal(markdownHeadingAnchor('使用', 1), '使用-1');
  assert.equal(markdownHeadingAnchor('使用', 2), '使用-2');
});

test('literal percent-encoded pane IDs resolve before decoding and preserve document anchor priority', () => {
  const id = outlineId('detail:a', '最后章节', 0, 'document:README.md:fixedcommit:最后章节');
  assert(id.includes('%3A'));
  const nodes = [{ id: 'fallback', outlineAnchor: 'setup' }, { id, originalId: 'setup' }, { id: 'literal%20name' }, { id: 'literal name' }];
  assert.equal(paneAnchorIndex(nodes, id), 1, 'Directory passes the literal generated DOM ID');
  assert.equal(paneAnchorIndex(nodes, '#' + id), 1);
  assert.equal(paneAnchorIndex(nodes, '#' + encodeURIComponent(id)), 1, 'URL-encoded document links may decode once as fallback');
  assert.equal(paneAnchorIndex(nodes, '#setup'), 1, 'Original document IDs take precedence over fallback aliases');
  assert.equal(paneAnchorIndex(nodes, '#literal%20name'), 2, 'A literal percent escape must not be redirected to a different ID');
  assert.equal(paneAnchorIndex([{ id: 'literal name' }], '#literal%20name'), 0);
  assert.equal(paneAnchorIndex([{ id: 'invalid%escape' }], '#invalid%escape'), 0);
  assert.equal(paneAnchorIndex(nodes, '#missing'), -1);
});

test('split metadata restores only an owned business pane and never persists arbitrary fields', () => {
  const original = sample();
  assert.equal(restoreWindow(original).splitView, undefined);
  for (const rightPaneId of ['home', 'another-window-pane', '']) assert.equal(restoreWindow({ ...original, splitView: { rightPaneId, ratio: 0.55 } }).splitView, undefined);
  const stored = workspaceForStorage({ windows: [{ ...original, activePaneId: 'home', splitView: { rightPaneId: 'b', ratio: 0.67, token: 'private', document: 'private' } }] });
  assert.deepEqual(stored.windows[0].splitView, { rightPaneId: 'b', ratio: 0.67 });
  assert.equal(stored.windows[0].activePaneId, 'home');
  assert(!JSON.stringify(stored).includes('private'));
  assert.deepEqual(splitViewForPanes({ rightPaneId: 'a', ratio: Infinity }, original.panes), { rightPaneId: 'a', ratio: 0.55 });
  assert.equal(splitViewForPanes({ rightPaneId: 'a', ratio: -4 }, original.panes)?.ratio, 0.1);
  assert.equal(splitViewForPanes({ rightPaneId: 'a', ratio: 4 }, original.panes)?.ratio, 0.9);
});

test('split focus and business navigation preserve one Home and live pane identities', () => {
  const original: WindowWorkspace = { ...sample(), splitView: { rightPaneId: 'b', ratio: 0.55 } };
  const left = activateWorkspacePane(original, 'home');
  assert.equal(left.activePaneId, 'home'); assert.deepEqual(left.splitView, original.splitView);
  assert.deepEqual(visibleWorkspacePaneIds(left.panes, left.activePaneId, left.splitView), ['home', 'b']);
  const right = activateWorkspacePane(left, 'a');
  assert.equal(right.activePaneId, 'a'); assert.deepEqual(right.splitView, { rightPaneId: 'a', ratio: 0.55 });
  assert.deepEqual(visibleWorkspacePaneIds(right.panes, right.activePaneId, right.splitView), ['home', 'a']);
  for (let index = 0; index < original.panes.length; index++) assert.strictEqual(right.panes[index], original.panes[index]);
  assert.strictEqual(right.panes, original.panes);
  const single = { ...right, splitView: undefined };
  assert.deepEqual(visibleWorkspacePaneIds(single.panes, single.activePaneId, single.splitView), ['a']);
  assert.strictEqual(single.panes, original.panes);
  assert.equal(original.activePaneId, 'b');
});

test('closing or reopening a split business page keeps a valid right side without copying sessions', () => {
  const initial: WindowWorkspace = { ...sample(), activePaneId: 'home', splitView: { rightPaneId: 'b', ratio: 0.7 } };
  const reused = openWorkspacePane(initial, detail('duplicate', 'a'));
  assert.equal(reused.panes.length, 3); assert.equal(reused.activePaneId, 'a'); assert.equal(reused.splitView?.rightPaneId, 'a');
  assert.strictEqual(reused.panes[1], initial.panes[1]);
  const closed = closeWorkspacePane(initial, 'b');
  assert.equal(closed.activePaneId, 'home'); assert.deepEqual(closed.splitView, { rightPaneId: 'a', ratio: 0.7 });
  const last = closeWorkspacePane(closed, 'a');
  assert.equal(last.splitView, undefined); assert.deepEqual(visibleWorkspacePaneIds(last.panes, last.activePaneId), ['home']);
  assert.deepEqual(visibleWorkspacePaneIds(initial.panes, 'a', { rightPaneId: 'foreign', ratio: 0.5 }), ['a']);
});

test('divider positions keep each side at least 320 logical pixels and narrow layouts scroll', () => {
  assert.equal(splitRatioForPosition(10, 1200), 320 / 1200);
  assert.equal(splitRatioForPosition(1190, 1200), 880 / 1200);
  assert.equal(splitRatioForPosition(660, 1200), 0.55);
  for (const width of [640, 500, 0, NaN, Infinity]) {
    for (const left of [-100, 200, 900, NaN]) assert.equal(splitRatioForPosition(left, width), 0.5);
  }
  assert.equal(splitRatioForPosition(NaN, 1200), 0.55);
});
