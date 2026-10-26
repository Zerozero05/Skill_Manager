import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';
import { clientConnectChoices, clientConnectDraft, clientConnectionRows, clientSelectionRange, clientSelectionShortcut, clientUsageText, completedClientBatchDraft, sameReadyClientConnection, selectFilteredClientItems, type ClientConnection, type ClientSkill } from '../src/client-selection.ts';

const skills: ClientSkill[] = [{ id: 'one', name: 'one', displayName: '工具一', localPath: 'D:\\Skills\\one' }, { id: 'two', name: 'two', localPath: 'D:/Skills/two' }, { id: 'readonly', name: 'managed', localPath: 'D:/managed', readOnly: true }, { id: 'remote', name: 'remote' }];
const connection = (patch: Partial<ClientConnection> = {}): ClientConnection => ({ id: 'connection', skillId: 'one', client: 'codex', path: 'D:/fake-home/.agents/skills/one', target: 'D:/Skills/one', health: 'ready', linked: true, ...patch });

test('connection candidates exclude only healthy same-source entries for the selected client', () => {
  assert.deepEqual(clientConnectChoices(skills, [connection()], 'codex').map(skill => skill.id), ['two']);
  for (const patch of [{ health: 'missing' }, { linked: false }, { target: 'D:/other/one' }, { client: 'cursor' }, { skillId: 'two' }]) assert(clientConnectChoices(skills, [connection(patch)], 'codex').some(skill => skill.id === 'one'));
  assert.deepEqual(clientConnectChoices(skills, [], 'codex', '工具一').map(skill => skill.id), ['one']);
  assert(sameReadyClientConnection(skills[0], connection({ target: '\\\\?\\D:\\SKILLS\\one\\' }), 'codex'));
  assert(!sameReadyClientConnection({ id: 'one', name: 'one', localPath: '/skills/One' }, connection({ target: '/skills/one' }), 'codex'));
});

test('filtered selection covers all matching pages and preserves hidden choices through search changes', () => {
  const selected = ['outside'];
  assert.deepEqual(selectFilteredClientItems(selected, ['first', 'second', 'first']), ['outside', 'first', 'second']);
  const ranged = clientSelectionRange(selected, ['first', 'second', 'third', 'fourth'], 'second', 'fourth', true, true);
  assert.deepEqual(ranged, ['outside', 'second', 'third', 'fourth']);
  assert.deepEqual(clientSelectionRange(ranged, ['first', 'second', 'third', 'fourth'], 'fourth', 'second', false, true), ['outside']);
  assert.deepEqual(clientSelectionRange(selected, ['first'], 'filtered-out-anchor', 'first', true, true), ['outside', 'first']);
  assert.deepEqual(selected, ['outside']);
});

test('client rows search current-client full paths and abnormal state without changing source records', () => {
  const rows = [connection(), connection({ id: 'missing', skillId: 'two', target: 'D:/Skills/two', health: 'missing' }), connection({ id: 'other-client', client: 'cursor' })];
  assert.deepEqual(clientConnectionRows(skills, rows, 'codex', '', true).map(item => item.id), ['missing']);
  assert.deepEqual(clientConnectionRows(skills, rows, 'codex', '工具一', false).map(item => item.id), ['connection']);
  assert.equal(rows.length, 3);
});

test('runtime drafts and current-content usage labels retain distinct meanings', () => {
  assert.deepEqual(clientConnectDraft({ query: 'one', selectedIds: ['one', 'one', 3, 'two'], anchorId: 'two' }), { query: 'one', selectedIds: ['one', 'two'], anchorId: 'two' });
  assert.deepEqual(clientConnectDraft(undefined), { query: '', selectedIds: [] });
  const confirmation = { client: 'codex', confirmedAt: '2026-10-25T08:00:00Z', contentFingerprint: 'current-content' };
  assert.equal(clientUsageText(connection({ verified: true })), '未记录使用', 'A legacy verified flag is not a current-content user confirmation');
  assert.equal(clientUsageText(connection({ usageConfirmation: confirmation, usageStatus: 'current' })), '已记录使用');
  assert.equal(clientUsageText(connection({ usageConfirmation: confirmation, usageStatus: 'stale' })), '使用记录已过期');
});

test('Ctrl+A only selects inside the owning list, preserving editing and other modifiers', () => {
  const key = { key: 'a', ctrlKey: true, metaKey: false, altKey: false };
  assert.equal(clientSelectionShortcut(key), 'all');
  assert.equal(clientSelectionShortcut(key, true), undefined);
  for (const patch of [{ key: ' ' }, { ctrlKey: false }, { metaKey: true }, { altKey: true }, { isComposing: true }, { defaultPrevented: true }]) assert.equal(clientSelectionShortcut({ ...key, ...patch }), undefined);
});

test('transferred batch results retain failed and hidden selections and reject another client context', () => {
  const draft = { query: 'only currently visible', selectedIds: ['success', 'failure', 'hidden'], anchorId: 'failure' };
  const batch = { taskId: 'actual-background-task', client: 'codex', results: [{ id: 'success', label: '成功项', success: true }, { id: 'failure', label: '失败项', success: false, error: '冲突原件已保留' }] };
  assert.equal(completedClientBatchDraft(draft, 'cursor', batch), undefined);
  const next = completedClientBatchDraft(draft, 'codex', batch)!;
  assert.deepEqual(next.selectedIds, ['failure', 'hidden']); assert.equal(next.query, draft.query);
  assert.deepEqual(clientConnectDraft(next).results, batch.results); assert.equal(clientConnectDraft(next).batchClient, 'codex');
  assert.deepEqual(draft.selectedIds, ['success', 'failure', 'hidden']);
});

// Render the actual component without a browser; the native acceptance suite measures geometry and clicks.
let clientPanelModule: Promise<typeof import('../src/ClientConnections.tsx')> | undefined;
function actualClientPanel() {
  clientPanelModule ??= readFile(new URL('../src/ClientConnections.tsx', import.meta.url), 'utf8').then(source => {
    let output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } }).outputText;
    output = output.replace(/import\s+['"]\.\/clients\.css['"];?/g, '');
    output = output.replace(/from\s+(['"])([^'"]+)\1/g, (_match, _quote, specifier: string) => {
      const url = specifier === './client-selection' ? new URL('../src/client-selection.ts', import.meta.url).href : import.meta.resolve(specifier);
      return `from ${JSON.stringify(url)}`;
    });
    return import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}`);
  });
  return clientPanelModule;
}

test('actual client names offer detail navigation only for an existing exact Skill identity', async () => {
  const { ClientPanel } = await actualClientPanel();
  const html = renderToStaticMarkup(createElement(ClientPanel, {
    skills: [...skills, { id: 'same-name', name: 'one', localPath: 'D:/another/one' }],
    connections: [connection(), connection({ id: 'removed', skillId: 'not-in-library', target: 'D:/another/one' })],
    client: 'codex', onClientChange: () => {}, onCheck: async () => {}, onOpenSkill: () => {},
  }));
  assert.equal((html.match(/class="client-row-name client-skill-link"/g) || []).length, 1);
  assert.match(html, /<button[^>]+title="查看 工具一 的 Skill 详情"[^>]*>工具一<\/button>/);
  assert.match(html, /<strong class="client-row-name">已移除的 Skill<\/strong>/);
  assert(!html.includes('调用未验证'));
});

test('actual connection disclosure keeps path and fixed-label toggle in a single native summary', async () => {
  const { ClientPanel } = await actualClientPanel();
  const html = renderToStaticMarkup(createElement(ClientPanel, {
    skills, connections: [connection()], client: 'codex', onClientChange: () => {}, onCheck: async () => {},
  }));
  const summary = html.match(/<details class="client-row-details"><summary[^>]*>(.*?)<\/summary>/s)?.[1];
  assert(summary, 'One native summary owns both the visible source and the disclosure control');
  assert.match(summary, /class="client-source"[^>]*>D:\/Skills\/one<\/code>/);
  assert.match(summary, /class="client-details-toggle">连接详情/);
  assert.equal((html.match(/class="client-source"/g) || []).length, 1);
  assert.match(html, /<\/summary><dl class="info-grid compact">/);
});

test('actual client header keeps switching and search together while entry information stays collapsed', async () => {
  const { ClientPanel } = await actualClientPanel();
  const html = renderToStaticMarkup(createElement(ClientPanel, {
    skills, connections: [connection()], client: 'codex', onClientChange: () => {}, onCheck: async () => {},
  }));
  const header = html.match(/<header class="client-panel-header"[^>]*data-pane-sticky[^>]*>(.*?)<\/header>/s)?.[1];
  assert(header, 'One sticky header owns client switching, search and connection actions');
  assert.match(header, /role="tablist" aria-label="客户端"/);
  assert.match(header, /aria-label="搜索客户端连接"/);
  assert.match(header, /检查连接/);
  assert.match(header, /连接 Skill/);
  assert.match(header, /<details class="client-entry-info"><summary[^>]*aria-label="查看 Codex 入口信息"/);
  assert.match(header, /<code>~\/\.agents\/skills<\/code>/);
  assert(!header.includes(' open=""'), 'Entry information does not expand into a permanent header row');
  assert(!html.includes('client-root-note'), 'The former standalone entry-description row is removed');
});
