export type ClientSkill = { id: string; name: string; displayName?: string; localPath?: string | null; readOnly?: boolean };
export type ClientUsageConfirmation = { client: string; confirmedAt: string; contentFingerprint: string };
export type ClientConnection = {
  id: string; skillId: string; client: string; path: string; target: string;
  origin?: string; kind?: string; health?: string; error?: string; linked?: boolean; verified?: boolean; checkedAt?: string;
  autoRepairable?: boolean; repairReason?: string;
  usageConfirmation?: ClientUsageConfirmation; usageStatus?: 'current' | 'stale' | 'unconfirmed' | 'unavailable';
};
export type ClientConnectDraft = { query: string; selectedIds: string[]; anchorId?: string; results?: ConnectionResult[]; batchClient?: string };
export type ClientManagementMode = 'disconnect' | 'stop';
export const CLIENT_MANAGEMENT_COPY = {
  disconnect: { label: '断开连接', description: '仅解除客户端目录链接，Skill 原文件保留在原目录。' },
  stop: { label: '忽略此连接，客户端继续可用', description: '只从管理器中忽略此连接，不移除客户端入口或 Skill 原文件。' },
} as const;
export type ConnectionResult = { id: string; label: string; success?: boolean; error?: string };
export type ClientBatchCompletion = { taskId: string; client: string; results: ConnectionResult[] };
export const CLIENT_OPTIONS = [{ id: 'codex', name: 'Codex', folder: '~/.agents/skills' }, { id: 'claude', name: 'Claude Code', folder: '~/.claude/skills' }, { id: 'cursor', name: 'Cursor', folder: '~/.cursor/skills' }];
export const clientSkillName = (skill?: ClientSkill) => skill?.displayName || skill?.name || '已移除的 Skill';

const pathIdentity = (value: string) => {
  const windows = /^(?:[a-z]:[\\/]|\\\\)/i.test(value);
  const normalized = value.replace(/^\\\\\?\\UNC\\/i, '//').replace(/^\\\\\?\\/, '').replaceAll('\\', '/').replace(/\/+$/, '');
  return windows ? normalized.toLocaleLowerCase() : normalized;
};
export function sameReadyClientConnection(skill: ClientSkill, connection: ClientConnection, client: string): boolean {
  return connection.client === client && connection.skillId === skill.id && connection.health === 'ready' && connection.linked !== false
    && !!skill.localPath && !!connection.target && pathIdentity(skill.localPath) === pathIdentity(connection.target);
}
export function clientConnectChoices(skills: readonly ClientSkill[], connections: readonly ClientConnection[], client: string, query = ''): ClientSkill[] {
  const search = query.trim().toLocaleLowerCase();
  return skills.filter(skill => skill.localPath && !skill.readOnly && !connections.some(connection => sameReadyClientConnection(skill, connection, client))
    && (!search || `${clientSkillName(skill)} ${skill.name} ${skill.localPath}`.toLocaleLowerCase().includes(search)));
}
export function clientConnectionRows(skills: readonly ClientSkill[], connections: readonly ClientConnection[], client: string, query: string, abnormal: boolean): ClientConnection[] {
  const search = query.trim().toLocaleLowerCase();
  return connections.filter(connection => connection.client === client && (!abnormal || connection.health !== 'ready')
    && (!search || `${clientSkillName(skills.find(skill => skill.id === connection.skillId))} ${connection.target} ${connection.path}`.toLocaleLowerCase().includes(search)));
}
export function clientSelectionRange(selected: readonly string[], filteredIds: readonly string[], anchorId: string | undefined, targetId: string, checked: boolean, shift: boolean): string[] {
  const start = anchorId ? filteredIds.indexOf(anchorId) : -1, end = filteredIds.indexOf(targetId);
  const affected = shift && start >= 0 && end >= 0 ? filteredIds.slice(Math.min(start, end), Math.max(start, end) + 1) : [targetId];
  const next = new Set(selected);
  for (const id of affected) { if (checked) next.add(id); else next.delete(id); }
  return [...next];
}
export function selectFilteredClientItems(selected: readonly string[], filteredIds: readonly string[]): string[] { return [...new Set([...selected, ...filteredIds])]; }
export function clientConnectDraft(value: unknown): ClientConnectDraft {
  const raw = value && typeof value === 'object' ? value as Record<string, unknown> : {};
  const results = Array.isArray(raw.results) ? raw.results.filter((item): item is ConnectionResult => !!item && typeof item === 'object' && typeof item.id === 'string' && typeof item.label === 'string').map(item => ({ id: item.id, label: item.label, ...(typeof item.success === 'boolean' ? { success: item.success } : {}), ...(typeof item.error === 'string' ? { error: item.error } : {}) })) : undefined;
  return { query: typeof raw.query === 'string' ? raw.query : '', selectedIds: Array.isArray(raw.selectedIds) ? [...new Set(raw.selectedIds.filter((id): id is string => typeof id === 'string' && !!id))] : [], ...(typeof raw.anchorId === 'string' && raw.anchorId ? { anchorId: raw.anchorId } : {}), ...(results ? { results } : {}), ...(typeof raw.batchClient === 'string' ? { batchClient: raw.batchClient } : {}) };
}
export function clientUsageText(connection: ClientConnection): string {
  if (!connection.usageConfirmation) return '未记录使用';
  if (connection.usageStatus === 'current') return '已记录使用';
  if (connection.usageStatus === 'stale') return '使用记录已过期';
  return '使用记录待核对';
}
export function completedClientBatchDraft(draft: ClientConnectDraft, client: string, batch: ClientBatchCompletion): ClientConnectDraft | undefined {
  if (batch.client !== client) return undefined;
  const successful = new Set(batch.results.filter(item => item.success && !item.error).map(item => item.id));
  return { ...draft, selectedIds: draft.selectedIds.filter(id => !successful.has(id)), results: batch.results, batchClient: client };
}
export function clientSelectionShortcut(event: { key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; isComposing?: boolean; defaultPrevented?: boolean }, editable = false): 'all' | undefined {
  return !editable && !event.defaultPrevented && !event.isComposing && event.ctrlKey && !event.metaKey && !event.altKey && event.key.toLowerCase() === 'a' ? 'all' : undefined;
}
