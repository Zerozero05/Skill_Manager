import test from 'node:test';
import assert from 'node:assert/strict';
import { filterOperationHistory, operationHistoryDraft, operationSummaryPlacement, operationSummaryTasks } from '../src/operation-history.ts';
import { closeWorkspacePane, createHomePane, localHistoryPane, openWorkspacePane, paneTitle, restorePane, visibleWorkspacePaneIds, workspaceForStorage, type WorkspaceNavigation, type WorkspacePane } from '../src/workspace.ts';
import type { ManagerTask } from '../src/tasks.ts';

const history = (id: string): WorkspacePane => ({ id, kind: 'history', state: {} });
const task = (id: string, status: string, extras: Partial<ManagerTask> = {}): ManagerTask => ({ id, status, action: 'synthetic-read', label: `测试操作 ${id}`, stage: '检查资料', createdAt: '2026-10-25T08:00:00Z', origin: { paneId: 'detail-a', windowLabel: 'main' }, cancellable: status === 'running', ...extras });

test('history restores navigation without executable drafts or fresh-review state', () => {
  const restored = restorePane({ id: 'history-a', kind: 'history', state: { scrollTop: 420, reviewRequired: true, search: 'private query', status: 'failed', request: { action: 'apply' }, result: 'private result' } });
  assert.deepEqual(restored, { id: 'history-a', kind: 'history', state: { scrollTop: 420 } });
  assert.equal(paneTitle(restored!), '操作记录');
  const saved = workspaceForStorage({ revision: 8, windows: [{ label: 'main', panes: [history('history-a')], activePaneId: 'history-a' }] });
  assert.equal(saved.revision, 8);
  assert.equal(saved.windows[0].activePaneId, 'history-a');
  assert.equal(saved.windows[0].panes[1].kind, 'history');
  assert(!JSON.stringify(saved).includes('reviewRequired'));
});

test('sidebar history lookup reuses only the current window and preserves existing views', () => {
  const first = history('history-main'); const second = history('history-main-second'); const other = history('history-child');
  const workspace: WorkspaceNavigation = { revision: 1, windows: [{ label: 'main', panes: [createHomePane(), first, second], activePaneId: second.id }, { label: 'workspace-child', panes: [createHomePane('home:workspace-child'), other], activePaneId: other.id }] };
  assert.strictEqual(localHistoryPane(workspace, 'main'), first);
  assert.strictEqual(localHistoryPane(workspace, 'workspace-child'), other);
  assert.equal(localHistoryPane(workspace, 'workspace-empty'), undefined);
  const childOnly = { windows: [workspace.windows[1], { label: 'main', panes: [createHomePane()], activePaneId: 'home' }] };
  assert.equal(localHistoryPane(childOnly, 'main'), undefined, 'A history in another window must not steal the sidebar action');
  assert.equal(workspace.windows[0].activePaneId, second.id);
});

test('intentional history views retain independent identities through generic open, split and close', () => {
  const window = { label: 'main', panes: [createHomePane(), history('history-a')], activePaneId: 'history-a' };
  const opened = openWorkspacePane(window, history('history-b'));
  assert.deepEqual(opened.panes.map(pane => pane.id), ['home', 'history-a', 'history-b']);
  assert.strictEqual(opened.panes[1], window.panes[1]);
  assert.deepEqual(visibleWorkspacePaneIds(opened.panes, opened.activePaneId, { rightPaneId: 'history-b', ratio: .55 }), ['home', 'history-b']);
  const closed = closeWorkspacePane(opened, 'history-b');
  assert.equal(closed.activePaneId, 'history-a');
  assert.strictEqual(closed.panes[1], window.panes[1]);
  assert.equal(window.panes.length, 2);
});

test('history filtering keeps shared record order and distinguishes active, ended and attention states', () => {
  const tasks = [task('queued', 'queued'), task('running', 'running'), task('done', 'succeeded'), task('partial', 'partial'), task('failed', 'failed', { error: '网络暂时不可用\n请检查连接' }), task('cancelled', 'cancelled'), task('interrupted', 'interrupted')];
  const before = structuredClone(tasks);
  assert.deepEqual(filterOperationHistory(tasks, { search: '', status: 'active' }).map(item => item.id), ['queued', 'running']);
  assert.deepEqual(filterOperationHistory(tasks, { search: '', status: 'ended' }).map(item => item.id), ['done', 'partial', 'failed', 'cancelled', 'interrupted']);
  assert.deepEqual(filterOperationHistory(tasks, { search: '', status: 'failed' }).map(item => item.id), ['partial', 'failed', 'interrupted']);
  assert.deepEqual(filterOperationHistory(tasks, { search: ' 检查连接 ', status: 'all' }).map(item => item.id), ['failed']);
  assert.deepEqual(filterOperationHistory(tasks, { search: '排队中', status: 'all' }).map(item => item.id), ['queued']);
  assert.deepEqual(filterOperationHistory(tasks, { search: 'DONE', status: 'ended' }).map(item => item.id), ['done']);
  assert.deepEqual(tasks, before);
  assert.strictEqual(filterOperationHistory(tasks, operationHistoryDraft({}))[0], tasks[0]);
});

test('each history view owns a validated runtime draft without changing another filter', () => {
  const transferred = operationHistoryDraft({ search: '目标 Skill', status: 'failed', scrollTop: 88 });
  const other = operationHistoryDraft(undefined);
  assert.deepEqual(transferred, { search: '目标 Skill', status: 'failed' });
  assert.deepEqual(other, { search: '', status: 'all' });
  transferred.search = '独立搜索';
  assert.deepEqual(other, { search: '', status: 'all' });
  assert.deepEqual(operationHistoryDraft({ search: 12, status: 'execute' }), { search: '', status: 'all' });
});

test('sidebar summary selects five visible tasks with active first without touching records', () => {
  const tasks = [task('hidden', 'running', { hidden: true }), task('old-active', 'running', { createdAt: '2026-10-24T08:00:00Z' }), task('new-done', 'succeeded', { createdAt: '2026-10-25T11:00:00Z' }), task('new-active', 'queued', { createdAt: '2026-10-25T10:00:00Z' }), task('done-3', 'failed'), task('done-2', 'partial'), task('done-1', 'succeeded')];
  const before = structuredClone(tasks), rows = operationSummaryTasks(tasks);
  assert.equal(rows.length, 5);
  assert.deepEqual(rows.map(item => item.id), ['new-active', 'old-active', 'new-done', 'done-1', 'done-2']);
  assert.strictEqual(rows[0], tasks[3]);
  assert.deepEqual(tasks, before);
  assert.deepEqual(operationSummaryTasks(tasks, 0), []);
});

test('summary fits the visible main area at the sidebar edge and never flips onto categories', () => {
  const anchor = { left: 20, top: 600, right: 210, bottom: 640 };
  const bounds = { left: 224, top: 45, right: 1200, bottom: 700 };
  const wide = operationSummaryPlacement(anchor, bounds, 480);
  assert.equal(wide.left, 232); assert.equal(wide.width, 360); assert.equal(wide.compact, false);
  assert(wide.top >= bounds.top + 8 && wide.top + 480 <= bounds.bottom - 8);
  const narrow = operationSummaryPlacement(anchor, { ...bounds, right: 410 }, 480);
  assert.equal(narrow.left, wide.left); assert.equal(narrow.width, 170); assert.equal(narrow.compact, true);
  assert(narrow.left + narrow.width <= 402);
  const small = operationSummaryPlacement(anchor, { ...bounds, bottom: 200 }, 480);
  assert.equal(small.maxHeight, 139); assert.equal(small.compact, true);
  assert(small.top + small.maxHeight <= 192);
});
