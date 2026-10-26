import { runtimeAttentionCount, type RuntimeIssue } from './runtime.ts';

export type LibraryMode = 'list' | 'details' | 'cards';
/** Read old/unknown preferences without rewriting the saved value. */
export function libraryMode(value: unknown): LibraryMode {
  return value === 'details' || value === 'cards' ? value : 'list';
}
export function cardNavigationIndex(index: number, count: number, columns: number, key: string): number | null {
  if (count <= 0 || index < 0 || index >= count) return null;
  const step = Math.max(1, Math.floor(columns) || 1);
  const next = key === 'ArrowLeft' ? index - 1 : key === 'ArrowRight' ? index + 1 : key === 'ArrowUp' ? index - step : key === 'ArrowDown' ? index + step : key === 'Home' ? 0 : key === 'End' ? count - 1 : null;
  return next === null ? null : Math.min(count - 1, Math.max(0, next));
}
export type LibrarySort = 'name' | 'status' | 'checked';
export type LibrarySourceFilter = 'all' | 'local' | 'github' | 'linked';
export type LibraryStatusGroup = 'local_only' | 'remote_only' | 'synced' | 'changed' | 'review' | 'problem';
export type LibraryStatusFilter = 'all' | 'attention' | LibraryStatusGroup | 'remote_updated' | 'local_modified' | 'both_modified' | 'docs_updated' | 'unknown_baseline' | 'local_extra' | 'remote_missing' | 'remote_check_required' | 'error';
export type LibraryQuery = { query?: string; sourceFilter?: LibrarySourceFilter; statusFilter?: LibraryStatusFilter; sort?: LibrarySort; categoryFilter?: string };
export interface LibraryItem {
  id: string; name: string; displayName?: string; description?: string; repo?: string | null;
  localPath?: string | null; remotePath?: string | null; sourceId?: string | null;
  status?: string; error?: string | null; remoteMissing?: boolean; remoteCheckRequired?: boolean;
  localNotice?: string | null; untrackedFiles?: readonly string[]; runtimeWarnings?: readonly string[]; runtimeIssues?: RuntimeIssue[];
  runtimeAudit?: { status: string; reason?: string | null };
  readOnly?: boolean; lastChecked?: string | null; categoryId?: string | null;
  favorite?: boolean; comparisonScope?: readonly string[]; runtimeConfirmed?: boolean;
  baselineCommit?: string | null; baselineKind?: string; installedCommit?: string | null; commit?: string | null; lastLocalCheckedAt?: string | null; lastRemoteCheckedAt?: string | null;
  changeDetails?: readonly {path: string; change: string; side?: string}[];
  comparisonNote?: string | null; associationNote?: string | null; associationPending?: boolean;
}
const attention = new Set(['error', 'remote_missing', 'remote_check_required', 'remote_updated', 'local_modified', 'both_modified', 'unknown_baseline', 'local_extra']);
const priority: Record<string, number> = { error: 0, remote_missing: 0, both_modified: 1, remote_updated: 2, local_modified: 3, docs_updated: 8, remote_check_required: 5, unknown_baseline: 5, local_extra: 6, remote_only: 7, synced: 8, local_only: 9 };
const groups: Record<LibraryStatusGroup, {label: string; tone: string}> = {
  local_only:{label:'仅本地',tone:'neutral'}, remote_only:{label:'可下载',tone:'blue'}, synced:{label:'内容一致',tone:'green'},
  changed:{label:'有变化',tone:'amber'}, review:{label:'待核对',tone:'amber'}, problem:{label:'检查异常',tone:'red'},
};
export const LIBRARY_STATUS_GROUPS = Object.entries(groups).map(([value, info]) => ({ value: value as LibraryStatusGroup, ...info }));
const isStatusGroup = (value: string): value is LibraryStatusGroup => Object.hasOwn(groups,value);
const statuses: Record<string, {group: LibraryStatusGroup; detail: string}> = {
  local_only:{group:'local_only',detail:'尚未关联 GitHub 来源'}, remote_only:{group:'remote_only',detail:'尚未下载到本地'},
  synced:{group:'synced',detail:'已核对范围内容一致'}, local_modified:{group:'changed',detail:'本地有修改'},
  remote_updated:{group:'changed',detail:'远端有更新'}, both_modified:{group:'changed',detail:'双方有变化'},
  docs_updated:{group:'synced',detail:'已核对范围内容一致'}, unknown_baseline:{group:'review',detail:'基准或范围待核对'},
  local_extra:{group:'review',detail:'范围外有本地文件'}, remote_missing:{group:'problem',detail:'远端对应目录缺失'},
  remote_check_required:{group:'review',detail:'当前远端尚未核对'}, error:{group:'problem',detail:'本次检查未完成'},
};
export function displayStatus(skill: LibraryItem): {key:string;group:LibraryStatusGroup;label:string;tone:string;detail:string} {
  const key = skill.remoteMissing ? 'remote_missing' : skill.error ? 'error' : skill.remoteCheckRequired ? 'remote_check_required' : skill.status || 'unknown';
  const info = Object.hasOwn(statuses,key) ? statuses[key] : {group:'review' as const,detail:'状态未知'};
  const detail = key === 'unknown_baseline' ? skill.baselineCommit ? '比较范围或差异待核对' : '比较基准尚未核实' : key === 'local_extra' && skill.untrackedFiles?.length ? `范围外有 ${skill.untrackedFiles.length} 个本地文件` : info.detail;
  return {key,group:info.group,...groups[info.group],detail};
}
export function statusFilterLabel(value: LibraryStatusFilter): string {
  return value === 'all' ? '全部状态' : value === 'attention' ? '需要关注' : isStatusGroup(value) ? groups[value].label : Object.hasOwn(statuses,value) ? statuses[value].detail : '状态未知';
}
export function comparisonScope(skill: LibraryItem): string[] { return [...(skill.comparisonScope || [])]; }
export function statusExplanation(skill: LibraryItem): string {
  const status = displayStatus(skill), scope = skill.comparisonScope;
  const time = (value?: string|null) => {const date = value ? new Date(value) : null;return date && !Number.isNaN(date.valueOf()) ? date.toLocaleString('zh-CN',{hour12:false}) : '未知';};
  const meaning: Record<string,string> = {
    synced:'最近比较时，已核对范围内的文件内容一致；不代表整个目录或GitHub此刻的最新内容完全相同。',
    local_extra:'存在比较范围外的本地文件，需核对是否纳入运行清单；这些文件尚不等于待上传修改。',
    docs_updated:'运行比较内容一致，远端说明或管理信息的更新已检查；无需重装运行文件来刷新说明。',
    error:`本次检查异常，不能把之前的一致结果当作当前结论。${skill.error || ''}`,
    remote_missing:'远端对应目录已缺失；本地文件仍保留，请核对来源。',
    remote_check_required:'需要重新核对远端，当前不能确认GitHub最新状态。',
    local_only:'仅登记本地目录，尚未关联可比较的GitHub来源。',
    remote_only:'已发现远端Skill，尚未下载到本地。',
    local_modified:'已核对范围内的本地运行内容有变化，上传仍需预览并手动确认。',
    remote_updated:'已核对范围内的远端运行内容有变化，下载仍需预览并手动确认。',
    both_modified:'本地与远端均有运行内容变化，需先核对差异。',
    unknown_baseline:skill.baselineCommit ? '已有比较基准，但当前范围或差异仍需核对；不据此判断哪侧较新。' : '比较基准尚未核实；仅凭当前内容差异不能判断哪侧较新。',
  };
  return [meaning[status.key] || '状态尚未核实。',
    scope?.length ? `比较范围：${scope.length} 个文件（${scope.join('、')}）。` : '比较文件范围尚未记录。',
    `运行清单：${skill.runtimeConfirmed ? '已确认' : '待确认，不能据此认定完整运行包一致'}。`,
    `比较基准：${skill.baselineCommit ? '已核实' : '未知'}；历史安装提交：${skill.installedCommit ? '已记录' : '未知'}。`,
    `最近远端检查：${time(skill.lastRemoteCheckedAt)}；最近本地检查：${time(skill.lastLocalCheckedAt)}。`,
    skill.untrackedFiles?.length ? `另有 ${skill.untrackedFiles.length} 个本地文件未纳入运行清单。` : '',
    skill.comparisonNote || '',
  ].filter(Boolean).join('\n');
}
export function selectionForDrag(id:string, checked: readonly string[], pageIds: readonly string[]): string[] {
  const selected = pageIds.filter(value => checked.includes(value));
  return selected.includes(id) ? selected : pageIds.includes(id) ? [id] : [];
}
export function rangeSelection(pageIds: readonly string[], checked: readonly string[], anchor:string|null, target:string, value:boolean):string[] {
  const end = pageIds.indexOf(target), start = anchor ? pageIds.indexOf(anchor) : -1;
  if (end < 0) return pageIds.filter(id => checked.includes(id));
  const range = pageIds.slice(Math.min(start < 0 ? end : start,end),Math.max(start < 0 ? end : start,end)+1);
  return pageIds.filter(id => range.includes(id) ? value : checked.includes(id));
}
const names = new Intl.Collator('zh-Hans', { numeric: true, sensitivity: 'base' });
const normalize = (value: string) => value.normalize('NFKC').toLocaleLowerCase();
export function libraryCounts(items: readonly LibraryItem[], sources: readonly { id: string; repo?: string }[]) {
  const repositories = new Set(sources.map(s => s.repo ? s.repo.toLocaleLowerCase() : s.id)).size;
  return { total: items.length, local: items.filter(s => s.localPath).length, github: items.filter(s => s.sourceId).length, linked: items.filter(s => s.localPath && s.sourceId).length, repositories };
}
export function githubFolderUrl(source: { repo?: string | null; branch?: string | null; commit?: string | null; remotePath?: string | null }): string | null {
  if (!source.repo || !/^[a-zA-Z0-9_.-]+\/[a-zA-Z0-9_.-]+$/.test(source.repo)) return null;
  const parts = (source.remotePath || '').split('/').filter(Boolean);
  if (parts.some(p => p === '.' || p === '..' || /[\\\x00-\x1f]/.test(p))) return null;
  const ref = source.branch || source.commit || 'HEAD';
  return `https://github.com/${source.repo.split('/').map(encodeURIComponent).join('/')}/tree/${encodeURIComponent(ref)}${parts.length ? '/' + parts.map(encodeURIComponent).join('/') : ''}`;
}
const runtimeAuditNeedsAttention = (skill: LibraryItem) => skill.runtimeAudit?.status === 'needs_review' || skill.runtimeAudit?.status === 'failed';
export function needsAttention(skill: LibraryItem): boolean {
  const runtimeAttention = skill.runtimeIssues === undefined ? skill.runtimeWarnings?.length : runtimeAttentionCount(skill.runtimeIssues);
  return Boolean(skill.associationPending || skill.localNotice || skill.untrackedFiles?.length || runtimeAttention || runtimeAuditNeedsAttention(skill) || attention.has(displayStatus(skill).key));
}
export function queryLibrary<T extends LibraryItem>(items: readonly T[], options: LibraryQuery = {}): T[] {
  const terms = normalize(options.query || '').trim().split(/\s+/).filter(Boolean);
  const source = options.sourceFilter || 'all', status = options.statusFilter || 'all';
  const result = items.filter(skill => {
    if (options.categoryFilter === 'favorites' ? !skill.favorite : options.categoryFilter === 'uncategorized' ? Boolean(skill.categoryId) : options.categoryFilter && options.categoryFilter !== 'all' && skill.categoryId !== options.categoryFilter) return false;
    if (source === 'local' && !skill.localPath || source === 'github' && !skill.sourceId || source === 'linked' && !(skill.localPath && skill.sourceId)) return false;
    if (status === 'attention' && !needsAttention(skill)) return false;
    if (status !== 'all' && status !== 'attention' && (isStatusGroup(status) ? displayStatus(skill).group !== status : displayStatus(skill).key !== status)) return false;
    const searchable = normalize([skill.name, skill.displayName, skill.description, skill.repo, skill.localPath, skill.remotePath, skill.readOnly ? '只读' : ''].filter(Boolean).join(' '));
    return terms.every(term => searchable.includes(term));
  });
  return result.sort((a, b) => {
    if (options.sort === 'status') {
      const rank = (s: T) => priority[displayStatus(s).key] ?? 10;
      const difference = rank(a) - rank(b); if (difference) return difference;
    }
    if (options.sort === 'checked') {
      const checked = (s: T) => { const time = Date.parse(s.lastChecked || ''); return Number.isFinite(time) ? time : 0; };
      const difference = checked(b) - checked(a); if (difference) return difference;
    }
    return names.compare(a.displayName || a.name, b.displayName || b.name) || names.compare(a.name, b.name) || a.id.localeCompare(b.id);
  });
}

/** Facets use the same category/source/search domain as the list, before its status filter. */
export function libraryStatusOptions(items: readonly LibraryItem[], options: LibraryQuery = {}): {value: LibraryStatusFilter; label: string; count: number}[] {
  const base = queryLibrary(items, {...options,statusFilter:'all'}), selected = options.statusFilter || 'all';
  const count = (value: LibraryStatusFilter) => value === 'all' ? base.length : value === 'attention' ? base.filter(needsAttention).length : base.filter(skill => isStatusGroup(value) ? displayStatus(skill).group === value : displayStatus(skill).key === value).length;
  const candidates: LibraryStatusFilter[] = ['all','attention',...LIBRARY_STATUS_GROUPS.map(group => group.value)];
  if (!candidates.includes(selected)) candidates.push(selected);
  return candidates.map(value => ({value,label:statusFilterLabel(value),count:count(value)})).filter(option => option.value === 'all' || option.count > 0 || option.value === selected);
}

export type ChangesGroup = 'changed' | 'review' | 'problem';
export type ChangesFilter = ChangesGroup | 'all';
export function changesGroup(skill: LibraryItem): ChangesGroup | null {
  if (!needsAttention(skill)) return null;
  const group = displayStatus(skill).group;
  return group === 'changed' || group === 'problem' ? group : 'review';
}
export function changesSummary(items: readonly LibraryItem[]) {
  const counts = {total:0,changed:0,review:0,problem:0};
  for (const item of items) { const group = changesGroup(item); if (group) { counts.total++; counts[group]++; } }
  return counts;
}
export function queryChanges<T extends LibraryItem>(items: readonly T[], options: {query?:string;group?:ChangesFilter} = {}): T[] {
  return queryLibrary(items,{query:options.query,sort:'status'}).filter(skill => { const group = changesGroup(skill); return !!group && (!options.group || options.group === 'all' || group === options.group); });
}
export function changeReason(skill: LibraryItem): string {
  const status = displayStatus(skill);
  if (status.key === 'error') return skill.error || '本次检查未完成，之前的比较结果不能作为当前结论。';
  if (status.key === 'remote_missing') return '远端对应目录已缺失；本地文件继续保留，请核对来源。';
  if (status.key === 'remote_check_required') return '尚未核对当前远端，不能把上次比较结果当作当前状态。';
  if (skill.associationPending) return skill.associationNote || '发现多个可能的来源，请在来源页选择对应项。';
  if (runtimeAuditNeedsAttention(skill)) return skill.runtimeAudit?.reason || (skill.runtimeAudit?.status === 'failed' ? '运行文件检查未完成，请在文件页查看原因并重新核对。' : '运行文件或必要引用尚未核对完整，请查看文件。');
  if (status.key === 'unknown_baseline') return skill.comparisonNote || (skill.baselineCommit ? '已有比较基准，但当前范围或差异仍需核对；不判断哪侧较新。' : '比较基准尚未核实，不能判断哪侧较新。');
  if (status.key === 'local_extra') return skill.localNotice || `${skill.untrackedFiles?.length ? `范围外有 ${skill.untrackedFiles.length} 个本地文件` : '有范围外本地文件'}，尚不等于待上传修改。`;
  if (status.key === 'docs_updated' && !needsAttention(skill)) return '运行比较内容一致，远端说明或管理信息的更新已检查，无需重装运行文件。';
  if (status.key === 'both_modified') return '本地与远端均有内容变化，请先比较差异，再决定处理方向。';
  if (status.key === 'local_modified') return '已核对范围内的本地内容有修改，提交前仍需预览并确认。';
  if (status.key === 'remote_updated') return '已核对范围内的远端内容有更新，应用前仍需预览并确认。';
  const runtime = skill.runtimeIssues === undefined ? skill.runtimeWarnings?.[0] : skill.runtimeIssues.find(issue => issue.severity !== 'info' && (issue.category === 'package_required' || issue.category === 'uncertain'))?.reason;
  return runtime || skill.localNotice || (skill.untrackedFiles?.length ? `另有 ${skill.untrackedFiles.length} 个本地文件未纳入文件范围，原文件继续保留。` : '存在需要核对的信息，请查看具体依据。');
}
export type ChangeActionKind = 'open' | 'compare' | 'docs' | 'files' | 'check';
export function changePrimaryAction(skill: LibraryItem): {kind:ChangeActionKind;label:string} {
  const status = displayStatus(skill);
  if (status.group === 'problem' || status.key === 'remote_check_required') return {kind:'check',label:'重新检查'};
  if (skill.associationPending) return {kind:'open',label:'选择来源'};
  if (runtimeAuditNeedsAttention(skill)) return {kind:'files',label:'查看文件'};
  if (status.key === 'docs_updated') return needsAttention(skill) ? {kind:'files',label:'查看文件'} : {kind:'docs',label:'查看说明'};
  if (status.key === 'local_extra') return {kind:'files',label:'查看文件'};
  if (status.key === 'unknown_baseline') return {kind:'open',label:'核对依据'};
  if (status.group === 'changed' && skill.sourceId && skill.localPath) return {kind:'compare',label:status.key === 'remote_updated' ? '预览更新' : '查看差异'};
  return skill.localPath ? {kind:'files',label:'查看文件'} : {kind:'open',label:'查看详情'};
}
export function paginateLibrary<T>(items: readonly T[], requestedPage: number, pageSize: number) {
  const size = Number.isFinite(pageSize) ? Math.max(1, Math.floor(pageSize)) : 20;
  const total = items.length, pageCount = Math.max(1, Math.ceil(total / size));
  const page = Math.min(pageCount, Math.max(1, Number.isFinite(requestedPage) ? Math.floor(requestedPage) : 1));
  const offset = (page - 1) * size;
  return { items: items.slice(offset, offset + size), total, page, pageCount, start: total ? offset + 1 : 0, end: Math.min(offset + size, total) };
}
