import { useCallback, useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import './storage.css';

type Kind = 'state' | 'webview' | 'recovery' | 'receipts' | 'temp' | 'newSkill' | 'export';
type Config = { managedRoot: string; overrides: Partial<Record<Kind, string>> };
type PathInfo = { effective: string; configured?: string; source: string; locked: boolean; restartRequired: boolean };
type Storage = { managedRoot: string; config: Config; paths: Record<Kind, PathInfo>; bootstrapPath?: string; lastMigration?: { status: string; error?: string }; developer?: { projectRoot: string; dependencyRoot?: string; configurationFile?: string; error?: string }; systemManaged: string[]; atomicTemporary: string };
type Preview = { id: string; changes: { kind: Kind; from: string; to: string; restartRequired: boolean }[]; files: number; bytes: number; spaceChecked: boolean; restartRequired: boolean; scope: string };
type Statistics = { checkedAt: string; rows: { kind: string; label?: string; developer?: boolean; path: string; files: number; bytes: number; skippedLinks: number; error?: string }[] };
type Request = <T = unknown>(action: string, args?: Record<string, unknown>) => Promise<T>;
export type StorageSettingsProps = { request: Request; onRequestRestart: (previewId: string) => Promise<void> };
export type StorageSettingsDraft = { config?: Config; advanced?: boolean };
const labels: Record<Kind, string> = { state: '管理数据', webview: '界面缓存', recovery: '更新恢复材料', receipts: '上传恢复记录', temp: '临时工作目录', newSkill: '新下载 Skill', export: '导出 ZIP' };
const sources: Record<string, string> = { commandLine: '启动参数', environment: '环境变量', legacySidecar: '启动路径文件', bootstrap: '保存位置配置', override: '单独指定', applicationDefault: '程序默认', fixture: '独立测试目录' };
function size(bytes: number) { return bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes / 1024).toFixed(1)} KB` : bytes < 1073741824 ? `${(bytes / 1048576).toFixed(1)} MB` : `${(bytes / 1073741824).toFixed(2)} GB`; }
const errorText = (cause: unknown) => cause instanceof Error ? cause.message : String(cause);

export function StorageSettings({ request: suppliedRequest, onRequestRestart, initialDraft, onDraftChange }: StorageSettingsProps & { initialDraft?: StorageSettingsDraft; onDraftChange?: (draft: StorageSettingsDraft) => void }) {
  const requestRef = useRef(suppliedRequest); requestRef.current = suppliedRequest;
  const request = useCallback<Request>((action, args) => requestRef.current(action, args), []);
  const [storage, setStorage] = useState<Storage | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [advanced, setAdvanced] = useState(initialDraft?.advanced || false);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [statistics, setStatistics] = useState<Statistics | null>(null);
  const [busy, setBusy] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const draftRef = useRef(initialDraft); const onDraftChangeRef = useRef(onDraftChange); onDraftChangeRef.current = onDraftChange;
  useEffect(() => { if (config) onDraftChangeRef.current?.({ config, advanced }); }, [config, advanced]);
  const refresh = useCallback(async (replaceDraft = true) => {
    const result = await request<{ storage: Storage }>('storage_get');
    setStorage(result.storage);
    if (replaceDraft) setConfig(result.storage.config);
  }, [request]);
  useEffect(() => { let alive = true; void request<{ storage: Storage }>('storage_get').then(result => { if (alive) { setStorage(result.storage); setConfig(draftRef.current?.config || result.storage.config); } }).catch(cause => { if (alive) setError(errorText(cause)); }); return () => { alive = false; }; }, [request]);
  useEffect(() => { let unlisten: (() => void) | undefined; let stopped = false; void listen('state-changed', () => { void refresh(false).catch(() => undefined); }).then(stop => { if (stopped) stop(); else unlisten = stop; }).catch(() => undefined); return () => { stopped = true; unlisten?.(); }; }, [refresh]);
  const update = (next: Config) => { setConfig(next); setPreview(null); setMessage(''); setError(''); };
  async function choose(kind?: Kind) {
    if (!config) return;
    try {
      const path = await open({ directory: true, multiple: false, title: kind ? `选择${labels[kind]}保存位置` : '选择管理器根目录' });
      if (typeof path === 'string') update(kind ? { ...config, overrides: { ...config.overrides, [kind]: path } } : { ...config, managedRoot: path });
    } catch (cause) { setError(errorText(cause)); }
  }
  async function inspect() {
    if (!config) return;
    setBusy('preview'); setError(''); setMessage('');
    try { const result = await request<{ preview: Preview }>('storage_preview', { config }); setPreview(result.preview); }
    catch (cause) { setError(errorText(cause)); }
    finally { setBusy(''); }
  }
  async function apply() {
    if (!preview) return;
    setBusy('apply'); setError('');
    try {
      if (preview.restartRequired) await onRequestRestart(preview.id);
      else { await request('storage_apply_defaults', { previewId: preview.id }); await refresh(); setPreview(null); setMessage('默认保存位置已更新；其他窗口会同步读取。'); }
    } catch (cause) { setError(errorText(cause)); }
    finally { setBusy(''); }
  }
  async function checkSize() {
    setBusy('stats'); setError('');
    try { setStatistics(await request<Statistics>('storage_stats')); }
    catch (cause) { setError(errorText(cause)); }
    finally { setBusy(''); }
  }
  if (!storage || !config) return <section className="card storage-settings"><div className="card-header"><h2>保存位置</h2></div><div className="storage-content">{error || '正在读取实际保存位置…'}</div></section>;
  const locked = storage.paths.state.locked;
  const renderPath = (kind: Kind) => <div className="storage-path" key={kind}>
          <div className="storage-path-title"><strong>{labels[kind]}</strong><span>{storage.paths[kind].restartRequired ? '更改后重启' : '立即应用'}</span></div>
          <div className="storage-effective"><code>{storage.paths[kind].effective}</code><button className="storage-link" onClick={() => void request('storage_open_path', { kind }).catch(cause => setError(errorText(cause)))}>打开</button></div>
          <small>当前来源：{sources[storage.paths[kind].source] || storage.paths[kind].source}{storage.paths[kind].locked ? ' · 已锁定' : ''}{statistics?.rows.find(row => row.kind === kind) ? ` · ${size(statistics.rows.find(row => row.kind === kind)!.bytes)} · ${statistics.rows.find(row => row.kind === kind)!.files} 个文件` : ''}</small>
          {advanced && <div className="storage-input-row"><input aria-label={`${labels[kind]}自定义目录`} placeholder="留空跟随根目录" value={config.overrides[kind] || ''} readOnly={storage.paths[kind].locked} onChange={event => { const overrides = { ...config.overrides }; if (event.target.value) overrides[kind] = event.target.value; else delete overrides[kind]; update({ ...config, overrides }); }} /><button className="button secondary" disabled={storage.paths[kind].locked || !!busy} onClick={() => void choose(kind)}>选择</button></div>}
          {statistics?.rows.find(row => row.kind === kind)?.error && <small className="storage-error">统计未完成：{statistics.rows.find(row => row.kind === kind)?.error}</small>}
        </div>;
  const secondaryKinds: Kind[] = ['webview', 'recovery', 'receipts', 'temp'];
  const customSecondary = secondaryKinds.filter(kind => config.overrides[kind] || storage.paths[kind].locked).length;
  const secondaryErrors = statistics?.rows.filter(row => secondaryKinds.includes(row.kind as Kind) && row.error).length || 0;
  return <section className="card storage-settings" aria-labelledby="storage-settings-title">
    <div className="card-header"><div><h2 id="storage-settings-title">保存位置</h2></div><button className="button secondary" disabled={!!busy} onClick={() => void checkSize()}>{busy === 'stats' ? '正在统计…' : '查看占用'}</button></div>
    <div className="storage-content">
      {error && <p className="storage-feedback storage-error" role="alert">{error}</p>}
      {message && <p className="storage-feedback" role="status">{message}</p>}
      {storage.lastMigration && <p className={`storage-feedback${storage.lastMigration.status === 'verified' ? '' : ' storage-error'}`}>{storage.lastMigration.status === 'verified' ? '上次迁移已通过重启后的文件与数据库检查；旧文件保留。' : `上次迁移未完成，已保留原位置。${storage.lastMigration.error || ''}`}</p>}
      <label className="storage-root-label">管理器根目录<div className="storage-input-row"><input aria-label="管理器根目录" value={config.managedRoot} readOnly={locked} onChange={event => update({ ...config, managedRoot: event.target.value })} /><button className="button secondary" disabled={locked || !!busy} onClick={() => void choose()}>选择</button></div></label>
      <p className="storage-hint">{locked ? `由${sources[storage.paths.state.source] || storage.paths.state.source}指定；移除该覆盖后才能更改内部数据位置。` : '根目录变化只处理管理数据与缓存；源码、工具链和已登记的 Skill 不会自动搬动。'}</p>
      <button className="storage-disclosure" aria-expanded={advanced} onClick={() => setAdvanced(value => !value)}>{advanced ? '▾' : '▸'} 单独设置各类目录</button>
      <div className="storage-paths">
        {(['state', 'newSkill', 'export'] as Kind[]).map(renderPath)}
        <details className="storage-secondary" open={advanced || undefined}><summary>缓存、恢复与临时目录<span>{secondaryKinds.length} 类{customSecondary ? ` · ${customSecondary} 项自定义／锁定` : ' · 跟随管理器根目录'}{secondaryErrors ? ` · ${secondaryErrors} 项统计异常` : ''}</span></summary><div className="storage-paths">{secondaryKinds.map(renderPath)}</div></details>
      </div>
      {statistics && <p className="storage-hint">检查时间：{new Date(statistics.checkedAt).toLocaleString()}。按真实路径去重，跳过目录链接，不扫描系统凭据或客户端其他文件。</p>}
      {storage.developer && <details className="storage-boundaries"><summary>开发与构建文件（单独统计）{storage.developer.error ? " · 配置异常" : ""}{statistics?.rows.some(row => row.developer && row.error) ? " · 统计异常" : ""}</summary><p>源码：<code>{storage.developer.projectRoot}</code></p>{storage.developer.error ? <p className="storage-error">{storage.developer.error}</p> : <><p>依赖与构建根目录：<code>{storage.developer.dependencyRoot}</code></p><p>可手动编辑 <code>{storage.developer.configurationFile}</code> 的 dependencyRoot，然后通过 scripts/build.ps1 使用；源码、已有工具链和开发文件不会自动迁移。</p></>}{statistics?.rows.filter(row => row.developer).map(row => <div className="storage-path" key={row.kind}><strong>{row.label}</strong><code>{row.path}</code><small>{size(row.bytes)} · {row.files} 个文件 · 跳过 {row.skippedLinks} 个链接{row.error ? ` · 未完成：${row.error}` : ''}</small></div>)}</details>}
      {preview && <div className="storage-preview" aria-label="保存位置变更预览"><strong>{preview.changes.length ? '将应用以下变更' : '保存位置没有变化'}</strong>{preview.changes.map(change => <div key={change.kind}><b>{labels[change.kind]}</b><code>{change.from}</code><span>→</span><code>{change.to}</code></div>)}<p>预计复制 {preview.files} 个文件 · {size(preview.bytes)}{preview.spaceChecked ? ' · 已检查目标写入权限与可用空间' : ' · 当前系统不能预检可用空间'}。</p><p>{preview.scope}。迁移先复制并校验，原件保留；失败会返回原位置。</p>{preview.restartRequired && <p>所有窗口将先保存，等待正在写入的任务完成，再关闭界面缓存、复制数据并重启。</p>}</div>}
      <div className="form-actions"><button className="button secondary" disabled={!!busy} onClick={() => void inspect()}>{busy === 'preview' ? '正在检查…' : '预览变更'}</button>{preview && preview.changes.length > 0 && <button className="button primary" disabled={!!busy} onClick={() => void apply()}>{busy === 'apply' ? '正在保存…' : preview.restartRequired ? '保存并重启' : '应用默认位置'}</button>}</div>
      <details className="storage-boundaries"><summary>由系统或客户端管理的位置</summary><p>{storage.systemManaged.join('；')}。</p><p>{storage.atomicTemporary}。下载和更新仍写入明确选定的 Skill；导出可每次选择位置。</p></details>
    </div>
  </section>;
}
