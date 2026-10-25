import { useEffect, useMemo, useRef, useState, type ReactNode, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { CircleAlert, FolderOpen, RefreshCw, Search, LocateFixed, ChevronRight } from 'lucide-react';
import './external.css';
import { buildExternalMappings, type MappingDraft } from './external-mapping';
import { groupExternalReferences, referencePresentationIssues, type InventoryFile, type InventoryIdentity } from './external';
import { clusterExportReferences, exportInclusionLabel, resourceForReference, type ExportResource } from './export-selection';
import { exportResourceState, exportSourceLabel, filterExportResources, resourcePathParts } from './export-selection';
import { FileResourceRow } from './FileResourceRow';
import { ExportResourceRow } from './ExportSelection';
import { runtimeIssuesForReference } from './runtime-presentation';
import { runtimeAttentionCount, type RuntimeIssue } from './runtime';

export type ExternalRole = 'required' | 'optional' | 'environment' | 'history';
export type ExternalSourceEvidence = { status: string; path?: string | null; repositoryPath?: string; repo?: string; commit?: string; hash?: string; checkedAt?: string; readable?: boolean; error?: string; matchKind?: string; candidateCount?: number; candidatesTruncated?: boolean };
export type ExternalCandidate = { source: 'github'; repo: string; repositoryPath: string; commit: string; gitBlobSha: string; size: number; exact: boolean };
export type ExternalDependency = {
  id: string; source: string; target: string; sourceHash: string; referenceFingerprint: string; role: ExternalRole | 'alias';
  status: 'available' | 'missing' | 'inaccessible' | 'needs_selection' | 'needs_review' | 'unsupported' | 'environment' | 'reference';
  reason: string; canonicalPath?: string | null; kind?: string | null; hash?: string | null;
  packagePath?: string | null; required: boolean; defaultIncluded: boolean; portable: boolean;
  exists: boolean; readable: boolean; checkedAt: string; bindingStale: boolean;
  referenceKind?: 'file' | 'directory' | 'package_file' | 'skill_alias' | 'command'; local?: ExternalSourceEvidence; remote?: ExternalSourceEvidence;
  effectiveSource?: 'local' | 'github' | null; candidates?: ExternalCandidate[];
};
export type ExternalBindingOptions = { source?: 'local' | 'github'; repositoryPath?: string; remoteCommit?: string; expectedFingerprint?: string };
export type ExternalBatchRequest = {
  operation: 'role' | 'bind' | 'remap' | 'recheck' | 'auto_match';
  items: { id: string; expectedFingerprint: string; role?: ExternalRole; path?: string; source?: 'local' | 'github'; repositoryPath?: string; remoteCommit?: string }[];
};
export type ExternalBatchResult = { results: { id?: string; status?: string; error?: string; reason?: string }[]; autoMatchSummary?: { matched: number; needsSelection: number; missing: number; error: number } };
export type ExternalMappingDraft = MappingDraft;
export type ExternalManagementDraft = { query: string; filter: string; managed: string[]; batchRole: ExternalRole; mappings: ExternalMappingDraft[] | null; managementMode?: boolean };
export type ExternalSections = {
  referencesFor: (path: string) => ReactNode;
  requiredByReference: (path: string) => boolean;
  issueCount: (path: string) => number;
  toolbar: ReactNode;
  requiredExternal: ReactNode;
  optionalExternal: ReactNode;
  environment: ReactNode;
  counts: { requiredExternal: number; optionalExternal: number; environment: number; attachedFiles: number; references: number };
};
type Props = {
  items: ExternalDependency[]; selectedIds: string[]; onSelectionChange: (ids: string[]) => void;
  onCheck: () => void; onBind: (id: string, role: ExternalRole, path?: string, options?: ExternalBindingOptions) => void | Promise<void>;
  onBatch?: (request: ExternalBatchRequest) => void | ExternalBatchResult | Promise<void | ExternalBatchResult>; onLocate?: (item: ExternalDependency) => void;
  initialDraft?: ExternalManagementDraft; onDraftChange?: (draft: ExternalManagementDraft) => void;
  inventory?: InventoryFile[]; identity?: InventoryIdentity;
  children?: (presentation: ExternalSections) => ReactNode;
  layout?: 'legacy' | 'sections';
  runtimeIssues?: RuntimeIssue[];
  onRevealFile?: (path: string, referenceId?: string) => void;
  exportResources?: ExportResource[];
  onExportToggle?: (resourceId: string, included: boolean) => void;
  evidencePending?: boolean;
  busy?: boolean; readOnly?: boolean;
};
const labels: Record<ExternalDependency['status'], string> = { available: '可用', missing: '路径缺失', inaccessible: '无法访问', needs_selection: '请选择来源', needs_review: '引用已变化', unsupported: '不能随包', environment: '环境要求', reference: '参考信息' };
const sourceLabels: Record<string, string> = { ...labels, missing: '未找到', needs_selection: '需映射', not_checked: '未检查', not_linked: '未关联', not_installed: '未安装', not_applicable: '不适用', error: '读取失败' };
const roles: { value: ExternalRole; label: string }[] = [{ value: 'required', label: '必要运行文件' }, { value: 'optional', label: '可选附加文件' }, { value: 'environment', label: '环境与共享规则' }, { value: 'history', label: '维护或历史信息' }];
function actualRole(item: ExternalDependency): ExternalRole { return item.role === 'alias' ? 'environment' : item.role; }
function isProblem(item: ExternalDependency) { return ['missing', 'inaccessible', 'needs_selection', 'needs_review', 'unsupported'].includes(item.status); }
type Mapping = ExternalMappingDraft;

export function ExternalDependencies({ items, selectedIds, onSelectionChange, onCheck, onBind, onBatch, onLocate, initialDraft, onDraftChange, inventory = [], identity = {}, children, layout = 'legacy', runtimeIssues = [], onRevealFile, exportResources, onExportToggle, evidencePending = false, busy = false, readOnly = false }: Props) {
  const [error, setError] = useState(''), [query, setQuery] = useState(() => initialDraft?.query || ''), [filter, setFilter] = useState(() => initialDraft?.filter || 'all');
  const [managed, setManaged] = useState<string[]>(() => initialDraft?.managed || []), [batchRole, setBatchRole] = useState<ExternalRole>(() => initialDraft?.batchRole || 'optional');
  const [mappings, setMappings] = useState<Mapping[] | null>(() => initialDraft?.mappings || null);
  const [managementMode, setManagementMode] = useState(initialDraft?.managementMode || false);
  const [mappingFocus, setMappingFocus] = useState<string>();
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({});
  const presentation = useMemo(() => groupExternalReferences(items, inventory, identity), [items, inventory, identity]);
  const draftCallback = useRef(onDraftChange); draftCallback.current = onDraftChange;
  useEffect(() => { draftCallback.current?.({ query, filter, managed, batchRole, mappings, managementMode }); }, [query, filter, managed, batchRole, mappings, managementMode]);
  const anchor = useRef<string | null>(null), rowRefs = useRef(new Map<string, HTMLDivElement>());
  function issueCountFor(item: ExternalDependency) { return referencePresentationIssues(item, inventory.find(file => file.path === presentation.fileByReference.get(item.id)), identity).length + Number(!!rowErrors[item.id]) + Number(runtimeAttentionCount(runtimeIssuesForReference(runtimeIssues, item)) > 0); }
  const filtered = useMemo(() => items.filter(item => `${item.target} ${item.source} ${item.canonicalPath || ''} ${item.remote?.repositoryPath || ''}`.toLowerCase().includes(query.toLowerCase()) && (filter === 'all' || (filter === 'problems' ? issueCountFor(item) > 0 : filter === 'github' ? item.remote?.status === 'available' : item.required))), [items, query, filter, rowErrors, presentation, inventory, identity, runtimeIssues]);
  const chosen = items.filter(item => managed.includes(item.id));
  useEffect(() => {
    if (!mappingFocus) return;
    const frame = requestAnimationFrame(() => {
      const row = rowRefs.current.get(mappingFocus); if (!row || row.closest('[hidden]')) { setMappingFocus(undefined); return; }
      for (let parent = row.parentElement; parent; parent = parent.parentElement) if (parent instanceof HTMLDetailsElement) parent.open = true;
      row.querySelector<HTMLElement>('.external-mapping input')?.focus({ preventScroll: true });
      row.querySelector<HTMLElement>('.external-mapping')?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
      setMappingFocus(undefined);
    });
    return () => cancelAnimationFrame(frame);
  }, [mappingFocus, mappings]);
  const blocked = items.filter(item => item.required && item.status !== 'available').length;
  const standalone = filtered.filter(item => !presentation.fileByReference.has(item.id));
  const resourceOf = (item: ExternalDependency) => resourceForReference(exportResources || [], item.id);
  const groupOf = (item: ExternalDependency) => {
    const resource = resourceOf(item);
    if (resource) return resource.group === 'optional_package' ? 'optional_external' : resource.group;
    return item.required ? 'required' : actualRole(item) === 'optional' ? 'optional_external' : actualRole(item) === 'history' ? 'history' : 'environment';
  };
  const groups = [
    { id: 'required', name: '必要外部文件', items: standalone.filter(item => groupOf(item) === 'required'), open: true },
    { id: 'optional_external', name: '可选外部文件', items: standalone.filter(item => groupOf(item) === 'optional_external'), open: false },
    { id: 'environment', name: '环境与参考', items: standalone.filter(item => ['environment', 'history'].includes(groupOf(item))), open: false },
  ].filter(group => group.items.length);
  async function safely(action: () => void | Promise<void>, id?: string) { try { setError(''); if (id) setRowErrors(previous => ({ ...previous, [id]: '' })); await action(); } catch (cause) { const message = cause instanceof Error ? cause.message : String(cause); setError(message); if (id) setRowErrors(previous => ({ ...previous, [id]: message })); } }
  async function chooseFile(item: ExternalDependency) { await safely(async () => { const path = await open({ title: '选择此引用实际使用的文件（原件保持原位置）', multiple: false, directory: false }); if (typeof path === 'string') await onBind(item.id, actualRole(item), path, { source: 'local', expectedFingerprint: item.referenceFingerprint }); }, item.id); }
  function toggleManaged(item: ExternalDependency, checked: boolean, range: boolean) {
    let ids = [item.id];
    if (range && anchor.current) { const start = filtered.findIndex(row => row.id === anchor.current), end = filtered.findIndex(row => row.id === item.id); if (start >= 0 && end >= 0) ids = filtered.slice(Math.min(start, end), Math.max(start, end) + 1).map(row => row.id); }
    setManaged(previous => checked ? [...new Set([...previous, ...ids])] : previous.filter(id => !ids.includes(id))); anchor.current = item.id;
  }
  function revealReference(id: string) {
    const path = presentation.fileByReference.get(id), node = rowRefs.current.get(id);
    for (let parent = node?.parentElement; parent; parent = parent.parentElement) if (parent instanceof HTMLDetailsElement) parent.open = true;
    if (path && onRevealFile) { onRevealFile(path, id); requestAnimationFrame(() => rowRefs.current.get(id)?.focus({ preventScroll: true })); return; }
    requestAnimationFrame(() => { node?.scrollIntoView({ behavior: 'instant', block: 'nearest' }); node?.focus({ preventScroll: true }); });
  }
  function showResults(result: void | ExternalBatchResult) {
    if (!result) return;
    const failed = result.results.filter(item => item.error || ['error', 'missing', 'needs_selection', 'needs_review'].includes(item.status || ''));
    setRowErrors(previous => { const next = { ...previous }; result.results.forEach(item => { if (item.id) next[item.id] = failed.includes(item) ? item.reason || item.error || '此项需要处理' : ''; }); return next; });
    if (failed.length) { setError(`${failed.length} 项仍需处理；结果显示在对应文件的引用详情中。`); if (failed[0].id) { const id = failed[0].id; if (!filtered.some(item => item.id === id)) { setQuery(''); setFilter('all'); requestAnimationFrame(() => revealReference(id)); } else revealReference(id); } }
  }
  async function batch(operation: ExternalBatchRequest['operation']) { if (onBatch && chosen.length) await safely(async () => { showResults(await onBatch({ operation, items: chosen.map(item => ({ id: item.id, expectedFingerprint: item.referenceFingerprint, ...(operation === 'role' ? { role: batchRole } : {}) })) })); }); }
  function beginMappings(rows: ExternalDependency[], source?: 'github') { const values = buildExternalMappings(rows, source); if (!values.length) { setError('这些引用无需绑定文件。'); return; } setError(''); if (!filtered.some(item => item.id === values[0].id)) { setQuery(''); setFilter('all'); } setMappings(values); revealReference(values[0].id); setMappingFocus(values[0].id); }
  function updateMapping(id: string, change: Partial<Mapping>) { setMappings(previous => previous?.map(row => row.id === id ? { ...row, ...change } : row) || null); }
  async function saveMappings() { if (!mappings?.length) return; await safely(async () => {
    const changes = mappings.map(mapping => { const item = items.find(item => item.id === mapping.id); if (!item) throw new Error('引用列表已变化，请重新检查。'); if (mapping.source === 'local' ? !mapping.path.trim() : !mapping.repositoryPath.trim() || !mapping.remoteCommit) throw new Error('请为每项选择完整文件路径或固定提交的 GitHub 文件。'); return { id: item.id, expectedFingerprint: item.referenceFingerprint, role: actualRole(item), source: mapping.source, ...(mapping.source === 'local' ? { path: mapping.path.trim() } : { repositoryPath: mapping.repositoryPath.trim(), remoteCommit: mapping.remoteCommit }) }; });
    if (onBatch) { const result = await onBatch({ operation: 'bind', items: changes }); const failed = result?.results.filter(item => item.status === 'error' || item.error) || []; showResults(result); if (failed.length) { setMappings(previous => previous?.filter(mapping => failed.some(item => item.id === mapping.id)) || null); return; } } else if (changes.length === 1) { const change = changes[0]; await onBind(change.id, change.role, 'path' in change ? change.path : undefined, change); } else throw new Error('批量绑定入口尚未连接。'); setMappings(null);
  }); }
  function mappingEditor(item: ExternalDependency) {
    const mapping = mappings?.find(row => row.id === item.id); if (!mapping) return null;
    return <section className="external-mapping" aria-label={'文件映射 ' + item.target}>
      <strong>确认此文件的来源</strong><p>只保存映射，用途不变。可更换已匹配路径；同名候选须核对完整位置。</p>
      <div className="external-mapping-row"><select aria-label={'绑定来源 ' + item.target} value={mapping.source} onChange={event => updateMapping(item.id, { source: event.target.value as Mapping['source'] })}><option value="local">本地原文件</option><option value="github" disabled={!item.remote?.commit}>关联 GitHub</option></select>
      {mapping.source === 'local' ? <><input aria-label={'绑定本地路径 ' + item.target} placeholder="完整文件路径" value={mapping.path} onChange={event => updateMapping(item.id, { path: event.target.value })} /><button className="button" type="button" onClick={() => void safely(async () => { const path = await open({ directory: false, multiple: false, title: '选择实际原文件' }); if (typeof path === 'string') updateMapping(item.id, { path }); })}>选择文件</button></> : <><input aria-label={'绑定仓库路径 ' + item.target} list={'external-candidates-' + item.id} placeholder="完整仓库文件路径" value={mapping.repositoryPath} onChange={event => updateMapping(item.id, { repositoryPath: event.target.value })} /><datalist id={'external-candidates-' + item.id}>{item.candidates?.map(candidate => <option value={candidate.repositoryPath} key={candidate.repositoryPath}>{candidate.exact ? '已核验对应路径' : '候选，需确认'}</option>)}</datalist><small>提交 {mapping.remoteCommit.slice(0, 12) || '尚未读取'}</small></>}
      </div>{mapping.source === 'github' && item.remote?.candidatesTruncated && <p className="external-note">共 {item.remote.candidateCount} 项候选，此处列出前 40 项；可输入完整仓库路径。自动匹配核对完整清单。</p>}{error && <p className="error-text" role="alert">{error}</p>}<div className="external-mapping-actions"><button className="button" type="button" onClick={() => setMappings(null)}>关闭预览</button><button className="button" type="button" disabled={busy || readOnly || !mappings?.length} onClick={() => void saveMappings()}>确认这些映射</button></div>
    </section>;
  }
  function exportChoice(item: ExternalDependency, attached: boolean) {
    const resource = resourceOf(item);
    if (attached) return null;
    if (resource) {
      if (resource.group === 'required') return <span className="external-file-inclusion">{exportInclusionLabel(resource)}</span>;
      if (!resource.included && (resource.group === 'environment' || !resource.selectable)) return <span className="external-file-inclusion is-excluded" title={resource.reason}>不加入 ZIP</span>;
      return <label className="external-export-choice"><input type="checkbox" aria-label={`加入 ZIP ${resource.label}`} checked={resource.included} disabled={busy || !onExportToggle} title={onExportToggle ? resource.selectable ? '仅影响本次 ZIP，不改变长期用途' : '此项已不可用，可以取消本次选择' : '正在连接导出选择'} onChange={event => onExportToggle?.(resource.id, event.target.checked)} />加入 ZIP<span>{exportInclusionLabel(resource)}</span></label>;
    }
    if (item.required) return <span className="external-file-inclusion">导出范围待核对</span>;
    if (exportResources === undefined && actualRole(item) === 'optional') return <label className="external-export-choice"><input type="checkbox" aria-label={`加入 ZIP ${item.target}`} checked={selectedIds.includes(item.id)} disabled={busy || item.status !== 'available' || !item.packagePath} title={item.status !== 'available' || !item.packagePath ? item.reason : '仅影响本次 ZIP，不改变长期用途'} onChange={event => onSelectionChange(event.target.checked ? [...new Set([...selectedIds, item.id])] : selectedIds.filter(id => id !== item.id))} />加入 ZIP</label>;
    return <span className="external-file-inclusion is-excluded">{exportResources ? '导出范围待核对' : '不加入 ZIP'}</span>;
  }
  function renderItem(item: ExternalDependency, attached = false, resourceOnly = false) {
    const resource = resourceOf(item);
    const issues = issueCountFor(item);
    const scanIssues = runtimeIssuesForReference(runtimeIssues, item);
    const openManagement = managementMode || issues > 0 || !!rowErrors[item.id] || !!mappings?.some(mapping => mapping.id === item.id);
    return <div className={`external-item ${managementMode && !resourceOnly && managed.includes(item.id) ? 'is-selected' : ''}`} key={item.id} data-external-reference={resourceOnly ? undefined : item.id} tabIndex={-1} ref={node => { if (resourceOnly) return; if (node) rowRefs.current.set(item.id, node); else rowRefs.current.delete(item.id); }}>
      <div className="external-item-title">{managementMode && !resourceOnly && <label className="external-manage-choice"><input type="checkbox" aria-label={`管理选择 ${item.target}`} checked={managed.includes(item.id)} onChange={event => toggleManaged(item, event.target.checked, (event.nativeEvent as MouseEvent).shiftKey)} /><span>管理选择</span></label>}<strong className={item.required && item.status !== 'available' ? 'error-text' : ''}>{attached ? `引用：${item.source}` : item.referenceKind === 'command' ? '客户端命令' : item.referenceKind === 'package_file' ? '包内文件' : item.referenceKind === 'skill_alias' ? '当前 Skill 入口' : resource?.label || labels[item.status] || item.status}</strong>{exportChoice(item, attached)}</div>
      {!attached && <code>{item.canonicalPath || item.target}</code>}{!attached && item.canonicalPath && item.canonicalPath.replaceAll('\\', '/').toLowerCase() !== item.target.replaceAll('\\', '/').toLowerCase() && <small>原引用：{item.target}</small>}
      {(!attached || issues > 0 || managementMode) && <><div className="external-sources"><span title={item.local?.error}>本地：{sourceLabels[item.local?.status || item.status] || item.local?.status}</span><span title={item.remote?.error}>GitHub：{sourceLabels[item.remote?.status || 'not_checked'] || item.remote?.status}</span>{item.effectiveSource && <b>{resource?.included ? '本次采用' : '可用来源'}：{item.effectiveSource === 'github' ? 'GitHub' : '本地'}</b>}</div><p>{resource?.reason || item.reason}</p></>}
      {!resourceOnly && <details className="external-management" data-fold-key={`reference:${item.id}:management`} open={openManagement || undefined}><summary>{attached ? '管理此处引用' : '管理来源与用途'}{issues > 0 && <span>{issues} 项需要处理</span>}</summary>
        <div className="external-item-footer"><small>引用位置：{item.source}</small><label>长期用途<select aria-label={`外部依赖用途 ${item.target}`} value={actualRole(item)} disabled={busy || readOnly || item.referenceKind === 'command' || item.referenceKind === 'skill_alias'} title={readOnly ? '托管 Skill 只读，不能修改长期用途' : item.referenceKind === 'command' || item.referenceKind === 'skill_alias' ? '命令或当前 Skill 入口保持环境用途' : '修改长期用途，不代替本次导出选择'} onChange={event => void safely(() => onBind(item.id, event.target.value as ExternalRole, undefined, { expectedFingerprint: item.referenceFingerprint }), item.id)}>{roles.map(role => <option key={role.value} value={role.value}>{role.label}</option>)}</select></label>
          {['command', 'skill_alias', 'package_file'].includes(item.referenceKind || '') && <button className="button" type="button" disabled={!onLocate} onClick={() => presentation.fileByReference.has(item.id) ? revealReference(item.id) : onLocate?.(item)}><LocateFixed size={14} />{item.referenceKind === 'command' ? '查看引用' : '定位包内文件'}</button>}{item.bindingStale && <button className="button" type="button" disabled={busy || readOnly} onClick={() => void safely(() => onBind(item.id, actualRole(item), undefined, { expectedFingerprint: item.referenceFingerprint }), item.id)}>重新确认用途</button>}
          <button className="button" type="button" title={item.referenceKind === 'command' ? '这是客户端命令，无需绑定文件' : item.referenceKind === 'skill_alias' ? '当前 Skill 目录入口不重复绑定' : readOnly ? '托管 Skill 只读，不能更换绑定' : '绑定实际文件，保留当前用途'} disabled={busy || readOnly || item.referenceKind === 'command' || item.referenceKind === 'skill_alias'} onClick={() => void chooseFile(item)}><FolderOpen size={14} />{item.referenceKind === 'package_file' ? '更换绑定' : '绑定文件'}</button>{item.remote?.commit && !['command', 'skill_alias'].includes(item.referenceKind || '') && <button className="button" type="button" disabled={busy || readOnly} onClick={() => beginMappings([item], 'github')}>{item.remote?.status === 'available' ? '查看匹配／更换' : '选择 GitHub 文件'}</button>}
        </div>{rowErrors[item.id] && <p className="error-text" role="alert">{rowErrors[item.id]}</p>}{mappingEditor(item)}
        {scanIssues.length > 0 && <div className="external-runtime-records" aria-label="对应扫描记录">{scanIssues.map((issue, index) => <p className={`severity-${issue.severity}`} key={`${issue.category}:${index}`}><strong>{issue.severity === 'error' ? '缺失或不可用' : issue.severity === 'warning' ? '待核对' : '扫描说明'}：</strong>{issue.reason}</p>)}</div>}
        <details className="external-evidence" data-fold-key={`reference:${item.id}:evidence`}><summary>来源与核验详情</summary><p>引用目标：<code>{item.target}</code></p><p>实际来源：<code>{item.canonicalPath || resource?.localPath || item.local?.path || '尚未确认'}</code></p>{resource?.zipPath && <p>ZIP 内路径：<code>{resource.zipPath}</code></p>}<p>{item.reason}</p>{resource?.reason && resource.reason !== item.reason && <p>{resource.reason}</p>}{item.remote?.repositoryPath && <p>GitHub：<code>{item.remote.repo} / {item.remote.repositoryPath}</code> · {item.remote.commit?.slice(0, 12)}</p>}{item.remote?.error && <p className="error-text">{item.remote.error}</p>}<p>最近检查：{item.checkedAt}。可读性与实际调用分别验证。</p>{(item.required || actualRole(item) === 'optional') && !item.portable && item.status === 'available' && <p className="external-environment-note">此引用不能安全自动改写；资源保留在包中，使用时仍需要对应环境。</p>}</details>
      </details>}
    </div>;
  }
  function renderStandaloneCluster(references: ExternalDependency[]) {
    const first = references[0], resource = resourceOf(first);
    if (layout === 'legacy' && !exportResources && references.length === 1) return renderItem(first);
    const issues = references.reduce((count, item) => count + Number(issueCountFor(item) > 0), 0);
    const path = resourcePathParts(first.canonicalPath || resource?.path || first.target);
    const state = exportResourceState(resource);
    return <div className="external-resource-cluster" key={resource?.id || references[0].id} data-external-resource={resource?.id}>
      <FileResourceRow resource={resource} required={first.required || undefined} evidencePending={evidencePending} title={path.name} subtitle={path.directory} source={resource ? exportSourceLabel(resource.source) : labels[first.status]} description={state.problem || ['environment', 'history'].includes(groupOf(first)) ? resource?.reason || first.reason : undefined} sizeText={typeof resource?.size !== 'number' ? undefined : resource.size < 1024 ? `${resource.size} B` : resource.size < 1048576 ? `${(resource.size / 1024).toFixed(1)} KB` : `${(resource.size / 1048576).toFixed(1)} MB`} filePath={first.canonicalPath || first.target} onToggle={onExportToggle} disabled={busy}>
        <details className={`external-attached ${issues ? 'has-issues' : ''}`} data-fold-key={`resource:${resource?.id || `reference:${first.id}`}:references`} open={managementMode || issues > 0 || undefined} onKeyDown={manageKeyboard}><summary>引用 {references.length} 处{issues > 0 && <span className="external-issue-count">{issues} 项需要处理</span>}</summary><div className="external-attached-content">{references.map(item => renderItem(item, true))}</div></details>
      </FileResourceRow>
    </div>;
  }
  function referencesFor(path: string) {
    const references = presentation.byFile.get(path) || [];
    if (!references.length) return null;
    const issues = references.reduce((count, item) => count + Number(issueCountFor(item) > 0), 0);
    return <details className={`external-attached ${issues ? 'has-issues' : ''}`} data-fold-key={`file:${path}:references`} open={issues > 0 || undefined} data-file-references={path} onKeyDown={manageKeyboard}>
      <summary><span>引用 {references.length} 处</span>{issues > 0 && <span className="external-issue-count">{issues} 项需处理</span>}</summary>
      <div className="external-attached-content">{references.map(item => renderItem(item, true))}</div>
    </details>;
  }
  function manageKeyboard(event: ReactKeyboardEvent<HTMLElement>) { const target = event.target; if (target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement || (target instanceof HTMLInputElement && target.type !== 'checkbox') || (target instanceof HTMLElement && target.isContentEditable)) return; if (managementMode && event.ctrlKey && event.key.toLowerCase() === 'a') { event.preventDefault(); event.stopPropagation(); setManaged(filtered.map(item => item.id)); } }
  function nextProblem() { const problems = filtered.filter(item => issueCountFor(item) > 0), current = problems.findIndex(item => item.id === anchor.current), problem = problems[(current + 1) % problems.length]; if (!problem) return; anchor.current = problem.id; revealReference(problem.id); }

  const allStandalone = items.filter(item => !presentation.fileByReference.has(item.id));
  const orphans = (exportResources || []).filter(resource => ['optional_external', 'environment'].includes(resource.group) && resource.source !== 'generated' && !items.some(item => resource.referenceIds.includes(item.id)));
  const visibleOrphans = filterExportResources(orphans, query).filter(resource => filter === 'all' || (filter === 'problems' ? exportResourceState(resource).problem : filter === 'github' ? resource.source === 'github' : resource.group === 'required'));
  const resourceCount = (group: string) => new Set(allStandalone.filter(item => groupOf(item) === group).map(resourceOf).filter((resource): resource is ExportResource => !!resource).map(resource => resource.id).concat(orphans.filter(resource => resource.group === group).map(resource => resource.id))).size;
  const counts = { requiredExternal: resourceCount('required'), optionalExternal: resourceCount('optional_external'), environment: clusterExportReferences(allStandalone.filter(item => ['environment', 'history'].includes(groupOf(item))), exportResources || []).length + orphans.filter(resource => resource.group === 'environment').length, attachedFiles: presentation.byFile.size, references: items.length };
  const toolbar = items.length > 0 || orphans.length > 0 ? <div className="external-toolbar" tabIndex={0} onKeyDown={manageKeyboard}>
    <div className={`external-heading ${blocked ? 'is-blocked' : ''}`}>{blocked > 0 && <span className="external-toolbar-status" role="status"><CircleAlert size={15} />{blocked} 条必要引用需处理</span>}<button className="button" type="button" aria-pressed={managementMode} onClick={() => setManagementMode(value => !value)}>{managementMode ? '结束批量管理' : '批量管理'}</button><button className="button" type="button" disabled={busy} onClick={onCheck}><RefreshCw size={14} />检查本地与 GitHub</button></div>
    <details className="external-controls" data-fold-key="external-management-toolbar" open={managementMode || !!query || filter !== 'all' || undefined}><summary>搜索、筛选与管理引用{query || filter !== 'all' ? ` · 当前 ${filtered.length} 条` : ''}</summary>
    <div className="external-tools">
      <label className="external-search"><Search size={15} /><input aria-label="搜索外部引用" placeholder="搜索文件、路径或引用位置" value={query} onChange={event => setQuery(event.target.value)} /></label>
      <select aria-label="筛选外部引用" value={filter} onChange={event => setFilter(event.target.value)}><option value="all">全部引用</option><option value="problems">需要处理</option><option value="github">GitHub 可用</option><option value="required">必要文件</option></select><button className="button" type="button" onClick={nextProblem}>下一个问题<ChevronRight size={14} /></button>
    </div>
    {managementMode && <div className="external-batch" aria-label="批量管理外部引用"><label><input type="checkbox" checked={filtered.length > 0 && filtered.every(item => managed.includes(item.id))} onChange={event => setManaged(event.target.checked ? [...new Set([...managed, ...filtered.map(item => item.id)])] : managed.filter(id => !filtered.some(item => item.id === id)))} />选择当前结果</label><small>管理已选 {chosen.length} 项</small>{chosen.length > 0 && <><select aria-label="批量用途" value={batchRole} onChange={event => setBatchRole(event.target.value as ExternalRole)}>{roles.map(role => <option key={role.value} value={role.value}>{role.label}</option>)}</select>
      <button className="button" type="button" disabled={busy || readOnly || !chosen.length || !onBatch} onClick={() => void batch('role')}>设置用途</button><button className="button" type="button" disabled={busy || readOnly || !chosen.length} onClick={() => beginMappings(chosen)}>批量绑定</button><button className="button" type="button" disabled={busy || readOnly || !chosen.length || !onBatch} onClick={() => void batch('remap')}>重新匹配来源</button><button className="button" type="button" disabled={busy || !chosen.length || !onBatch} onClick={() => void batch('recheck')}>核验所选</button><button className="button" type="button" disabled={busy || readOnly || !chosen.length || !onBatch} onClick={() => void batch('auto_match')}>自动匹配所选</button>{managed.length > 0 && <button className="button" type="button" onClick={() => setManaged([])}>取消选择</button>}</>}
    </div>}
    {managementMode && <p className="external-note">管理选择仅用于改用途、绑定和核验，不影响本次 ZIP。</p>}
    {presentation.fileByReference.size > 0 && <p className="external-note external-attached-summary">{presentation.fileByReference.size} 条引用位于 {presentation.byFile.size} 个包内文件中，可在对应文件行展开。</p>}
    {!filtered.length && <p className="external-note">没有符合当前筛选的引用。</p>}
    {filtered.some(item => presentation.fileByReference.has(item.id)) && (query || filter !== 'all') && <div className="external-attached-matches"><span>文件清单中的匹配引用</span>{filtered.filter(item => presentation.fileByReference.has(item.id)).map(item => <button key={item.id} className="text-button" type="button" onClick={() => revealReference(item.id)}>{presentation.fileByReference.get(item.id)} · {item.source}{rowErrors[item.id] && ' · 需处理'}</button>)}</div>}
    </details>
    {error && <p className="error-text" role="alert">{error}</p>}
    {mappings && <p className="external-note" role="status">正在核对 {mappings.length} 项映射；编辑器位于对应文件旁，确认后一起保存。</p>}
  </div> : null;
  function sectionFor(id: string) {
    const references = groups.find(group => group.id === id)?.items || [];
    const resources = visibleOrphans.filter(resource => resource.group === id);
    if (!references.length && !resources.length) return null;
    return <div className="external-section-content" tabIndex={0} onKeyDown={manageKeyboard}>{clusterExportReferences(references, exportResources || []).map(renderStandaloneCluster)}{resources.map(resource => <ExportResourceRow key={resource.id} resource={resource} busy={busy || !onExportToggle} onToggle={(id, included) => onExportToggle?.(id, included)} />)}</div>;
  }
  const sections: ExternalSections = { referencesFor, requiredByReference: path => (presentation.byFile.get(path) || []).some(item => item.required), issueCount: path => (presentation.byFile.get(path) || []).filter(item => issueCountFor(item) > 0).length, toolbar, requiredExternal: sectionFor('required'), optionalExternal: sectionFor('optional_external'), environment: sectionFor('environment'), counts };
  if (layout === 'sections') return <>{children?.(sections)}</>;
  return <>{children?.(sections)}{toolbar && <section className="external-dependencies" aria-label="外部依赖" tabIndex={0} onKeyDown={manageKeyboard}>{toolbar}{groups.map(group => <details className="external-group" key={group.id} data-fold-key={`external-group:${group.id}`} open={group.open || group.items.some(item => issueCountFor(item) > 0) || undefined}><summary data-outline-label={group.name} data-outline-level="2">{group.name}<small>{clusterExportReferences(group.items, exportResources || []).length} 项</small></summary>{sectionFor(group.id)}</details>)}{!groups.some(group => group.id === 'optional_external') && sections.optionalExternal}{!groups.some(group => group.id === 'environment') && sections.environment}</section>}</>;
}
