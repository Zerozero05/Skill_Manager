import { useEffect, useMemo, useState } from 'react';
import { ArrowLeftRight, ChevronLeft, ChevronRight, CircleAlert, CircleHelp, RefreshCw, Search } from 'lucide-react';
import { changePrimaryAction, changeReason, changesGroup, changesSummary, displayStatus, paginateLibrary, queryChanges, statusExplanation, statusFilterLabel, type ChangesFilter, type ChangesGroup, type LibraryItem } from './library';
import './updates-v025.css';

export type ChangesSourceError = { id: string; repo: string; error?: string | null; lastChecked?: string | null; branch?: string };
export type CheckSummary = { checkedAt?: string; newLocal?: number; newRemote?: number; associated?: number; docsUpdated?: number; needsAttention?: number; errors?: number; rootErrors?: string[] };
export type ChangesPanelProps<T extends LibraryItem> = {
  skills: readonly T[];
  sourceErrors?: readonly ChangesSourceError[];
  busy?: boolean;
  onOpen: (skill: T, section?: 'docs' | 'files' | 'source') => void;
  onCompare: (skill: T) => void | Promise<unknown>;
  onUpload: (skill: T) => void | Promise<unknown>;
  onDocs: (skill: T) => void;
  onFiles: (skill: T) => void;
  onCheck: (skill: T) => void | Promise<unknown>;
  onCheckSource?: (source: ChangesSourceError) => void | Promise<unknown>;
  checkSummary?: CheckSummary;
  onCheckMany?: (skillIds: string[]) => void | Promise<unknown>;
};
const groups: ChangesGroup[] = ['changed', 'review', 'problem'];
const icons = { changed: ArrowLeftRight, review: CircleHelp, problem: CircleAlert };
const checked = (value?: string | null) => { const date = value ? new Date(value) : null; return date && Number.isFinite(date.valueOf()) ? date.toLocaleString('zh-CN', {hour12:false}) : '尚无检查记录'; };

export function ChangesPanel<T extends LibraryItem>({ skills, sourceErrors = [], busy = false, onOpen, onCompare, onUpload, onDocs, onFiles, onCheck, onCheckSource, checkSummary, onCheckMany }: ChangesPanelProps<T>) {
  const [query, setQuery] = useState(''), [group, setGroup] = useState<ChangesFilter>('all'), [page, setPage] = useState(1), [pageSize, setPageSize] = useState(20);
  const [selection, setSelection] = useState<string[]>([]);
  const [batchMode, setBatchMode] = useState(false);
  const matching = useMemo(() => queryChanges(skills, {query}), [skills, query]);
  const counts = useMemo(() => changesSummary(matching), [matching]);
  const filtered = useMemo(() => group === 'all' ? matching : matching.filter(skill => changesGroup(skill) === group), [matching, group]);
  const result = paginateLibrary(filtered, page, pageSize);
  const sources = sourceErrors.filter(source => !!source.error);
  const selected = selection.filter(id => skills.some(skill => skill.id === id && changesGroup(skill) !== null));
  useEffect(() => { if (page !== result.page) setPage(result.page); }, [page, result.page]);
  useEffect(() => { setSelection(previous => { const pending = previous.filter(id => skills.some(skill => skill.id === id && changesGroup(skill) !== null)); return pending.length === previous.length ? previous : pending; }); }, [skills]);
  useEffect(() => { if (!filtered.length) setBatchMode(false); }, [filtered.length]);
  function selectGroup(value: ChangesFilter) { setGroup(value); setPage(1); }
  function exitSelection() { setSelection([]); setBatchMode(false); }
  function runPrimary(skill: T) {
    const action = changePrimaryAction(skill);
    if (action.kind === 'compare') void onCompare(skill);
    else if (action.kind === 'docs') onDocs(skill);
    else if (action.kind === 'files') onFiles(skill);
    else if (action.kind === 'check') void onCheck(skill);
    else onOpen(skill, skill.associationPending || displayStatus(skill).key === 'unknown_baseline' ? 'source' : undefined);
  }
  return <section className="changes-panel" aria-label="变化与待核对" onKeyDown={event => { if (event.key === 'Escape' && !busy && (batchMode || selection.length) && !(event.target as HTMLElement).matches('input:not([type="checkbox"]),textarea,select,[contenteditable="true"]')) { event.preventDefault(); event.stopPropagation(); exitSelection(); } }}>
    {checkSummary && <details className="check-recap" data-fold-key="recent-check"><summary><strong>最近检查</strong><span>{checked(checkSummary.checkedAt)}</span></summary><dl><div><dt>新登记</dt><dd>{(checkSummary.newLocal || 0) + (checkSummary.newRemote || 0)}</dd></div><div><dt>新关联</dt><dd>{checkSummary.associated || 0}</dd></div>{checkSummary.docsUpdated !== undefined && <div><dt>已处理说明</dt><dd>{checkSummary.docsUpdated}</dd></div>}<div><dt>需处理</dt><dd>{checkSummary.needsAttention || 0}</dd></div><div><dt>异常</dt><dd>{checkSummary.errors || 0}</dd></div></dl>{checkSummary.rootErrors?.map((error, index) => <p className="error-text" key={index}>{error}</p>)}</details>}
    <div className="changes-overview" aria-label="需要关注的 Skill 分类">{groups.map(value => { const Icon = icons[value]; return <button key={value} className={`changes-overview-item is-${value}`} type="button" aria-pressed={group === value} onClick={() => selectGroup(group === value ? 'all' : value)}><Icon size={18} aria-hidden="true" /><strong>{counts[value]}</strong><span>{statusFilterLabel(value)}</span></button>; })}</div>
    <div className="changes-results">
      <div className="changes-toolbar">
        <label className="changes-search"><Search size={17} aria-hidden="true" /><input aria-label="搜索变化中的 Skill" placeholder="搜索 Skill、来源或路径" value={query} onChange={event => { setQuery(event.target.value); setPage(1); }} /></label>
        <button className={`button ${group === 'all' ? 'primary' : 'secondary'}`} type="button" aria-pressed={group === 'all'} onClick={() => selectGroup('all')}>全部需要关注 <span>{counts.total}</span></button>
        {(query || group !== 'all') && <button className="text-button" type="button" onClick={() => { setQuery(''); selectGroup('all'); }}>清除筛选</button>}
      </div>
      <div className="changes-list-heading"><div className="changes-list-title"><strong>{group === 'all' ? '需要关注的 Skill' : statusFilterLabel(group)}</strong><span role="status">{result.total} 个 Skill{query && ' · 当前搜索结果'}</span></div>{!batchMode && onCheckMany && filtered.length > 1 && <button className="button secondary changes-list-action" type="button" aria-label="批量选择 Skill" disabled={busy} onClick={() => setBatchMode(true)}>批量选择</button>}{!batchMode && filtered.length === 1 && changePrimaryAction(filtered[0]).kind !== 'check' && <button className="button secondary changes-list-action" type="button" disabled={busy} onClick={() => void onCheck(filtered[0])}><RefreshCw size={14} />重新检查</button>}</div>
      {onCheckMany && batchMode && !!filtered.length && <div className="changes-selection" role="toolbar" aria-label="变化批量选择"><div className="changes-selection-controls"><button type="button" className="text-button" disabled={busy} onClick={() => setSelection(filtered.map(skill => skill.id))}>选择当前结果 ({filtered.length})</button><button type="button" className="text-button" disabled={busy || !selected.length} onClick={() => setSelection([])}>清空</button></div><span className="changes-selection-count" role="status">已选 {selected.length} 项</span><div className="changes-selection-actions"><button type="button" className="button" aria-label="检查所选 Skill" disabled={busy || !selected.length} onClick={() => void onCheckMany(selected)}><RefreshCw size={14} />检查所选</button><button type="button" className="text-button" disabled={busy} onClick={exitSelection}>退出选择</button></div></div>}
      <div className="changes-rows" tabIndex={onCheckMany ? 0 : undefined} aria-label="待处理 Skill 列表" onKeyDown={event => { if ((event.target as HTMLElement).matches('input,textarea,select,[contenteditable="true"]') || busy) return; if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a' && onCheckMany && filtered.length) { event.preventDefault(); event.stopPropagation(); setBatchMode(true); setSelection(filtered.map(skill => skill.id)); } if (event.key === 'Escape' && (batchMode || selection.length)) { event.preventDefault(); event.stopPropagation(); exitSelection(); } }}>{result.items.map(skill => {
        const status = displayStatus(skill), action = changePrimaryAction(skill), reason = changeReason(skill);
        const blocked = busy || action.kind === 'compare' && !!skill.readOnly;
        const changedPaths = [...new Set((skill.changeDetails || []).filter(file => file.change !== 'unchanged').map(file => file.path))];
        return <article className="changes-item" key={skill.id} data-change-skill={skill.id} data-change-group={changesGroup(skill)}>
          <div className="changes-item-heading">{onCheckMany && batchMode && <input type="checkbox" aria-label={`选择变化 ${skill.displayName || skill.name}`} disabled={busy} checked={selected.includes(skill.id)} onChange={event => setSelection(previous => event.target.checked ? [...previous, skill.id] : previous.filter(id => id !== skill.id))} />}<button className="changes-skill-name" type="button" onClick={() => onOpen(skill)}>{skill.displayName || skill.name}</button><span className={`badge ${status.tone}`} title={statusExplanation(skill)} data-status={status.key} data-status-group={status.group}><span className="status-dot" />{status.label}</span></div>
          {(status.detail !== status.label || reason !== status.detail) && <p className="changes-reason">{status.detail !== status.label && <strong>{status.detail}</strong>}{reason !== status.detail && <span>{reason}</span>}</p>}
          <div className="changes-item-meta"><span title={skill.repo ? `${skill.repo}/${skill.remotePath || ''}` : skill.localPath || undefined}>{skill.repo || (skill.localPath ? '本地目录' : '来源待核对')}</span>{changedPaths.length > 0 && <span title={changedPaths.join('\n')}>{changedPaths.length} 个差异文件</span>}<span>{skill.comparisonScope?.length ? `比较范围 ${skill.comparisonScope.length} 个文件` : '比较范围待核对'}</span></div>
          <div className="changes-item-footer">{skill.lastChecked && <span>最近记录：{checked(skill.lastChecked)}</span>}<div className="changes-actions"><button className="button secondary" type="button" disabled={busy} onClick={() => onOpen(skill)}>查看详情</button><button className="button primary" type="button" disabled={blocked} title={skill.readOnly && action.kind === 'compare' ? '应用托管 Skill 保持只读，可查看详情与来源' : undefined} onClick={() => runPrimary(skill)}>{action.kind === 'check' && <RefreshCw size={14} />}{action.label}</button>{status.key === 'local_modified' && skill.sourceId && skill.localPath && <button className="button secondary" type="button" disabled={busy || skill.readOnly} title={skill.readOnly ? '应用托管 Skill 不能提交修改' : '先选择并预览，再确认提交'} onClick={() => void onUpload(skill)}>预览提交</button>}</div></div>
          <details className="changes-evidence" data-fold-key={`change:${skill.id}:evidence`}><summary>比较依据与说明</summary><p>{statusExplanation(skill)}</p>{skill.associationNote && <p>{skill.associationNote}</p>}</details>
        </article>;
      })}</div>
      {!result.items.length && <div className="changes-empty"><CircleHelp size={24} aria-hidden="true" /><strong>{query || group !== 'all' ? '当前筛选没有匹配的 Skill' : sources.length ? '暂无待处理的 Skill，来源检查仍未完成' : '当前没有需要处理的 Skill'}</strong>{(query || group !== 'all') && <p>清除筛选可查看其他需要关注的条目。</p>}</div>}
      <div className="changes-pagination"><span>{result.total ? `${result.start}–${result.end} / ${result.total}` : '0 个 Skill'}</span><label>每页 <select aria-label="变化页每页数量" value={pageSize} onChange={event => { setPageSize(Number(event.target.value)); setPage(1); }}>{[20,50,100].map(value => <option key={value} value={value}>{value} 项</option>)}</select></label><div><button className="icon-button" type="button" aria-label="上一页变化" disabled={result.page <= 1} onClick={() => setPage(result.page - 1)}><ChevronLeft size={17} /></button><span>第 {result.page} / {result.pageCount} 页</span><button className="icon-button" type="button" aria-label="下一页变化" disabled={result.page >= result.pageCount} onClick={() => setPage(result.page + 1)}><ChevronRight size={17} /></button></div></div>
    </div>
    {sources.length > 0 && <section className="changes-source-errors" aria-label="来源检查未完成"><div className="changes-source-heading"><CircleAlert size={17} /><strong>来源检查未完成</strong><span>{sources.length} 个来源 · 单独统计</span></div>{sources.map(source => <div className="changes-source-item" key={source.id} data-change-source={source.id}><div><strong>{source.repo}{source.branch && <small> · {source.branch}</small>}</strong><p>{source.error}</p><span>{checked(source.lastChecked)}</span></div>{onCheckSource && <button className="button secondary" type="button" disabled={busy} onClick={() => void onCheckSource(source)}><RefreshCw size={14} />重试检查</button>}</div>)}{!onCheckSource && <p className="changes-source-note">可使用页面上方的“检查更新”重新核对来源。</p>}</section>}
  </section>;
}
