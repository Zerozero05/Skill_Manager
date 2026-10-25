import { useEffect, useMemo, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { FolderOpen, LoaderCircle, Plus, RefreshCw, Search, X } from 'lucide-react';
import { candidatePath, localCandidates, localPathKey, rangePaths, uniqueCandidates, type LocalCandidate, type LocalImportDraft, type LocalRoot } from './local-sources';
import './checks.css';

type Props = { roots: readonly LocalRoot[]; ignoredPaths: readonly string[]; skills: readonly LocalCandidate[]; initialDraft?: LocalImportDraft; onDraft: (draft: LocalImportDraft) => void; action: <T>(label: string, action: string, args: Record<string, unknown>) => Promise<T>; onClose: () => void };
export function LocalSources({ roots, ignoredPaths, skills, initialDraft, onDraft, action, onClose }: Props) {
  const [draft, setDraft] = useState<LocalImportDraft>(() => initialDraft || { paths: [], input: '', selected: [], query: '', automatic: true });
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [notice, setNotice] = useState('');
  const [rootSelection, setRootSelection] = useState<string[]>([]), [ignoredSelection, setIgnoredSelection] = useState<string[]>([]);
  const [renaming, setRenaming] = useState(''), [rootName, setRootName] = useState('');
  const anchor = useRef<string | null>(null), alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { onDraft(draft); }, [draft, onDraft]);
  const managed = useMemo(() => new Set(skills.map(candidatePath).filter(Boolean).map(localPathKey)), [skills]);
  const candidates = draft.candidates || [];
  const matching = useMemo(() => localCandidates(candidates, draft.query), [candidates, draft.query]);
  const selectable = matching.filter(item => !managed.has(localPathKey(candidatePath(item)))).map(candidatePath);
  async function execute(label: string, command: string, args: Record<string, unknown>, success: string) {
    if (busy) return; setBusy(true); setError(''); setNotice('');
    try { const result = await action(label, command, args); if (alive.current) setNotice(success); return result; }
    catch (cause) { if (alive.current) setError(String(cause)); }
    finally { if (alive.current) setBusy(false); }
  }
  function pathsChanged(paths: string[]) { setDraft(previous => ({ ...previous, paths: [...new Map(paths.map(path => [localPathKey(path), path])).values()], candidates: undefined, selected: [], warnings: [] })); }
  async function choose() {
    try { const value = await open({ directory: true, multiple: true, title: '选择自动导入目录或 Skill 目录' }); if (value) pathsChanged([...draft.paths, ...(Array.isArray(value) ? value : [value])]); }
    catch (cause) { setError(String(cause)); }
  }
  async function scan() {
    if (busy) return;
    const paths = [...draft.paths, ...(draft.input.trim() ? [draft.input.trim()] : [])];
    if (!paths.length) return;
    setBusy(true); setError(''); setNotice('');
    try {
      const found: LocalCandidate[] = [], warnings: string[] = [];
      // Explicit preview only; background watching uses the backend lightweight index.
      for (const path of paths) { const value = await action<{ skills: LocalCandidate[]; warnings?: string[] }>('正在预览本地目录', 'scan_local', { path }); found.push(...value.skills); warnings.push(...value.warnings || []); }
      const unique = uniqueCandidates(found);
      if (alive.current) setDraft(previous => ({ ...previous, paths: [...new Map(paths.map(path => [localPathKey(path), path])).values()], input: '', candidates: unique, warnings, selected: unique.filter(item => !item.readOnly && !managed.has(localPathKey(candidatePath(item))) && !ignoredPaths.some(path => localPathKey(path) === localPathKey(candidatePath(item)))).map(candidatePath) }));
    } catch (cause) { if (alive.current) setError(String(cause)); }
    finally { if (alive.current) setBusy(false); }
  }
  async function register() {
    const selected = draft.selected.filter(path => !managed.has(localPathKey(path)));
    if (draft.automatic) {
      const excludedPaths = candidates.filter(item => !managed.has(localPathKey(candidatePath(item))) && !selected.includes(candidatePath(item))).map(candidatePath);
      const result = await execute('正在保存自动导入目录', 'add_local_roots', { paths: draft.paths, selectedPaths: selected, excludedPaths }, '目录已保存；新增 Skill 会自动登记，未勾选项已记住。');
      if (result) { setDraft(previous => ({ ...previous, paths: [], candidates: undefined, selected: [] })); }
    } else if (selected.length) await execute('正在登记本地 Skill', 'add_local', { paths: selected }, '已登记原目录。');
  }
  const pick = (path: string, checked: boolean, shift: boolean) => { const previousAnchor = anchor.current; setDraft(previous => ({ ...previous, selected: rangePaths(selectable, previous.selected, path, previousAnchor, checked, shift) })); anchor.current = path; };
  return <div className="local-import">
    <div className="local-import-toolbar"><input aria-label="本地目录" placeholder="输入目录路径，或多选文件夹" value={draft.input} disabled={busy} onChange={event => setDraft(previous => ({ ...previous, input: event.target.value }))} onKeyDown={event => { if (event.key === 'Enter') void scan(); }} /><button className="button" disabled={busy} onClick={() => void choose()}><FolderOpen size={16} />选择目录</button><button className="button" disabled={busy || !draft.paths.length && !draft.input.trim()} onClick={() => void scan()}>{busy ? <LoaderCircle size={16} className="spin" /> : <Search size={16} />}预览</button></div>
    {!!draft.paths.length && <ul className="local-path-list">{draft.paths.map(path => <li key={localPathKey(path)}><FolderOpen size={15} /><code>{path}</code><button className="icon-button" aria-label={`移除待添加目录 ${path}`} disabled={busy} onClick={() => pathsChanged(draft.paths.filter(item => item !== path))}><X size={15} /></button></li>)}</ul>}
    {error && <p className="notice error" role="alert">{error}</p>}{notice && <p className="notice success" role="status">{notice}</p>}
    {draft.warnings?.map((warning, index) => <p className="inline-warning" key={index}>{warning}</p>)}
    {draft.candidates && <section aria-label="目录预览"><div className="local-import-actions"><input className="local-import-search" aria-label="搜索发现的 Skill" placeholder="搜索名称或路径" value={draft.query} onChange={event => setDraft(previous => ({ ...previous, query: event.target.value }))} /><button className="text-button" disabled={busy || !selectable.length} onClick={() => setDraft(previous => ({ ...previous, selected: [...new Set([...previous.selected, ...selectable])] }))}>选择当前结果 ({selectable.length})</button><button className="text-button" disabled={busy || !draft.selected.length} onClick={() => setDraft(previous => ({ ...previous, selected: [] }))}>取消选择</button><span>{draft.selected.length} 项已选</span></div>
      <div className="local-discovery-list" tabIndex={0} aria-label="发现的 Skill 列表" onKeyDown={event => { if ((event.target as HTMLElement).matches('input:not([type="checkbox"]),textarea,[contenteditable="true"]')) return; if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') { event.preventDefault(); event.stopPropagation(); setDraft(previous => ({ ...previous, selected: [...new Set([...previous.selected, ...selectable])] })); } if (event.key === 'Escape') { event.preventDefault(); setDraft(previous => ({ ...previous, selected: [] })); } }}>
        {matching.map(item => { const path = candidatePath(item), known = managed.has(localPathKey(path)); return <label className="local-discovery-row" key={localPathKey(path)}><input type="checkbox" aria-label={`选择 ${item.displayName || item.name}`} checked={known || draft.selected.includes(path)} disabled={busy || known} onClick={event => pick(path, event.currentTarget.checked, event.shiftKey)} onChange={() => {}} /><div><strong>{item.displayName || item.name}</strong><small>{path}</small>{item.description && <p>{item.description}</p>}</div><span className="mini-tag">{known ? '已管理' : item.readOnly ? '只读' : '新增'}</span></label>; })}{!matching.length && <p>没有匹配的 Skill。</p>}
      </div>
      <div className="local-import-submit"><label><input type="checkbox" checked={draft.automatic} disabled={busy} onChange={event => setDraft(previous => ({ ...previous, automatic: event.target.checked }))} />以后自动导入这些目录中的新 Skill</label><button className="button primary" disabled={busy || !draft.paths.length || !draft.automatic && !draft.selected.length} onClick={() => void register()}><Plus size={16} />{draft.automatic ? '保存目录并登记所选' : `登记所选 (${draft.selected.length})`}</button></div>
    </section>}
    <section className="local-roots" aria-label="自动导入目录"><h3>自动导入目录 <span className="muted">{roots.length}</span></h3><div className="local-source-actions"><button className="text-button" disabled={busy || !roots.length} onClick={() => setRootSelection(roots.map(root => root.id))}>全选</button><button className="text-button" disabled={busy || !rootSelection.length} onClick={() => setRootSelection([])}>取消选择</button>{!!rootSelection.length && <><button className="button" disabled={busy} onClick={() => void execute('正在检查所选目录', 'check_now', { rootIds: rootSelection }, '所选目录已检查。')}><RefreshCw size={14} />检查所选</button><button className="button" disabled={busy} onClick={async () => { const value = await execute('正在移除自动导入目录', 'remove_local_roots', { rootIds: rootSelection }, '已停止导入；Skill 记录、原文件和客户端连接均保留。'); if (value) setRootSelection([]); }}>移除目录 ({rootSelection.length})</button></>}</div>
      {roots.map(root => <div className="local-root-row" key={root.id}><input type="checkbox" aria-label={`选择目录 ${root.name}`} disabled={busy} checked={rootSelection.includes(root.id)} onChange={event => setRootSelection(previous => event.target.checked ? [...previous, root.id] : previous.filter(id => id !== root.id))} /><div><div className="local-root-heading">{renaming === root.id ? <><input aria-label="目录名称" value={rootName} onChange={event => setRootName(event.target.value)} /><button className="button" disabled={busy || !rootName.trim()} onClick={async () => { if (await execute('正在更改目录名称', 'update_local_root', { rootId: root.id, name: rootName.trim() }, '名称已保存。')) setRenaming(''); }}>保存</button><button className="text-button" onClick={() => setRenaming('')}>取消</button></> : <><strong>{root.name}</strong><span className="mini-tag">{root.enabled ? '自动导入' : '已暂停'}</span></>}</div><p className="local-root-path">{root.path}</p>{root.error && <p className="error-text">{root.error}</p>}<div className="local-source-actions"><button className="button" disabled={busy} onClick={() => void execute('正在检查目录', 'check_now', { rootIds: [root.id] }, '目录已检查。')}>检查</button><button className="button" disabled={busy} onClick={() => { setRenaming(root.id); setRootName(root.name); }}>改名</button><button className="button" disabled={busy} onClick={() => void execute('正在更改自动导入设置', 'update_local_root', { rootId: root.id, enabled: !root.enabled }, root.enabled ? '目录已暂停自动导入。' : '目录已恢复自动导入。')}>{root.enabled ? '暂停' : '恢复'}</button></div></div></div>)}{!roots.length && <p className="muted">尚未设置。选择目录并预览后保存即可。</p>}
    </section>
    <details className="local-ignored"><summary>忽略的 Skill · {ignoredPaths.length}</summary><div className="local-source-actions"><button className="text-button" disabled={busy || !ignoredPaths.length} onClick={() => setIgnoredSelection([...ignoredPaths])}>全选</button><button className="text-button" disabled={busy || !ignoredSelection.length} onClick={() => setIgnoredSelection([])}>取消选择</button><button className="button" disabled={busy || !ignoredSelection.length} onClick={async () => { if (await execute('正在恢复忽略项', 'restore_local_ignores', { paths: ignoredSelection }, '所选项已恢复自动发现。')) setIgnoredSelection([]); }}>恢复所选 ({ignoredSelection.length})</button></div>{ignoredPaths.map(path => <label className="local-root-path" key={path}><input type="checkbox" disabled={busy} checked={ignoredSelection.includes(path)} onChange={event => setIgnoredSelection(previous => event.target.checked ? [...previous, path] : previous.filter(value => value !== path))} /> {path}</label>)}</details>
    <button className="button" onClick={onClose} disabled={busy}>关闭</button>
  </div>;
}
