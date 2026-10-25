import { useMemo, useState } from 'react';
import { CircleAlert, LoaderCircle, Search } from 'lucide-react';
import { EXPORT_GROUPS, exportResourceState, exportSourceLabel, filterExportResources, optionalExportIds, resourcePathParts, type ExportPlan, type ExportResource } from './export-selection';
import { FileResourceRow } from './FileResourceRow';
import './external.css';

type Props = {
  plan: ExportPlan | null;
  busy?: boolean;
  error?: string;
  selectedIds?: string[];
  updating?: boolean;
  onToggle: (id: string, included: boolean) => void;
  onSet: (ids: string[]) => void;
  onRefresh?: () => void;
  view?: 'files' | 'export';
};
const size = (value?: number) => typeof value !== 'number' ? '大小待核验' : value < 1024 ? `${value} B` : value < 1048576 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1048576).toFixed(1)} MB`;

export function ExportResourceRow({ resource, busy = false, evidencePending = false, onToggle }: { resource: ExportResource; busy?: boolean; evidencePending?: boolean; onToggle: (id: string, included: boolean) => void }) {
  const path = resourcePathParts(resource.localPath || resource.path || resource.label);
  const state = exportResourceState(resource);
  return <FileResourceRow resource={resource} evidencePending={evidencePending} title={path.name} subtitle={path.directory} description={state.problem || resource.group === 'environment' ? resource.reason : undefined} source={exportSourceLabel(resource.source)} sizeText={resource.group === 'environment' && typeof resource.size !== 'number' ? undefined : resource.source === 'generated' && typeof resource.size !== 'number' ? '导出时生成' : size(resource.size)} filePath={resource.path} onToggle={onToggle} disabled={busy}>
    <details className="export-resource-details" data-fold-key={`resource:${resource.id}:source`}><summary>来源与 ZIP 路径</summary><dl><dt>可用来源</dt><dd>{exportSourceLabel(resource.source)}<code>{resource.localPath || resource.path}</code></dd><dt>ZIP 内路径</dt><dd><code>{resource.zipPath || '不加入 ZIP'}</code></dd>{resource.reason && <><dt>说明</dt><dd>{resource.reason}</dd></>}</dl></details>
  </FileResourceRow>;
}

export function ExportSelection({ plan, busy = false, error, selectedIds, updating = false, onToggle, onSet, onRefresh, view = 'export' }: Props) {
  const [query, setQuery] = useState('');
  const [includedOnly, setIncludedOnly] = useState(false);
  const resources = useMemo(() => (plan?.resources || []).map(resource => selectedIds && !['required', 'generated'].includes(resource.group) ? { ...resource, included: selectedIds.includes(resource.id) } : resource), [plan, selectedIds]);
  const visible = useMemo(() => filterExportResources(resources, query, includedOnly), [resources, query, includedOnly]);
  if (!plan) return <section className="export-selection" aria-label="导出文件清单"><p className="export-selection-loading" role={error ? 'alert' : 'status'}>{!error && <LoaderCircle size={16} className="spin" />}{error || '正在核对可导出的文件…'}</p>{onRefresh && error && <button className="button" onClick={onRefresh}>重新核对</button>}</section>;
  const summary = plan.summary;
  const groups = EXPORT_GROUPS.map(group => ({ ...group, rows: visible.filter(resource => resource.group === group.id && resource.source !== 'generated') }));
  const generated = visible.filter(resource => resource.source === 'generated');
  const optionalIds = optionalExportIds(plan.resources);
  return <section className={`export-selection export-selection-${view}`} aria-label="导出文件清单" aria-busy={busy || updating} data-export-plan={plan.id}>
    <div className="export-selection-summary" role="status">{updating ? <><strong>本次选择已更新</strong><span><LoaderCircle size={13} className="spin" />正在核对最终清单…</span></> : <><strong>本次 ZIP · {summary.total} 个文件</strong><span>必要 {summary.required} 项</span><span>可选包内：已选 {summary.optionalPackageSelected}／共 {summary.optionalPackageTotal} 项</span><span>可选外部：已选 {summary.optionalExternalSelected}／共 {summary.optionalExternalTotal} 项</span>{summary.generated > 0 && <span>自动生成 {summary.generated} 项</span>}<span>联网读取 {summary.remoteCount} 项 · 资源大小 {size(summary.bytes)}</span></>}</div>
    <div className="export-selection-tools"><label className="export-selection-search"><Search size={16} /><input aria-label="搜索导出文件" placeholder="搜索文件、来源路径或说明" value={query} onChange={event => setQuery(event.target.value)} /></label><label><input type="checkbox" checked={includedOnly} onChange={event => setIncludedOnly(event.target.checked)} />只看将导出文件</label><button className="button" disabled={busy} title="清空本次可选文件，必要文件继续自动包含" onClick={() => onSet([])}>仅必要文件</button><button className="button" disabled={busy || !optionalIds.length} title={optionalIds.length ? '选择全部可用的包内与外部可选文件' : '当前没有可用的可选文件'} onClick={() => onSet(optionalIds)}>全选可用可选文件</button>{onRefresh && <button className="button" disabled={busy} onClick={onRefresh}>重新核对</button>}</div>
    {plan.remoteStatus === 'error' && <p className="inline-warning" role="status">GitHub 文件清单读取失败，远端可选文件可能未完整列出。可继续选择可用文件，或重新核对。{plan.remoteError && <span>{plan.remoteError}</span>}</p>}
    {['pending', 'loading'].includes(plan.remoteStatus || '') && <p className="inline-warning" role="status">GitHub 文件清单尚未核对完成，远端可选文件可能未完整列出。</p>}
    {error && <p className="export-selection-error" role="alert"><CircleAlert size={16} />{error}</p>}
    {plan.issues.length > 0 && <div className="export-selection-error" role="alert"><strong>{plan.canExport ? '请留意以下说明' : '以下项目需要处理，暂不能完整导出'}</strong>{plan.issues.map((issue, index) => <p key={index}>{issue}</p>)}</div>}
    {groups.map(group => {
      const total = resources.filter(resource => resource.group === group.id && resource.source !== 'generated').length;
      if (group.id === 'environment' && !total) return null;
      const selected = group.rows.filter(resource => resource.included).length;
      return <details className="export-selection-group" key={group.id} data-fold-key={`export-group:${group.id}`} open={group.id === 'required' || group.rows.some(resource => exportResourceState(resource).problem) || undefined}>
        <summary data-outline-key={`export-${group.id}`} data-outline-label={group.label} data-outline-level="1" title={group.description}><strong>{group.label}</strong><span>{group.id === 'required' ? `${total} 项${error ? ' · 等待重新核对' : resources.some(resource => resource.group === 'required' && exportResourceState(resource).problem) ? ' · 有项目待处理' : ' · 自动包含'}` : group.id === 'environment' ? `${total} 项 · ${resources.some(resource => resource.group === 'environment' && resource.included) ? '已选项需处理或取消' : '不加入 ZIP'}` : updating ? `${total} 项 · 正在更新选择` : `已选 ${resources.filter(resource => resource.group === group.id && resource.included).length}／共 ${total} 项`}</span></summary>
        {group.rows.map(resource => <ExportResourceRow key={resource.id} resource={resource} busy={busy} evidencePending={!!error} onToggle={onToggle} />)}
        {group.id === 'required' && generated.length > 0 && <details className="export-selection-generated" data-fold-key="export-generated"><summary>程序生成的包内记录 · {summary.generated} 项</summary>{generated.map(resource => <ExportResourceRow key={resource.id} resource={resource} busy={busy} evidencePending={!!error} onToggle={onToggle} />)}</details>}
        {!group.rows.length && <p className="export-selection-empty">{total ? '当前筛选下没有文件。' : `没有${group.label}。`}</p>}
        {selected > 0 && query && <span className="export-selection-filter-note">当前显示 {group.rows.length} 项，其中 {selected} 项将导出。</span>}
      </details>;
    })}
    <details className="export-selection-help" data-fold-key="export-help"><summary>导出说明</summary><p className="export-selection-footnote">选择与同一 Skill 的文件页保持一致。普通外部文件仅进入所选 ZIP，原文件及长期用途保持；导出前会重新核对来源。</p></details>
  </section>;
}
