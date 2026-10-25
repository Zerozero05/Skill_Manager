import { useCallback, useContext, useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { open, save } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import ReactMarkdown, { defaultUrlTransform } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import packageInfo from '../package.json';
import { comparisonScope, displayStatus, githubFolderUrl, libraryCounts, libraryStatusOptions, statusFilterLabel, needsAttention, paginateLibrary, queryLibrary, rangeSelection, selectionForDrag, statusExplanation, type LibraryStatusFilter } from './library';
import { usePreferenceQueue } from './usePreferenceQueue';
import { libraryMode as normalizeLibraryMode, type LibraryMode } from './library';
import { LibraryCards } from './LibraryCards';
import { ShortcutHelp } from './ShortcutHelp';
import { CategoryChip, CategoryColorPicker } from './CategoryColor';
import { categoryColors, nextCategoryColor, isCategoryColor, type Category, type CategoryColor } from './categories';
import { FloatingMenu, type OpenMenu } from './FloatingMenu';
import { SkillInformation } from './SkillInformation';
import { RuntimeChecks } from './RuntimeChecks';
import { requiredExportPaths, runtimeAttentionCount, type RuntimeIssue } from './runtime';
import { terminalTask, taskStatus, type ManagerTask } from './tasks';
import { WorkspaceSessionContext, useWorkspaceSession, type SessionDraft } from './session';
import { PaneFrame, WorkspaceFeedback, scrollToPaneAnchor } from './WorkspaceShell';
import { ClientPanel, ClientConnectPage, type ClientConnection, type ClientConnectDraft, type ClientManagementMode, type ConnectionResult } from './ClientPanel';
import { NetworkSettings, type NetworkSettingsDraft } from './NetworkSettings';
import { SettingsLayout, SettingsSection } from './SettingsLayout';
import { ChangesPanel } from './ChangesPanel';
import { StorageSettings, type StorageSettingsDraft } from './StorageSettings';
import { ExternalDependencies, type ExternalDependency, type ExternalBindingOptions, type ExternalBatchRequest, type ExternalManagementDraft } from './ExternalDependencies';
import { ExportSelection } from './ExportSelection';
import { FileResourceRow } from './FileResourceRow';
import { OperationHistory, type OperationHistoryDraft } from './OperationHistory';
import { operationHistoryDraft } from './operation-history';
import { OperationSummary } from './OperationSummary';
import { DestinationChoice } from './DestinationChoice';
import { destinationAfterPicker, destinationDraft, destinationReady, type DestinationDraft } from './destination';
import { CLIENT_MANAGEMENT_COPY } from './client-selection';
import { captureSectionFolds, restoreSectionFolds } from './folds';
import type { WorkspaceNavigation } from './workspace';
import { useExportSelection, type ExportSelectionDraft } from './useExportSelection';
import { bridgeMessage } from './bridge-recovery';
import { useExportPlan } from './useExportPlan';
import { useDocumentCache } from './useDocumentCache';
import { BoundedReadCache, documentCacheKey, inventoryReadKey } from './read-cache';
import { isStaleReadSnapshot } from './snapshot-clock';
import './updates.css';
import './tasks.css';
import {
  ArrowDownToLine, ArrowLeftRight, ArrowUpRight, BookOpen, Box, Check, CheckCircle2,
  ChevronLeft, ChevronRight, CircleAlert, CircleHelp, Code2, Copy, Download, Ellipsis,
  ExternalLink, File, FileCode2, Folder, FolderOpen, GitBranch, Github, Layers3,
  LayoutGrid, LayoutPanelLeft, Link2, List, LoaderCircle, LockKeyhole, Monitor, Plus, RefreshCw, Search, Settings2,
  ShieldCheck, SlidersHorizontal, Unlink, Upload, X, GripVertical, PanelLeftClose, PanelLeftOpen, Undo2, ArrowUp, ArrowDown, Star,
} from 'lucide-react';

// Stable component types preserve heading IDs and keyboard focus during saved-scroll rerenders.
const markdownHeadings = {
  h1: ({ children }: { children?: ReactNode }) => <h1 id={String(children).toLowerCase().replace(/\s+/g, '-')}>{children}</h1>,
  h2: ({ children }: { children?: ReactNode }) => <h2 id={String(children).toLowerCase().replace(/\s+/g, '-')}>{children}</h2>,
  h3: ({ children }: { children?: ReactNode }) => <h3 id={String(children).toLowerCase().replace(/\s+/g, '-')}>{children}</h3>,
};

type View = 'skills' | 'changes' | 'clients' | 'settings';
type Filter = 'all' | 'local' | 'github' | 'linked';
type UiTheme = 'light' | 'dark' | 'eye';

type LibrarySort = 'name' | 'status' | 'checked';
type LibraryPageSize = 20 | 50 | 100;

type Skill = {
  id: string; name: string; displayName?: string; description?: string; version?: string | null; categoryId?: string | null; favorite?: boolean;
  localPath?: string | null; path?: string; readOnly?: boolean; sourceId?: string | null;
  remotePath?: string | null; repo?: string | null; branch?: string | null; commit?: string | null;
  localHashes?: Record<string, string>; externalBindings?: unknown; baselineCommit?: string | null; baselineHashes?: Record<string, string>; baselineKind?: string; baselineVerifiedAt?: string; installedCommit?: string | null; runtimeFiles?: string[];
  runtimeConfirmed?: boolean; status?: string; lastChecked?: string | null; lastLocalCheckedAt?: string | null; lastRemoteCheckedAt?: string | null; comparisonScope?: string[]; error?: string | null;
  private?: boolean; dependencies?: unknown; verification?: unknown;
  runtimeWarnings?: string[]; runtimeIssues?: RuntimeIssue[]; runtimeConfirmationKind?: string; runtimeAudit?: { status: string; checkedAt?: string; canAutoConfirm?: boolean }; untrackedFiles?: string[]; localNotice?: string | null; remoteCheckRequired?: boolean; remoteMissing?: boolean;
  changeDetails?: { path: string; change: string; side: 'local' | 'remote' | 'both' }[]; associationNote?: string | null; comparisonNote?: string | null;
};
type Source = { id: string; repo: string; branch?: string; commit?: string; private?: boolean; lastChecked?: string; error?: string; roots?: string[]; rootsInferred?: boolean };
type Connection = ClientConnection & { discovered?: boolean };
type Snapshot = { stateRevision?: number; readGeneration?: number | null; skills: Skill[]; sources: Source[]; connections: Connection[]; settings: { checkIntervalMinutes?: number; editorPath?: string; recoveryMessages?: string[]; uiTheme?: UiTheme; uiZoom?: number; uiFontSize?: number; libraryMode?: LibraryMode; libraryPageSize?: LibraryPageSize; librarySort?: LibrarySort; categories?: Category[]; sidebarWidth?: number; sidebarCollapsed?: boolean }; dataDir?: string; managementUndo?: { available: boolean; label?: string } };
type FileEntry = { path: string; size?: number; hash?: string; runtime?: boolean; missing?: boolean; localAvailable?: boolean; remoteAvailable?: boolean; localHash?: string; remoteHash?: string; contentDifferent?: boolean };
type FileResult = { runtimeFiles?: string[]; runtimeConfirmationKind?: string; runtimeAudit?: { status: string; checkedAt?: string; canAutoConfirm?: boolean }; externalDependencies?: ExternalDependency[]; files: FileEntry[]; runtimeConfirmed: boolean; requiredFiles?: string[]; exportRequiredFiles?: string[]; exportError?: string | null; optionalFiles?: string[]; requiredReasons?: Record<string, string>; suggestedFiles?: string[]; warnings?: string[]; issues?: RuntimeIssue[]; remoteStatus?: 'ok' | 'error' | 'not_linked'; remoteError?: string; remoteCommit?: string; remoteCheckedAt?: string };
type DocumentResult = { commit?: string | null; readGeneration?: number | null; localFingerprint?: string; markdown: string; documentPath?: string; basePath?: string; baseUrl?: string; source?: string; checkedAt?: string; private?: boolean };
type DiffFile = { path: string; change: string; local?: string; remote?: string };
type ClientPreviewEntry = { connectionId: string; skillId?: string; client?: string; label: string; path?: string; target?: string; mode: ClientManagementMode; canApply: boolean; error?: string };
type Preview = { mode?: ClientManagementMode; entries?: ClientPreviewEntry[]; id?: string; kind: string; skillId: string; summary: string; files: DiffFile[]; warnings?: string[]; canApply: boolean; needsRuntimeSelection?: boolean; availableFiles?: (FileEntry | string)[]; requiredFiles?: string[]; optionalFiles?: string[]; requiredReasons?: Record<string, string>; suggestedFiles?: string[]; targetPath?: string; targetRepo?: string; targetBranch?: string; path?: string; target?: string; commit?: string; cleanupFiles?: { path: string; absolutePath: string; reason: string }[] };
type AuditSummary = { checked: number; passed: number; needsReview: number; failed: number; items: { skillId: string; status: string; reason?: string }[] };
type TransferResult = { skillId?: string; commit?: string; submitted?: boolean; unchanged?: boolean; verificationError?: string; cleanup?: { receiptId?: string; recordError?: string; removed: { path: string; absolutePath: string }[]; retained: { path: string; reason: string }[] } };
type CleanupReceipt = { id: string; commit: string; createdAt?: string; files?: { path: string }[] };
type UiContinuation = { kind: 'files' | 'runtime' | 'preview' | 'audit' | 'receipts' | 'export' | 'scan'; skillId?: string; operation?: 'install' | 'push' | 'upload' };
type Notice = { id: number; tone: 'success' | 'error' | 'info'; message: string; skillId?: string };
type Modal = { kind: 'client-result'; mode: ClientManagementMode; results: ConnectionResult[] } | { kind: 'client-connect'; client: string } | { kind: 'local' } | { kind: 'github' } | { kind: 'associate'; skill: Skill } |
  { kind: 'files'; skill: Skill; operation: 'install' | 'push' | 'update' | 'upload'; result: FileResult } |
  { kind: 'receipts'; skill: Skill; receipts: CleanupReceipt[] } |
  { kind: 'audit'; summary: AuditSummary } |
  { kind: 'runtime'; skill: Skill; result: FileResult } |
  { kind: 'task'; task: ManagerTask } |
  { kind: 'export'; skill: Skill; inFlightSelection?: ExportSelectionDraft } |
  { kind: 'preview'; preview: Preview } | { kind: 'forget'; skill: Skill } |
  { kind: 'category'; category?: Category } | { kind: 'delete-category'; category: Category } | null;
const EMPTY: Snapshot = { skills: [], sources: [], connections: [], settings: { checkIntervalMinutes: 30 } };
const CHANGES: Record<string, string> = { add: '新增', added: '新增', modified: '修改', deleted: '移除', unchanged: '一致', conflict: '冲突', different: '待核对' };
const CLIENTS = [{ id: 'codex', name: 'Codex', folder: '.agents/skills' }, { id: 'claude', name: 'Claude Code', folder: '.claude/skills' }, { id: 'cursor', name: 'Cursor', folder: '.cursor/skills' }];
const THEMES: { id: UiTheme; label: string; description: string }[] = [{ id: 'light', label: '明亮', description: '浅灰与白色' }, { id: 'dark', label: '夜间', description: '炭黑与石墨灰' }, { id: 'eye', label: '护眼', description: '低亮度暖灰米色' }];
function displayNumber(value: unknown, fallback: number, min: number, max: number) { const number = Number(value); return Number.isFinite(number) ? Math.min(max, Math.max(min, Math.round(number))) : fallback; }
function skillName(skill: Skill) { return skill.displayName || skill.name || '未命名 Skill'; }
function errText(error: unknown) { return error instanceof Error ? error.message : String(error); }
function dateText(value?: string | null) { if (!value) return '尚未检查'; const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : date.toLocaleString('zh-CN', { hour12: false }); }
function pathJoin(base: string, child: string) { return `${base.replace(/[\\/]+$/, '')}${base.includes('\\') ? '\\' : '/'}${child}`; }
function bytes(size?: number) { return size === undefined ? '—' : size < 1024 ? `${size} B` : size < 1048576 ? `${(size / 1024).toFixed(1)} KB` : `${(size / 1048576).toFixed(1)} MB`; }
function requiredFile(path: string) { return path === 'agents/openai.yaml' || /(^|\/)SKILL\.md$/i.test(path) || /(^|\/)(licen[cs]e|copying|notice)(\.[^/]*)?$/i.test(path); }
function requiredRuntimeFile(result: FileResult, path: string) { return requiredFile(path) || !!result.requiredFiles?.includes(path); }
function runtimeGroups(result: FileResult) { return [{ required: true, files: result.files.filter(file => requiredRuntimeFile(result, file.path)) }, { required: false, files: result.files.filter(file => !requiredRuntimeFile(result, file.path)) }]; }
function requiredReason(result: FileResult, path: string) { return result.requiredReasons?.[path] || (/SKILL\.md$/i.test(path) ? 'Skill 入口与执行规则' : path === 'agents/openai.yaml' ? '客户端调用配置' : /(^|\/)(licen[cs]e|copying|notice)(\.[^/]*)?$/i.test(path) ? '许可证与来源告知' : '运行所需依赖'); }
function inventoryStamp(skill: Skill) { return JSON.stringify([skill.id, skill.localPath, skill.sourceId, skill.repo, skill.branch, skill.remotePath, skill.commit, skill.localHashes, skill.baselineHashes, skill.runtimeConfirmed, skill.runtimeFiles, skill.externalBindings]); }
function localFile(file: FileEntry, skill: Skill) { return file.localAvailable ?? (!!skill.localPath && !file.missing); }
function remoteFile(file: FileEntry, skill: Skill) { return file.remoteAvailable ?? (!skill.localPath && !!skill.sourceId && !file.missing); }
function changedFiles(skill: Skill) { return (skill.changeDetails || []).filter(file => file.change !== 'unchanged'); }
function remoteDirectoryUrl(skill: Skill) { return githubFolderUrl(skill) || ''; }
function tokenCreationUrl(repo: string, access: 'read' | 'write') {
  const url = new URL('https://github.com/settings/personal-access-tokens/new');
  url.searchParams.set('name', 'Skill Manager');
  url.searchParams.set('description', 'Skill Manager: selected repository contents');
  url.searchParams.set('expires_in', '30');
  url.searchParams.set('contents', access);
  const match = /^(?:https:\/\/github\.com\/)?([a-z0-9-]+)\/[a-z0-9_.-]+\/?$/i.exec(repo.trim());
  if (match) url.searchParams.set('target_name', match[1]);
  return url.toString();
}
function resolveDocPath(current: string, target: string) {
  const clean = target.split(/[?#]/, 1)[0];
  let decoded: string; try { decoded = decodeURIComponent(clean); } catch { throw new Error('文档链接包含无效编码'); }
  if (decoded.includes('\\') || decoded.includes('\0')) throw new Error('文档路径无效');
  const parts = decoded.startsWith('/') ? [] : current.split('/').slice(0, -1);
  for (const part of decoded.split('/')) { if (!part || part === '.') continue; if (part === '..') { if (!parts.length) throw new Error('文档链接超出仓库范围'); parts.pop(); } else parts.push(part); }
  return parts.join('/');
}
async function request<T>(action: string, args: Record<string, unknown> = {}) {
  if (!isTauri()) throw new Error('此页面需要在 Skill Manager 桌面程序中打开，浏览器预览无法访问本地目录或凭据。');
  return invoke<T>('manager_request', { request: { action, ...args } });
}
function Badge({ skill }: { skill: Skill }) {
  const status = displayStatus(skill);
  return <span className={`badge ${status.tone}`} title={statusExplanation(skill)} data-status={status.key}><span className="status-dot" />{status.label}</span>;
}
function ComparisonBasis({ skill }: { skill: Skill }) {
  const scope = comparisonScope(skill);
  return <details className="comparison-basis" data-fold-key="comparison-basis"><summary data-outline-key="comparison-basis" data-outline-label="比较依据" data-outline-level="1">比较依据 · {scope.length ? `${scope.length} 个文件` : '范围未知'}</summary><p>{statusExplanation(skill)}</p><dl className="comparison-meta"><dt>GitHub 关联</dt><dd>{skill.sourceId ? '已关联仓库与目录；不代表内容一致' : '未关联'}</dd><dt>运行清单</dt><dd>{skill.runtimeConfirmed ? '已确认' : '待确认；当前比较不能代表完整运行包'}</dd><dt>比较基准</dt><dd>{skill.baselineCommit || (skill.baselineHashes && Object.keys(skill.baselineHashes).length ? '文件基准已保存' : '未知')}</dd><dt>远端提交</dt><dd className="monospace">{skill.commit || '未知'}</dd><dt>最近本地检查</dt><dd>{skill.lastLocalCheckedAt ? dateText(skill.lastLocalCheckedAt) : '未知'}</dd><dt>最近远端检查</dt><dd>{skill.lastRemoteCheckedAt ? dateText(skill.lastRemoteCheckedAt) : '未知'}</dd></dl><strong>实际比较文件 · {scope.length ? scope.length : '未知'} 项</strong>{scope.length ? <ol className="comparison-files">{scope.map(path => <li className="monospace" key={path}>{path}</li>)}</ol> : <p>尚未记录可靠的比较范围，请重新检查。运行文件清单不代替实际比较记录。</p>}</details>;
}
function Button({ children, className = '', variant = 'secondary', ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: 'primary' | 'secondary' | 'ghost' | 'danger'; children: ReactNode }) {
  return <button className={`button ${variant} ${className}`} {...props}>{children}</button>;
}
function Empty({ icon = <Box size={30} />, title, children }: { icon?: ReactNode; title: string; children?: ReactNode }) {
  return <div className="empty"><span className="empty-icon">{icon}</span><h3>{title}</h3>{children && <div className="muted empty-copy">{children}</div>}</div>;
}
function ModalFrame({ title, subtitle, children, footer, onClose, busy, error }: { title: string; subtitle?: string; children: ReactNode; footer?: ReactNode; onClose: () => void; busy: boolean; error?: string }) {
  const session = useContext(WorkspaceSessionContext);
  const titleId = useId();
  const inline = session?.pane.kind === 'operation';
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (inline || session?.active === false) return;
    const previous = document.activeElement as HTMLElement | null;
    const focusable = () => Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),summary,[tabindex="0"]') || []).filter(node => node.getClientRects().length > 0);
    focusable()?.[0]?.focus();
    const listener = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
      if (event.key === 'Tab') { const nodes = focusable(); if (!nodes?.length) return; const first = nodes[0]; const last = nodes[nodes.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }
    };
    document.addEventListener('keydown', listener); return () => { document.removeEventListener('keydown', listener); previous?.focus(); };
  }, [busy, onClose, inline, session?.active]);
  if (inline) return <section ref={panel} className="workflow-page"><div className="workflow-heading" data-outline-label={title}><h2>{title}</h2>{subtitle && <p>{subtitle}</p>}</div>{error && <div role="alert" className="inline-error">{error}</div>}<div className="workflow-content">{children}</div>{footer && <div className="workflow-footer">{footer}</div>}</section>;
  return <div className="modal-backdrop"><div ref={panel} className="modal" role="dialog" aria-modal="true" aria-labelledby={titleId}><div className="modal-heading"><div><h2 id={titleId}>{title}</h2>{subtitle && <p>{subtitle}</p>}</div><button className="icon-button" onClick={onClose} aria-label="关闭窗口" title={busy ? "收起窗口，任务继续运行" : "关闭窗口"}><X size={20} /></button></div>{error && <div className="modal-feedback" role="alert"><div className="inline-error"><CircleAlert size={15} />{error}</div></div>}<div className="modal-content">{children}</div>{footer && <div className="modal-footer">{footer}</div>}</div></div>;
}
function DetailDrawer({ children, feedback, onClose, suspended }: { children: ReactNode; feedback?: ReactNode; onClose: () => void; suspended: boolean }) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (suspended) return;
    const previous = document.activeElement as HTMLElement | null;
    const nodes = () => Array.from(panel.current?.querySelectorAll<HTMLElement>('button:not(:disabled),a[href],input:not(:disabled),select:not(:disabled),summary,[tabindex="0"]') || []).filter(node => node.getClientRects().length > 0);
    nodes()?.[0]?.focus();
    const listener = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); onClose(); }
      if (event.key === 'Tab') { const elements = nodes(); if (!elements?.length) return; const first = elements[0]; const last = elements[elements.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }
    };
    document.addEventListener('keydown', listener);
    return () => { document.removeEventListener('keydown', listener); if (previous?.isConnected) previous.focus(); };
  }, [onClose, suspended]);
  return <div className="library-detail-backdrop"><div ref={panel} className="library-detail-drawer" role="dialog" aria-modal={!suspended} aria-labelledby="library-detail-title"><div className="drawer-heading"><div><span className="eyebrow">SKILL MANAGER</span><h2 id="library-detail-title">Skill 详情</h2></div><button className="icon-button" aria-label="关闭详情" onClick={onClose}><X size={21} /></button></div>{feedback}{children}</div></div>;
}

function AuthImage({ src, alt, skill, version, documentPath, remoteCommit, localOrigin }: { src?: string; alt?: string; skill: Skill; version: 'installed' | 'latest'; documentPath: string; remoteCommit?: string; localOrigin?: boolean }) {
  const [result, setResult] = useState<{ src?: string; error?: string }>({});
  useEffect(() => { let cancelled = false; setResult({}); if (!src) return;
    if (/^https?:\/\//i.test(src)) { setResult({ src }); return; }
    let path: string; try { path = resolveDocPath(documentPath, src); } catch (error) { setResult({ error: errText(error) }); return; }
    request<{ dataUrl: string }>('get_doc_asset', { skillId: skill.id, version, path, ...(remoteCommit ? { remoteCommit } : {}), ...(localOrigin ? { origin: 'local' } : {}) }).then(value => { if (!cancelled) setResult({ src: value.dataUrl }); }).catch(error => { if (!cancelled) setResult({ error: errText(error) }); });
    return () => { cancelled = true; };
  }, [src, skill.id, version, documentPath, remoteCommit, localOrigin]);
  return result.error ? <span className="image-error" title={result.error}><CircleAlert size={14} />{alt || '图片无法加载'}</span> : result.src ? <img src={result.src} alt={alt || ''} loading="lazy" onError={() => setResult({ error: '图片无法加载' })} /> : <span className="image-error"><LoaderCircle size={14} className="spin" />{alt || '正在读取图片'}</span>;
}
function CodeBlock({ children, onCopy }: { children: ReactNode; onCopy: (text: string) => void }) {
  const ref = useRef<HTMLPreElement>(null);
  return <div className="code-block"><button className="code-copy" onClick={() => onCopy(ref.current?.innerText || '')} title="复制代码" aria-label="复制代码"><Copy size={14} /></button><pre ref={ref}>{children}</pre></div>;
}
function RuntimeWarnings({ warnings, issues }: { warnings?: string[]; issues?: RuntimeIssue[] }) {
  return <RuntimeChecks issues={issues} warnings={warnings} />;
}
function TransferNotice({ result, onClose }: { result: TransferResult; onClose: () => void }) {
  return <section className="transfer-result" role="status" aria-live="polite"><div className="section-caption"><strong>{result.unchanged ? '远端内容已一致' : result.submitted ? '文件上传成功' : '上传处理结果'}</strong><Button variant="ghost" aria-label="关闭上传结果" onClick={onClose}><X size={14} /></Button></div>
    {result.commit && <code>提交 {result.commit}</code>}
    {result.verificationError && <p className="error-text">上传已成功，完整内容核对未完成：{result.verificationError}。附件精简结果见下方。</p>}
    {result.cleanup && <><p>本地已精简 {result.cleanup.removed.length} 项 · 保留 {result.cleanup.retained.length} 项</p>
      {result.cleanup.recordError && <p className="error-text">管理范围保存未完成：{result.cleanup.recordError}</p>}
      {result.cleanup.removed.map(file => <p className="muted small-text" key={file.path}>已精简：{file.absolutePath || file.path}</p>)}
      {result.cleanup.retained.map(file => <p className="muted small-text" key={file.path}>{file.path}：{file.reason}</p>)}</>}
  </section>;
}
function RuntimeFileGroup({ required, total, selected, description, children }: { required: boolean; total: number; selected: number; description: string; children: ReactNode }) {
  const [expanded, setExpanded] = useState(required);
  const details = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const node = details.current;
    const expand = () => setExpanded(true);
    node?.addEventListener('workspace-expand', expand);
    return () => node?.removeEventListener('workspace-expand', expand);
  }, []);
  return <details ref={details} className={`runtime-file-group ${required ? 'required' : 'optional'}`} open={expanded} onToggle={event => setExpanded(event.currentTarget.open)}><summary className="runtime-group-heading runtime-group-summary" data-outline-key={required ? "runtime-required" : "runtime-optional"} data-outline-label={required ? "必要运行文件" : "可选说明与附加文件"}><h3><span className="runtime-group-title">{required ? '必要运行文件' : '可选说明与附加文件'}</span><span className="runtime-group-count">共 {total} 项 · 已选 {selected} 项</span></h3></summary><div className="runtime-group-content"><p className="runtime-group-description">{description}</p>{children}</div></details>;
}
function FileSection({ sectionKey, title, count, description, initiallyOpen = false, hasIssues = false, children }: { sectionKey: string; title: string; count: ReactNode; description: string; initiallyOpen?: boolean; hasIssues?: boolean; children: ReactNode }) {
  const [expanded, setExpanded] = useState(initiallyOpen);
  const ref = useRef<HTMLDetailsElement>(null);
  useEffect(() => { if (hasIssues) setExpanded(true); }, [hasIssues]);
  useEffect(() => { const node = ref.current; const expand = () => setExpanded(true); node?.addEventListener('workspace-expand', expand); return () => node?.removeEventListener('workspace-expand', expand); }, []);
  return <details ref={ref} className="file-section" data-fold-key={sectionKey} open={expanded} onToggle={event => setExpanded(event.currentTarget.open)}><summary data-outline-key={sectionKey} data-outline-label={title} data-outline-level="1"><h3>{title}</h3><span>{count}</span></summary><div className="file-section-body"><p className="file-section-description">{description}</p>{children}</div></details>;
}
function ChangeSummary({ skill }: { skill: Skill }) {
  const changes = changedFiles(skill);
  if (!changes.length) return null;
  const paths = [...new Set(changes.map(file => file.path))];
  return <div className="change-file-summary" title={[skill.comparisonNote, ...changes.map(file => `${file.path} · ${CHANGES[file.change] || file.change}`)].filter(Boolean).join('\n')}><strong>{paths.length} 个差异文件</strong><span>{paths.slice(0, 2).join('、')}{paths.length > 2 ? ` 等 ${paths.length} 项` : ''}</span></div>;
}

export default function SessionApp() {
  const session = useWorkspaceSession();
  const sessionRef = useRef(session); sessionRef.current = session;
  const pane = session.pane;
  const isHome = pane.kind === 'home';
  const seed = <T,>(key: string, fallback: T): T => (session.initialDraft[key] ?? fallback) as T;
  const captureRef = useRef<() => SessionDraft>(() => ({}));
  const historyDraft = useRef<OperationHistoryDraft>(operationHistoryDraft(session.initialDraft.historyDraft));
  const exportFlushRef = useRef<(() => Promise<unknown>) | undefined>(undefined);
  const paneRoot = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const startedTasks = useRef(new Set<string>(seed('pendingTaskIds', [] as string[])));
  const registrationAcks = useRef(new Set<Promise<void>>());
  const taskForResult = useRef(new WeakMap<object, string>());
  const pendingTaskIntents = useRef(new Map<string, UiContinuation>(Object.entries(seed('pendingTaskIntents', {} as Record<string, UiContinuation>))));
  const deferredModal = useRef<{ value: Modal; outcomeTaskId?: string } | undefined>(seed('deferredModal', undefined));
  const inheritedTasks = useRef(seed('pendingTaskIds', [] as string[]));
  const [resumingIds, setResumingIds] = useState(inheritedTasks.current);
  const externalManagementDraft = useRef<ExternalManagementDraft | undefined>(seed('externalManagementDraft', undefined));
  const [resumedTasks, setResumedTasks] = useState<ManagerTask[]>([]);
  const [visited, setVisited] = useState(session.visible);
  useEffect(() => { if (session.visible) setVisited(true); }, [session.visible]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [state, setState] = useState<Snapshot>(EMPTY);
  const [view, setLocalView] = useState<View>(seed('view', (pane.state.view || 'skills') as View));
  function setView(next: View) { if (isHome) setLocalView(next); else sessionRef.current.goHome(next); }
  const [filter, setFilter] = useState<Filter>(seed('filter', 'all'));
  const [query, setQuery] = useState(seed('query', ''));
  const [libraryMode, setLibraryMode] = useState<LibraryMode>('list');
  const [librarySort, setLibrarySort] = useState<LibrarySort>('name');
  const [libraryPageSize, setLibraryPageSize] = useState<LibraryPageSize>(20);
  const [libraryPage, setLibraryPage] = useState(seed('libraryPage', 1));
  const [statusFilter, setStatusFilter] = useState<LibraryStatusFilter>(seed('statusFilter', 'all'));
  const [categoryFilter, setCategoryFilter] = useState(seed('categoryFilter', 'all'));
  const [categoryName, setCategoryName] = useState(seed('categoryName', ''));
  const [categoryDraftColor, setCategoryDraftColor] = useState<CategoryColor>(() => { const saved = seed('categoryDraftColor', undefined); return isCategoryColor(saved) ? saved : 'blue'; });
  const [categoryUndo, setCategoryUndo] = useState<{ skillId: string; categoryId: string | null }[] | null>(null);
  const [categoryFeedback, setCategoryFeedback] = useState<{ message: string; targetId?: string; error?: boolean } | null>(null);
  const [floatingMenu, setFloatingMenu] = useState<OpenMenu | null>(null);
  const [dragSkillIds, setDragSkillIds] = useState<string[]>([]);
  const [dragCategoryId, setDragCategoryId] = useState<string | null>(null);
  const [categoryOrderTarget, setCategoryOrderTarget] = useState<{ id: string; position: 'before' | 'after' } | null>(null);
  const [dropCategory, setDropCategory] = useState<string | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(224);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [sidebarFlyout, setSidebarFlyout] = useState(false);
  const [viewportWidth, setViewportWidth] = useState(window.innerWidth);
  const [paneWidth, setPaneWidth] = useState<number>();
  const [resizingSidebar, setResizingSidebar] = useState(false);
  const [zoomFeedback, setZoomFeedback] = useState('');
  const [layoutError, setLayoutError] = useState('');
  const sidebarRef = useRef<HTMLElement>(null);
  const dragCategoryTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const resizeStart = useRef<{ x: number; width: number; originalWidth: number } | null>(null);
  const currentLayout = useRef({ zoom: 100, width: 224, collapsed: false });
  const stateRef = useRef(state);
  stateRef.current = state;
  const [checkedSkills, setCheckedSkills] = useState<string[]>(seed('checkedSkills', []));
  const selectionAnchor = useRef<string | null>(null);
  const dragSuppressedUntil = useRef(0);
  const blockedDrag = useRef(false);
  const dragGhost = useRef<HTMLElement | null>(null);
  const tableRef = useRef<HTMLDivElement>(null);
  const [detailOpen, setDetailOpen] = useState(seed('detailOpen', pane.kind === 'detail'));
  const detailOpenedAt = useRef(0);
  const requestedDetailSkill = useRef<string | null>(null);
  const [selectedId, setSelectedId] = useState<string>(seed('selectedId', pane.skillId || ''));
  const [tab, setTab] = useState<'docs' | 'files' | 'source'>(seed('tab', (['docs', 'files', 'source'].includes(pane.state.section || '') ? pane.state.section : 'docs') as 'docs' | 'files' | 'source'));
  const [version, setVersion] = useState<'installed' | 'latest'>(seed('version', pane.state.documentVersion || 'latest'));
  const [documentPath, setDocumentPath] = useState<string | undefined>(seed('documentPath', pane.state.documentPath));
  const [documentCommit, setDocumentCommit] = useState<string | undefined>(seed('documentCommit', undefined));
  const [documentLocal, setDocumentLocal] = useState(seed('documentLocal', false));
  const [files, setFiles] = useState<FileResult | null>(seed('files', null));
  const [runtimeFiles, setRuntimeFiles] = useState<string[]>(seed('runtimeFiles', []));
  const [fileContent, setFileContent] = useState<{ path: string; content: string; source: 'local' | 'github'; remoteCommit?: string } | null>(seed('fileContent', null));
  const [fileError, setFileError] = useState('');
  const [inventoryFailed, setInventoryFailed] = useState(seed('inventoryFailed', false));
  const [fileBusy, setFileBusy] = useState(false);
  const [inventoryRefresh, setInventoryRefresh] = useState(0);
  const inventoryGeneration = useRef(0);
  const inventoryCache = useRef(new BoundedReadCache<FileResult>(16, 4 * 1024 * 1024, 2 * 1024 * 1024));
  const inventoryPending = useRef(new Map<string, Promise<FileResult>>());
  const autoMatchAttempts = useRef(new Set<string>());
  const autoMatchSequence = useRef(0);
  const [autoMatchFeedback, setAutoMatchFeedback] = useState(seed('autoMatchFeedback', ''));
  const storageDraft = useRef<StorageSettingsDraft | undefined>(seed('storageDraft', undefined));
  const networkDraft = useRef<NetworkSettingsDraft | undefined>(seed('networkDraft', undefined));
  const [settingsOutline, setSettingsOutline] = useState(seed('settingsOutline', false));
  const clientConnectDraft = useRef<ClientConnectDraft | undefined>(seed('clientConnectDraft', undefined));
  const [operation, setOperation] = useState('');
  const { tasks, run: runTask, inspect: inspectTask, cancel: cancelTask, clear: clearTasks } = session.tasks;
  const [pendingContexts, setPendingContexts] = useState<{ id: number; modal: Modal; metadata: boolean }[]>([]);
  const contextRef = useRef<{ view: View; skillId: string; tab: string; clientId: string; modal: Modal }>({ view: 'skills', skillId: '', tab: 'docs', clientId: 'codex', modal: null });
  const revisionRef = useRef(-1);
  const readGenerationWatermark = useRef<number | undefined>(undefined);
  const managementInFlight = useRef(0);
  const libraryPreferenceSequence = useRef(0);
  const running = useRef(false);
  const [ready, setReady] = useState(false);
  const [notices, setNotices] = useState<Notice[]>([]);
  const [modal, setLocalModal] = useState<Modal>(seed('modal', null));
  const modalRef = useRef(modal); modalRef.current = modal;
  const inFlightExportSelection = useRef<ExportSelectionDraft | undefined>(seed('inFlightExportSelection', modal?.kind === 'export' ? modal.inFlightSelection : undefined));
  function setModal(next: React.SetStateAction<Modal>) {
    if (typeof next === 'function') { setLocalModal(next); return; }
    const resultPayload = next && ('result' in next ? next.result : next.kind === 'preview' ? next.preview : next.kind === 'audit' ? next.summary : next.kind === 'receipts' ? next.receipts : next);
    const outcomeTaskId = resultPayload && typeof resultPayload === 'object' ? taskForResult.current.get(resultPayload) || (next ? taskForResult.current.get(next) : undefined) : undefined;
    if (sessionRef.current.isTransferring()) { deferredModal.current = { value: next, outcomeTaskId }; return; }
    if (!next) { if (pane.kind === 'operation') sessionRef.current.closePane(pane.id); else setLocalModal(null); return; }
    if (['category', 'delete-category', 'forget'].includes(next.kind)) { setLocalModal(next); return; }
    if (pane.kind === 'operation') {
      setLocalModal(next); sessionRef.current.updateNavigation(pane.id, { operation: next.kind, ...(next.kind === 'client-connect' ? { clientId: next.client } : {}) }); return;
    }
    const draft = { ...captureRef.current(), modal: next, ...(outcomeTaskId ? { outcomeTaskId } : {}) };
    if (next.kind === 'files') {
      const source = stateRef.current.sources.find(item => item.id === next.skill.sourceId) || stateRef.current.sources[0];
      Object.assign(draft, { selectedFiles: ['push', 'upload'].includes(next.operation) ? requiredExportPaths(next.result) : next.result.files.filter(file => file.runtime || requiredRuntimeFile(next.result, file.path) || next.result.suggestedFiles?.includes(file.path)).map(file => file.path), uploadSource: source?.id || '', uploadBranch: source?.branch || 'main', uploadPath: next.skill.remotePath || next.skill.name, trimOptional: true, targetPath: '', destinationChoice: destinationDraft(), runtimeConsent: next.result.runtimeConfirmed });
    } else if (next.kind === 'export') Object.assign(draft, { destinationChoice: destinationDraft() });
    else if (next.kind === 'runtime') Object.assign(draft, { selectedFiles: next.result.files.filter(file => file.runtime || requiredRuntimeFile(next.result, file.path) || !next.result.runtimeConfirmed && next.result.suggestedFiles?.includes(file.path)).map(file => file.path) });
    else if (next.kind === 'preview') Object.assign(draft, { previewFile: next.preview.files?.find(file => file.change !== 'unchanged')?.path || next.preview.files?.[0]?.path || '', commitMessage: '' });
    else if (next.kind === 'client-connect') Object.assign(draft, { activeClient: next.client, clientConnectDraft: undefined });
    else if (next.kind === 'associate') Object.assign(draft, { associateSource: stateRef.current.sources[0]?.id || '', associateSkill: '' });
    const skillId = 'skill' in next ? next.skill.id : next.kind === 'preview' ? next.preview.skillId : undefined;
    const titles: Record<string, string> = { 'client-result': '连接操作结果', 'client-connect': `连接到 ${CLIENTS.find(item => item.id === (next.kind === 'client-connect' ? next.client : ''))?.name || '客户端'}`, files: '上传／下载范围', runtime: '运行文件', export: '导出 Skill', preview: '差异与操作预览', receipts: '恢复附件', audit: '核对结果', task: '操作记录', local: '添加本地 Skill', github: '添加 GitHub 来源', associate: '关联来源' };
    const name = skillId ? stateRef.current.skills.find(item => item.id === skillId) : undefined;
    sessionRef.current.openOperation(next, draft, `${name ? skillName(name) + ' · ' : ''}${titles[next.kind] || next.kind}`, skillId);
  }
  const [modalError, setModalError] = useState('');
  useEffect(() => {
    if (pane.kind === 'operation' && pane.state.operation === 'client-connect' && !modal) {
      const client = CLIENTS.some(item => item.id === pane.state.clientId) ? pane.state.clientId! : 'codex';
      setLocalModal({ kind: 'client-connect', client });
    }
  }, [pane.kind, pane.state.operation, pane.state.clientId, modal]);
  const taskModalId = modal?.kind === 'task' ? modal.task.id : '';
  const taskModalStatus = tasks.find(item => item.id === taskModalId)?.status;
  useEffect(() => {
    if (pane.kind !== 'operation' || modal || pane.state.operation !== 'task' || !pane.state.taskId) return;
    let cancelled = false;
    void inspectTask(pane.state.taskId).then(task => { if (!cancelled) setLocalModal({ kind: 'task', task }); }).catch(error => { if (!cancelled) setModalError(errText(error)); });
    return () => { cancelled = true; };
  }, [pane.kind, pane.state.operation, pane.state.taskId, modal, inspectTask]);
  useEffect(() => {
    if (!taskModalId || !taskModalStatus || ['queued', 'running'].includes(taskModalStatus)) return;
    let cancelled = false;
    void inspectTask(taskModalId).then(task => { if (!cancelled) setModal(previous => previous?.kind === 'task' && previous.task.id === task.id ? { kind: 'task', task } : previous); }).catch(() => undefined);
    return () => { cancelled = true; };
  }, [taskModalId, taskModalStatus, inspectTask]);
  const [uploadSource, setUploadSource] = useState(seed('uploadSource', ''));
  const [uploadBranch, setUploadBranch] = useState(seed('uploadBranch', ''));
  const [uploadPath, setUploadPath] = useState(seed('uploadPath', ''));
  const [trimOptional, setTrimOptional] = useState(seed('trimOptional', true));
  const [transferResult, setTransferResult] = useState<TransferResult | null>(seed('transferResult', null));
  useEffect(() => {
    if (!transferResult) return;
    const frame = requestAnimationFrame(() => { if (sessionRef.current.active) paneRoot.current?.querySelector('.transfer-result')?.scrollIntoView({ block: 'nearest' }); });
    return () => cancelAnimationFrame(frame);
  }, [transferResult]);
  const modalOpen = useRef(false);
  modalOpen.current = !!modal;
  useEffect(() => { setModalError(''); }, [modal?.kind]);
  const [addMenu, setAddMenu] = useState(false);
  const [scanPath, setScanPath] = useState(seed('scanPath', ''));
  const [scanResult, setScanResult] = useState<{ skills: Skill[]; warnings: string[] } | null>(seed('scanResult', null));
  const [scanSelected, setScanSelected] = useState<string[]>(seed('scanSelected', []));
  const [githubUrl, setGithubUrl] = useState(seed('githubUrl', ''));
  const [associateSource, setAssociateSource] = useState(seed('associateSource', ''));
  const [associateSkill, setAssociateSkill] = useState(seed('associateSkill', ''));
  const [targetPath, setTargetPath] = useState(seed('targetPath', ''));
  const [destinationChoice, setDestinationChoice] = useState(() => destinationDraft(seed('destinationChoice', undefined)));
  const destinationChoiceRef = useRef(destinationChoice); destinationChoiceRef.current = destinationChoice;
  const [destinationDefaultPath, setDestinationDefaultPath] = useState('');
  const [destinationLoading, setDestinationLoading] = useState(false), [destinationError, setDestinationError] = useState('');
  const [destinationRefresh, setDestinationRefresh] = useState(0), [choosingDestination, setChoosingDestination] = useState(false);
  const [preparingDestination, setPreparingDestination] = useState(false);
  const destinationPreparing = useRef(false);
  const destinationPicker = useRef(0);
  const destinationKind = modal?.kind === 'export' ? 'export' : modal?.kind === 'files' && modal.operation === 'install' ? 'newSkill' : null;
  const destinationSkill = modal && 'skill' in modal ? modal.skill : null;
  const destinationKey = destinationKind && destinationSkill ? `${destinationKind}:${destinationSkill.id}:${destinationSkill.name}` : '';
  useEffect(() => {
    if (!destinationKind || !destinationSkill) return;
    let cancelled = false;
    setDestinationLoading(true); setDestinationDefaultPath(''); setDestinationError('');
    void request<{ path: string }>('storage_resolve_default_path', { kind: destinationKind }).then(value => {
      if (!cancelled) setDestinationDefaultPath(pathJoin(value.path, destinationKind === 'export' ? `${destinationSkill.name || 'skill'}.zip` : destinationSkill.name));
    }).catch(cause => { if (!cancelled) setDestinationError(`读取默认位置失败：${errText(cause)}`); }).finally(() => { if (!cancelled) setDestinationLoading(false); });
    return () => { cancelled = true; };
  }, [destinationKey, destinationRefresh]);
  function changeDestination(choice: DestinationDraft) {
    setDestinationChoice(choice);
    if (destinationKind === 'newSkill') setTargetPath(choice.path);
  }
  async function chooseDestination() {
    const context = modalRef.current;
    if (!context || !('skill' in context) || !destinationKind) return;
    const sequence = ++destinationPicker.current;
    setChoosingDestination(true);
    try {
      const initial = destinationChoice.mode === 'custom' ? destinationChoice.path : destinationDefaultPath || pathJoin(stateRef.current.dataDir || '', destinationKind === 'export' ? `${context.skill.name}.zip` : context.skill.name);
      const picked = destinationKind === 'export'
        ? await save({ title: '自定义 ZIP 保存位置（不覆盖已有文件）', defaultPath: initial, filters: [{ name: 'Skill ZIP 包', extensions: ['zip'] }] })
        : await open({ directory: true, multiple: false, defaultPath: initial, title: '选择父目录，将在其中创建 Skill 目录' });
      if (sequence !== destinationPicker.current || !mounted.current || modalRef.current !== context || !sessionRef.current.owned || sessionRef.current.isTransferring()) return;
      const path = typeof picked === 'string' ? destinationKind === 'export' ? picked : pathJoin(picked, context.skill.name) : null;
      changeDestination(destinationAfterPicker(destinationChoiceRef.current, path));
    } catch (cause) { if (mounted.current && sequence === destinationPicker.current) setDestinationError(errText(cause)); }
    finally { if (mounted.current && sequence === destinationPicker.current) setChoosingDestination(false); }
  }
  async function checkedDestination() {
    const choice = destinationChoiceRef.current;
    const context = modalRef.current;
    if (!destinationKind || !destinationSkill || !destinationReady(choice, destinationDefaultPath)) throw new Error('请选择本次使用默认位置或自定义位置。');
    if (choice.mode === 'default') {
      const current = await request<{ path: string }>('storage_resolve_default_path', { kind: destinationKind });
      assertDestinationCurrent(context, choice);
      const path = pathJoin(current.path, destinationKind === 'export' ? `${destinationSkill.name || 'skill'}.zip` : destinationSkill.name);
      setDestinationDefaultPath(path);
      if (path !== choice.path) throw new Error('默认保存位置已变化，请重新选择本次位置。');
    }
    assertDestinationCurrent(context, choice);
    return choice;
  }
  function assertDestinationCurrent(context: Modal, choice: DestinationDraft) {
    if (!mounted.current || !sessionRef.current.owned || sessionRef.current.closing || sessionRef.current.isTransferring() || modalRef.current !== context || destinationChoiceRef.current.mode !== choice.mode || destinationChoiceRef.current.path !== choice.path) throw new Error('操作页面或保存位置已变化，本次未执行。请核对后重试。');
  }
  const [selectedFiles, setSelectedFiles] = useState<string[]>(seed('selectedFiles', []));
  const [runtimeConsent, setRuntimeConsent] = useState(seed('runtimeConsent', false));
  const [commitMessage, setCommitMessage] = useState(seed('commitMessage', ''));
  const [previewFile, setPreviewFile] = useState(seed('previewFile', ''));
  const [activeClient, setActiveClient] = useState(seed('activeClient', pane.state.clientId || 'codex'));
  const [editorPath, setEditorPath] = useState('');
  const editorInitialized = useRef(false);
  const [uiTheme, setUiTheme] = useState<UiTheme>('light');
  const [uiZoom, setUiZoom] = useState(100);
  const [zoomInput, setZoomInput] = useState('100');
  const [uiFontSize, setUiFontSize] = useState(16);
  const [tokenRepo, setTokenRepo] = useState('');
  const [token, setToken] = useState('');
  const [tokenAccess, setTokenAccess] = useState<'read' | 'write'>('read');
  const [tokenLinkStatus, setTokenLinkStatus] = useState('');
  const fileRequestId = useRef(0);
  const selectedIdRef = useRef(selectedId);
  selectedIdRef.current = selectedId;

  contextRef.current = { view, skillId: detailOpen || libraryMode === 'details' ? selectedId : '', tab, clientId: activeClient, modal };
  const notify = useCallback((tone: Notice['tone'], message: string, skillId = contextRef.current.skillId) => { setNotices(previous => [{ id: Date.now() + Math.random(), tone, message, skillId }, ...previous].slice(0, 100)); }, []);
  useEffect(() => {
    if (!notices[0] || notices[0].tone === 'error') return;
    const id = notices[0].id;
    const timer = setTimeout(() => setNotices(previous => previous.filter(item => item.id !== id)), 5000);
    return () => clearTimeout(timer);
  }, [notices]);
  const closeFloatingMenu = useCallback((restoreFocus = false) => {
    setFloatingMenu(previous => { if (restoreFocus && previous?.anchor.isConnected) previous.anchor.focus(); return null; });
  }, []);
  const snapshot = useCallback((value: unknown) => {
    const next = value as Snapshot;
    if (next && Array.isArray(next.skills) && Array.isArray(next.sources) && Array.isArray(next.connections)) {
      if (isStaleReadSnapshot(next, revisionRef.current, readGenerationWatermark.current)) return;
      if (typeof next.readGeneration === 'number') readGenerationWatermark.current = next.readGeneration;
      if (next.stateRevision !== undefined) revisionRef.current = next.stateRevision;
      sessionRef.current.acceptSnapshot(next);
      const prior = stateRef.current.settings;
      setState({ ...next, settings: next.settings || {} });
      if (editorInitialized.current) {
        if (next.settings.uiTheme !== prior.uiTheme) setUiTheme(next.settings.uiTheme || 'light');
        if (next.settings.uiFontSize !== prior.uiFontSize) setUiFontSize(next.settings.uiFontSize || 16);
        if (next.settings.uiZoom !== prior.uiZoom) { const zoom = displayNumber(next.settings.uiZoom, 100, 80, 160); setUiZoom(zoom); setZoomInput(String(zoom)); }
      }
      for (const message of next.settings?.recoveryMessages || []) { if (!editorInitialized.current) notify('info', `启动恢复：${message}`); }
      if (!editorInitialized.current) {
        setEditorPath(next.settings?.editorPath || '');
        setUiTheme(THEMES.some(theme => theme.id === next.settings?.uiTheme) ? next.settings.uiTheme! : 'light');
        const zoom = displayNumber(next.settings?.uiZoom ?? 100, 100, 80, 160);
        setUiZoom(zoom); setZoomInput(String(zoom));
        setUiFontSize(displayNumber(next.settings?.uiFontSize ?? 16, 16, 14, 22));
        setLibraryMode(normalizeLibraryMode(next.settings?.libraryMode));
        setLibraryPageSize([20, 50, 100].includes(next.settings?.libraryPageSize || 0) ? next.settings.libraryPageSize! : 20);
        setLibrarySort(['name', 'status', 'checked'].includes(next.settings?.librarySort || '') ? next.settings.librarySort! : 'name');
        setSidebarWidth(displayNumber(next.settings?.sidebarWidth, 224, 200, 420));
        setSidebarCollapsed(next.settings?.sidebarCollapsed === true);
        editorInitialized.current = true;
      }
      setSelectedId(previous => pane.kind !== 'home' ? pane.skillId || previous : next.skills.some(skill => skill.id === previous) ? previous : next.skills[0]?.id || '');
    }
  }, [notify]);
  const { queue: queueLayout, flush: flushLayout } = usePreferenceQueue<Snapshot>({
    save: patch => request<Snapshot>('set_settings', { settings: patch }),
    isBusy: () => running.current,
    onSaved: (_patch, result) => { snapshot(result); setLayoutError(''); },
    onFailed: (patch, error) => {
      const saved = stateRef.current.settings;
      if ('uiZoom' in patch) { const restored = displayNumber(saved.uiZoom, 100, 80, 160); setUiZoom(restored); setZoomInput(String(restored)); }
      if ('sidebarWidth' in patch && !resizeStart.current) setSidebarWidth(displayNumber(saved.sidebarWidth, 224, 200, 420));
      if ('sidebarCollapsed' in patch) setSidebarCollapsed(saved.sidebarCollapsed === true);
      const message = `界面设置保存失败${resizeStart.current ? '，拖动结束后将重新保存宽度' : '，未保存的调整已恢复'}：${errText(error)}`;
      notify('error', message); setLayoutError(message);
    },
  });
  currentLayout.current = { zoom: uiZoom, width: sidebarWidth, collapsed: sidebarCollapsed };
  const effectiveWidth = paneWidth || viewportWidth / (uiZoom / 100);
  const compactSidebar = sidebarCollapsed || effectiveWidth < 720;
  const expandedSidebarWidth = Math.min(sidebarWidth, Math.max(200, effectiveWidth - 520));
  useEffect(() => { if (compactSidebar && resizeStart.current) cancelSidebarResize(); }, [compactSidebar]);
  useEffect(() => sessionRef.current.register(pane.id, { capture: () => captureRef.current(), flush: async () => { await Promise.all([...registrationAcks.current]); try { await exportFlushRef.current?.(); } catch (cause) { setLayoutError(errText(cause)); return false; } return flushLayout(); } }), [pane.id, flushLayout]);
  useEffect(() => { const resize = () => setViewportWidth(window.innerWidth); window.addEventListener('resize', resize); return () => window.removeEventListener('resize', resize); }, []);
  useEffect(() => {
    const node = paneRoot.current; if (!node || !isHome) return;
    const measure = () => { if (node.clientWidth > 0) setPaneWidth(node.clientWidth); };
    const observer = new ResizeObserver(measure); observer.observe(node); measure();
    return () => observer.disconnect();
  }, [isHome]);
  useEffect(() => {
    if (!zoomFeedback) return;
    const timer = window.setTimeout(() => setZoomFeedback(''), 1500);
    return () => window.clearTimeout(timer);
  }, [zoomFeedback, uiZoom]);
  useEffect(() => {
    const adjust = (value: number) => {
      if (!editorInitialized.current) return;
      if (!isHome || sessionRef.current.closing) return;
      const zoom = displayNumber(value, 100, 80, 160);
      currentLayout.current.zoom = zoom;
      setUiZoom(zoom); setZoomInput(String(zoom)); queueLayout({ uiZoom: zoom }); setZoomFeedback(`界面缩放 ${zoom}%`);
    };
    const wheel = (event: WheelEvent) => { if (!isHome || !event.ctrlKey || event.defaultPrevented || sessionRef.current.closing) return; event.preventDefault(); if (event.deltaY) adjust(currentLayout.current.zoom + (event.deltaY < 0 ? 5 : -5)); };
    const keys = (event: KeyboardEvent) => {
      if (!isHome || !event.ctrlKey || event.altKey || event.metaKey || event.defaultPrevented || sessionRef.current.closing) return;
      if (event.key === '0' || event.code === 'Numpad0') { event.preventDefault(); adjust(100); }
      else if (['+', '=', '-'].includes(event.key)) { event.preventDefault(); adjust(currentLayout.current.zoom + (event.key === '-' ? -5 : 5)); }
    };
    window.addEventListener('wheel', wheel, { passive: false, capture: true });
    window.addEventListener('keydown', keys, true);
    return () => { window.removeEventListener('wheel', wheel, true); window.removeEventListener('keydown', keys, true); };
  }, [queueLayout]);
  useEffect(() => {
    if (!sidebarFlyout) return;
    const dismiss = (event: PointerEvent) => { if (!(event.target as Element).closest('.floating-menu') && !sidebarRef.current?.contains(event.target as Node)) setSidebarFlyout(false); };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') setSidebarFlyout(false); };
    document.addEventListener('pointerdown', dismiss); document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', dismiss); document.removeEventListener('keydown', escape); };
  }, [sidebarFlyout]);
  useEffect(() => () => clearTimeout(dragCategoryTimer.current), []);
  useEffect(() => { closeFloatingMenu(); }, [view, categoryFilter, modal, uiZoom, session.active, closeFloatingMenu]);
  useEffect(() => () => dragGhost.current?.remove(), []);
  useEffect(() => {
    if (!isHome) return;
    const root = document.documentElement;
    root.dataset.theme = uiTheme;
    root.style.fontSize = `${uiFontSize}px`;
    root.style.setProperty('--ui-zoom', String(uiZoom / 100));
    root.style.setProperty('zoom', String(uiZoom / 100));
  }, [uiTheme, uiZoom, uiFontSize]);
  const perform = useCallback(async <T,>(label: string, action: string, args: Record<string, unknown> = {}, success?: string, onFailure?: (message: string) => void, foreground = true, continuation?: UiContinuation): Promise<T | undefined> => {
    const origin = { ...contextRef.current };
    const uiContinuation = continuation || (action.startsWith('preview_') ? { kind: 'preview' as const } : action === 'audit_skills' ? { kind: 'audit' as const } : action === 'scan_local' ? { kind: 'scan' as const } : action === 'get_cleanup_receipts' ? { kind: 'receipts' as const } : undefined);
    let acknowledgeRegistration = () => {};
    let registeredTaskId: string | undefined;
    const registration = new Promise<void>(resolve => { acknowledgeRegistration = resolve; });
    registrationAcks.current.add(registration);
    const id = Date.now() + Math.random();
    const metadata = ['assign_category', 'restore_category_assignments', 'create_category', 'rename_category', 'delete_category', 'reorder_categories', 'set_category_color', 'set_favorite', 'undo_management'].includes(action);
    if (metadata) { managementInFlight.current += 1; running.current = true; }
    setPendingContexts(previous => [...previous, { id, modal: origin.modal, metadata }]); setOperation(label); if (foreground && origin.modal) setModalError('');
    const current = () => mounted.current && contextRef.current.modal === origin.modal;
    try {
      const value = await runTask<T>(label, action, args, { view: origin.view, skillId: typeof args.skillId === 'string' ? args.skillId : origin.skillId || undefined, tab: origin.tab, clientId: typeof args.client === 'string' ? args.client : origin.view === 'clients' ? origin.clientId : undefined, paneId: pane.id }, foreground, task => { registeredTaskId = task.id; startedTasks.current.add(task.id); if (uiContinuation) pendingTaskIntents.current.set(task.id, { ...uiContinuation, skillId: typeof args.skillId === 'string' ? args.skillId : origin.skillId || undefined }); acknowledgeRegistration(); registrationAcks.current.delete(registration); });
      if (registeredTaskId && value && typeof value === 'object') { taskForResult.current.set(value, registeredTaskId); const children = value as { auditSummary?: object; receipts?: object }; if (children.auditSummary) taskForResult.current.set(children.auditSummary, registeredTaskId); if (children.receipts) taskForResult.current.set(children.receipts, registeredTaskId); }
      snapshot(value); const nested = value as { stateSnapshot?: Snapshot }; if (nested?.stateSnapshot) snapshot(nested.stateSnapshot);
      if (['set_token', 'delete_token'].includes(action)) { try { await emit('read-context-changed', {}); } catch { /* Local snapshot and the task result remain authoritative. */ } }
      if (success || action !== 'refresh') notify('success', success || `${label.replace(/^正在/, '')}已完成。`, typeof args.skillId === 'string' ? args.skillId : origin.skillId);
      return current() ? value : undefined;
    } catch (error) { if (['set_token', 'delete_token'].includes(action)) { try { snapshot(await request<Snapshot>('get_state')); await emit('read-context-changed', {}); } catch { /* The original error remains visible. */ } } const message = `${label}：${errText(error)}`; notify('error', message, origin.skillId); if (current()) { if (origin.modal) setModalError(message); onFailure?.(message); } return undefined; }
    finally { acknowledgeRegistration(); registrationAcks.current.delete(registration); if (registeredTaskId) requestAnimationFrame(() => { if (!sessionRef.current.isTransferring()) { startedTasks.current.delete(registeredTaskId!); pendingTaskIntents.current.delete(registeredTaskId!); } }); if (metadata) { managementInFlight.current -= 1; running.current = managementInFlight.current > 0; } setPendingContexts(previous => previous.filter(item => item.id !== id)); setOperation(''); }
  }, [notify, runTask, snapshot]);
  const refreshReadContext = useCallback(async (reply?: unknown) => {
    const candidate = reply as Partial<Snapshot> | undefined;
    const latest = candidate?.skills && candidate?.sources && candidate?.connections ? candidate : await request<Snapshot>('get_state');
    snapshot(latest);
    try { await emit('read-context-changed', {}); }
    catch { notify('info', '设置已保存；其他窗口可重新获得焦点以刷新读取状态。'); }
  }, [snapshot, notify]);
  const refresh = useCallback(async (quiet = false) => { const result = await perform<Snapshot>('正在检查本地与远端变化', 'refresh', {}, undefined, undefined, !quiet); if (!result) return; const failed = result.skills.filter(skill => skill.error || skill.remoteCheckRequired || skill.remoteMissing).length; const failedSources = result.sources.filter(source => source.error).length; if (!quiet || failed || failedSources) notify(failed || failedSources ? 'info' : 'success', failed || failedSources ? `检查返回，${failed} 个 Skill、${failedSources} 个来源需要核对异常。` : '检查已完成；请按列表状态决定下一步。'); }, [perform, notify]);
  const undoManagement = useCallback(async () => {
    if (!stateRef.current.managementUndo?.available || running.current || modalOpen.current) return;
    const label = stateRef.current.managementUndo.label || '管理操作';
    const result = await perform<Snapshot>('正在撤销管理操作', 'undo_management', {}, `已撤销${label}。`);
    if (result) { setCategoryUndo(null); setCategoryFeedback(null); }
  }, [perform]);
  useEffect(() => {
    const undo = (event: KeyboardEvent) => {
      if (!sessionRef.current.active || !event.ctrlKey || event.shiftKey || event.altKey || event.metaKey || event.key.toLowerCase() !== 'z' || event.defaultPrevented || running.current || modalOpen.current) return;
      const target = event.target instanceof Element ? event.target : document.activeElement;
      if (target?.closest('input,textarea,select,[contenteditable]:not([contenteditable="false"]),[role="textbox"]') || !stateRef.current.managementUndo?.available) return;
      event.preventDefault(); void undoManagement();
    };
    window.addEventListener('keydown', undo);
    return () => window.removeEventListener('keydown', undo);
  }, [undoManagement]);
  useEffect(() => {
    if (session.managerSnapshot) { snapshot(session.managerSnapshot); setReady(true); }
  }, [session.managerSnapshot, snapshot]);
  useEffect(() => {
    if (pane.kind === 'home' && pane.state.view) setLocalView(pane.state.view as View);
    if (pane.kind === 'home' && pane.state.clientId) setActiveClient(pane.state.clientId);
    if (pane.kind === 'detail' && ['docs', 'files', 'source'].includes(pane.state.section || '')) setTab(pane.state.section as 'docs' | 'files' | 'source');
  }, [pane.kind, pane.state.view, pane.state.clientId, pane.state.section]);
  const selected = state.skills.find(skill => skill.id === selectedId);
  const localSkills = state.skills.filter(skill => skill.localPath);
  const libraryTotals = libraryCounts(state.skills, state.sources);
  const changedSkills = state.skills.filter(needsAttention);
  const remoteErrors = state.sources.filter(source => source.error);
  const categories = state.settings.categories || [];
  const resolvedCategoryColors = useMemo(() => categoryColors(categories), [categories]);
  const categoryTitle = categoryFilter === 'all' ? '全部 Skills' : categoryFilter === 'favorites' ? '收藏' : categoryFilter === 'uncategorized' ? '未分类' : categories.find(category => category.id === categoryFilter)?.name || '全部 Skills';
  const categoryTotal = state.skills.filter(skill => categoryFilter === 'all' || (categoryFilter === 'favorites' ? skill.favorite : categoryFilter === 'uncategorized' ? !skill.categoryId : skill.categoryId === categoryFilter)).length;
  const matching = useMemo(() => queryLibrary(state.skills, { query, sourceFilter: filter, statusFilter, sort: librarySort, categoryFilter }), [state.skills, query, filter, statusFilter, librarySort, categoryFilter]);
  const libraryResult = useMemo(() => paginateLibrary(matching, libraryPage, libraryPageSize), [matching, libraryPage, libraryPageSize]);
  const pageSkillIds = useMemo(() => libraryResult.items.map(skill => skill.id), [libraryResult.items]);
  const visibleChecked = checkedSkills.filter(id => pageSkillIds.includes(id));
  const detailVisible = pane.kind === 'detail' ? visited : isHome && view === 'skills' && libraryMode === 'details' && pageSkillIds.includes(selectedId);
  const libraryQueryKey = JSON.stringify([query, filter, statusFilter, librarySort, libraryPageSize, categoryFilter]);
  const previousLibraryQuery = useRef(libraryQueryKey);
  useEffect(() => {
    const filtersChanged = previousLibraryQuery.current !== libraryQueryKey;
    previousLibraryQuery.current = libraryQueryKey;
    const requestedIndex = libraryMode === 'details' && view === 'skills' && requestedDetailSkill.current ? matching.findIndex(skill => skill.id === requestedDetailSkill.current) : -1;
    requestedDetailSkill.current = null;
    const nextPage = requestedIndex >= 0 ? Math.floor(requestedIndex / libraryPageSize) + 1 : filtersChanged ? 1 : libraryResult.page;
    if (filtersChanged) setCheckedSkills([]);
    if (libraryPage !== nextPage) setLibraryPage(nextPage);
    if (libraryMode === 'details' && view === 'skills') {
      const pageItems = paginateLibrary(matching, nextPage, libraryPageSize).items;
      const target = requestedIndex >= 0 ? matching[requestedIndex] : pageItems.find(skill => skill.id === selectedId) || pageItems[0];
      if (target && target.id !== selectedId) { setSelectedId(target.id); setTab('docs'); }
    }
  }, [libraryQueryKey, libraryPage, libraryPageSize, libraryResult.page, matching, selectedId, libraryMode, view]);
  useEffect(() => { setCheckedSkills([]); selectionAnchor.current = null; closeFloatingMenu(); }, [libraryPage, libraryQueryKey, closeFloatingMenu]);
  const exportSkillContext = modal?.kind === 'export' ? state.skills.find(item => item.id === modal.skill.id) || modal.skill : detailVisible ? selected : undefined;
  const exportSelection = useExportSelection({ skillId: exportSkillContext?.id, paneId: pane.id, enabled: !!exportSkillContext?.localPath && session.owned, visible: session.visible });
  exportFlushRef.current = exportSkillContext?.localPath && session.owned ? exportSelection.flush : undefined;
  const exportPlanState = useExportPlan({ skillId: exportSkillContext?.id, selectedIds: exportSelection.selectedIds, enabled: !!exportSkillContext?.localPath && !!exportSelection.draft && session.owned && (modal?.kind === 'export' || detailVisible && tab === 'files'), cacheable: state.readGeneration != null, contentKey: exportSkillContext ? `${state.readGeneration}:${inventoryStamp(exportSkillContext)}:${inventoryRefresh}` : '', remoteCommit: selected?.id === exportSkillContext?.id ? files?.remoteCommit : undefined });
  const exportPlan = exportPlanState.plan;
  const exportResources = (exportPlan?.resources || []).map(resource => !['required', 'generated'].includes(resource.group) ? { ...resource, included: exportSelection.selectedIds.includes(resource.id) } : resource);
  const extraFiles = exportSelection.selectedIds.filter(id => id.startsWith('file:')).map(id => id.slice(5));
  const selectedExternalIds = exportResources.filter(resource => resource.group === 'optional_external' && resource.included).flatMap(resource => resource.referenceIds);
  function toggleExport(id: string, included: boolean) { void exportSelection.toggle(id, included).catch(cause => notify('error', errText(cause))); }
  function setExportChoices(ids: string[]) { void exportSelection.set(ids).catch(cause => notify('error', errText(cause))); }
  async function recoverExport() {
    try { await exportSelection.refresh(); exportPlanState.refresh(); }
    catch (cause) { notify('error', bridgeMessage(cause)); }
  }
  const previousSelection = useRef(selectedId);
  useEffect(() => { if (previousSelection.current === selectedId) return; previousSelection.current = selectedId; setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); setVersion('latest'); setFileContent(null); setFiles(null); }, [selectedId]);
  useEffect(() => { if (isHome && !detailVisible) { setFileContent(null); fileRequestId.current += 1; } }, [detailVisible, isHome]);
  const selectedContentStamp = selected ? inventoryStamp(selected) : '';
  const selectedContentStampRef = useRef(selectedContentStamp); selectedContentStampRef.current = selectedContentStamp;
  const docKey = documentCacheKey({ readGeneration: state.readGeneration, skillId: selected?.id || '', sourceId: selected?.sourceId,
    repo: selected?.repo, branch: selected?.branch, remotePath: selected?.remotePath,
    commit: documentCommit || (version === 'installed' ? selected?.installedCommit : selected?.commit), version, path: documentPath,
    origin: documentLocal || !selected?.sourceId ? 'local' : 'remote', localPath: selected?.localPath, localFingerprint: selected?.localHashes });
  const documentState = useDocumentCache<DocumentResult>({ cacheKey: docKey, enabled: !!selected && detailVisible && tab === 'docs' && session.owned,
    cacheable: state.readGeneration != null, initialValue: seed('documentCacheKey', '') === docKey ? seed<DocumentResult | undefined>('documentValue', undefined) : undefined,
    load: () => request<DocumentResult>('get_document', { skillId: selected!.id, version, ...(documentPath ? { path: documentPath } : {}), ...(documentCommit ? { remoteCommit: documentCommit } : {}), ...(documentLocal ? { origin: 'local' } : {}) }) });
  const doc = documentState.value, docBusy = documentState.loading, docError = documentState.error;
  const selectedInventoryStamp = inventoryReadKey(selected?.id || '', state.readGeneration, selectedContentStamp);
  useEffect(() => {
    if (!selected || !detailVisible || tab !== 'files') return;
    let cancelled = false;
    const skill = selected;
    const stamp = selectedInventoryStamp;
    const generation = ++inventoryGeneration.current;
    const cached = inventoryCache.current.get(stamp);
    function accept(value: FileResult) {
      if (cancelled || generation !== inventoryGeneration.current) return;
      setFiles(value);
      setInventoryFailed(false);
      setRuntimeFiles(requiredExportPaths(value));
    }
    setFileError('');
    if (cached) { accept(cached); setFileBusy(false); return; }
    setFileBusy(true);
    void (async () => {
      if (skill.localPath || !skill.sourceId) {
        try {
          const local = await request<FileResult>('get_files', { skillId: skill.id });
          accept({ ...local, ...(!skill.sourceId ? { remoteStatus: 'not_linked' as const } : {}) });
          if (!skill.sourceId && generation === inventoryGeneration.current) inventoryCache.current.set(stamp, { ...local, remoteStatus: 'not_linked' });
        } catch (error) { if (!cancelled && generation === inventoryGeneration.current) { setInventoryFailed(true); setFileError(`本地清单读取失败：${errText(error)}`); } }
      }
      if (skill.sourceId) {
        let pending = inventoryPending.current.get(stamp);
        if (!pending) { pending = request<FileResult>('get_files', { skillId: skill.id, includeRemote: true }); inventoryPending.current.set(stamp, pending); }
        try {
          const merged = await pending;
          if (inventoryPending.current.get(stamp) === pending) inventoryCache.current.set(stamp, merged);
          accept(merged);
        } catch (error) {
          if (!cancelled && generation === inventoryGeneration.current) setFiles(previous => previous ? { ...previous, remoteStatus: 'error', remoteError: errText(error) } : { files: [], runtimeConfirmed: !!skill.runtimeConfirmed, remoteStatus: 'error', remoteError: errText(error) });
        } finally { if (inventoryPending.current.get(stamp) === pending) inventoryPending.current.delete(stamp); }
      }
      if (!cancelled && generation === inventoryGeneration.current) setFileBusy(false);
    })();
    return () => { cancelled = true; };
  }, [selectedInventoryStamp, detailVisible, tab, inventoryRefresh]);
  useEffect(() => {
    if (selected && files?.remoteStatus === 'ok' && files.remoteCommit && tab === 'files' && detailVisible && session.owned) void autoMatchExternal(selected, files);
  }, [files, selectedId, tab, detailVisible, session.owned]);

  const closeDetail = useCallback(() => { if (sessionRef.current.pane.kind !== 'home') sessionRef.current.closePane(sessionRef.current.pane.id); else { setDetailOpen(false); setFileContent(null); fileRequestId.current += 1; } }, []);
  function openDetail(skill: Skill, section: 'docs' | 'files' | 'source' = 'docs') {
    if (Date.now() < dragSuppressedUntil.current) return;
    sessionRef.current.openDetail(skill, section);
  }
  function clearLibraryFilters() { setQuery(''); setFilter('all'); setStatusFilter('all'); }
  function selectCategory(id: string) { closeFloatingMenu(); setCategoryFilter(id); setLibraryPage(1); setCheckedSkills([]); setView('skills'); setSidebarFlyout(false); }
  function editCategory(category?: Category) { setCategoryName(category?.name || ''); setCategoryDraftColor(category ? resolvedCategoryColors.get(category.id) || 'blue' : nextCategoryColor(resolvedCategoryColors.values())); setModal({ kind: 'category', category }); }
  async function saveCategory() {
    if (modal?.kind !== 'category') return;
    const action = !modal.category ? 'create_category' : categoryName.trim() === modal.category.name ? 'set_category_color' : 'rename_category';
    const result = await perform<Snapshot>(modal.category ? '正在保存分类' : '正在创建分类', action, { ...(modal.category ? { categoryId: modal.category.id } : {}), name: categoryName.trim(), color: categoryDraftColor });
    if (result) setModal(null);
  }
  async function removeCategory() {
    if (modal?.kind !== 'delete-category') return;
    const id = modal.category.id;
    const result = await perform<Snapshot>('正在移除分类', 'delete_category', { categoryId: id }, '分类已移除，其中的 Skill 已转为未分类；原文件保持原位置。');
    if (result) { setModal(null); setCategoryUndo(null); if (categoryFilter === id) selectCategory('uncategorized'); }
  }
  async function moveCategory(id: string, offset: number) {
    const ids = categories.map(category => category.id); const index = ids.indexOf(id); const target = index + offset;
    if (index < 0 || target < 0 || target >= ids.length) return;
    [ids[index], ids[target]] = [ids[target], ids[index]];
    await perform('正在调整分类顺序', 'reorder_categories', { categoryIds: ids });
  }
  async function assignCategory(skillIds: string[], categoryId: string | null) {
    if (running.current) return;
    const assignments = state.skills.filter(skill => skillIds.includes(skill.id) && (skill.categoryId || null) !== categoryId).map(skill => ({ skillId: skill.id, categoryId: skill.categoryId || null }));
    if (!assignments.length) { setCategoryFeedback({ message: '所选 Skill 已在此分类，归属保持不变。' }); return; }
    const message = `已将 ${assignments.length} 项移入“${categories.find(item => item.id === categoryId)?.name || '未分类'}”。`;
    const result = await perform<Snapshot>('正在归类 Skill', 'assign_category', { skillIds: assignments.map(item => item.skillId), categoryId }, message, error => setCategoryFeedback({ message: `${error}；勾选和当前分类已保留，可重试。`, error: true }));
    if (result) { setCategoryUndo(assignments); setCheckedSkills([]); setCategoryFeedback({ message, targetId: categoryId || 'uncategorized' }); }
  }
  async function undoCategory() {
    if (!categoryUndo) return;
    const result = await perform<Snapshot>('正在撤销归类', 'restore_category_assignments', { assignments: categoryUndo }, '已撤销最近一次归类。');
    if (result) { setCategoryUndo(null); setCategoryFeedback({ message: '已撤销最近一次归类。' }); }
  }
  function startSkillDrag(event: React.DragEvent, skill: Skill) {
    if (running.current || blockedDrag.current && !(event.currentTarget as Element).classList.contains('skill-drag-handle')) { event.preventDefault(); return; }
    event.stopPropagation(); closeFloatingMenu();
    const ids = selectionForDrag(skill.id, visibleChecked, pageSkillIds);
    event.dataTransfer.setData('application/x-skill-manager-skills', JSON.stringify(ids));
    event.dataTransfer.setData('text/plain', `归类 ${ids.length} 个 Skill`);
    event.dataTransfer.effectAllowed = 'move'; setDragSkillIds(ids); dragSuppressedUntil.current = Date.now() + 60000;
    dragGhost.current?.remove();
    const ghost = document.createElement('div'); ghost.className = 'skill-drag-ghost';
    const count = document.createElement('strong'); count.textContent = `移入分类 · ${ids.length} 项`;
    const names = document.createElement('span'); names.className = 'drag-ghost-names'; names.textContent = ids.slice(0, 2).map(id => skillName(state.skills.find(item => item.id === id)!)).join('、') + (ids.length > 2 ? '…' : '');
    ghost.append(count, names); ghost.style.cssText = 'position:fixed;left:-10000px;top:0;pointer-events:none;'; document.body.append(ghost); dragGhost.current = ghost;
    event.dataTransfer.setDragImage(ghost, 12, 12);
  }
  function endSkillDrag() { dragSuppressedUntil.current = Date.now() + 350; dragGhost.current?.remove(); dragGhost.current = null; setDragSkillIds([]); setDropCategory(null); setSidebarFlyout(false); clearTimeout(dragCategoryTimer.current); }
  function dragPointer(event: React.PointerEvent) {
    const control = (event.target as Element).closest('button,input,select,a,textarea,[role="menuitem"]');
    blockedDrag.current = !!control && !control.classList.contains('skill-name-button') && !control.classList.contains('skill-drag-handle');
  }
  function startCategoryDrag(event: React.DragEvent, category: Category) {
    if (running.current) { event.preventDefault(); return; }
    event.stopPropagation(); closeFloatingMenu(); setDragCategoryId(category.id);
    event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('application/x-skill-manager-category', category.id);
  }
  function endCategoryDrag() { setDragCategoryId(null); setCategoryOrderTarget(null); }
  function overCategoryOrder(event: React.DragEvent, id: string) {
    if (running.current || id === dragCategoryId || !event.dataTransfer.types.includes('application/x-skill-manager-category')) return;
    event.preventDefault(); event.dataTransfer.dropEffect = 'move';
    const rect = event.currentTarget.getBoundingClientRect();
    setCategoryOrderTarget({ id, position: event.clientY < rect.top + rect.height / 2 ? 'before' : 'after' });
  }
  async function dropCategoryOrder(event: React.DragEvent, id: string) {
    if (!event.dataTransfer.types.includes('application/x-skill-manager-category')) return;
    event.preventDefault();
    const source = event.dataTransfer.getData('application/x-skill-manager-category');
    const rect = event.currentTarget.getBoundingClientRect(); const after = event.clientY >= rect.top + rect.height / 2;
    const ids = categories.map(item => item.id).filter(item => item !== source);
    const target = ids.indexOf(id);
    endCategoryDrag();
    if (!categories.some(item => item.id === source) || source === id || target < 0) return;
    ids.splice(target + (after ? 1 : 0), 0, source);
    await perform('正在调整分类顺序', 'reorder_categories', { categoryIds: ids });
  }
  async function toggleFavorite(skill: Skill) {
    await perform('正在保存收藏', 'set_favorite', { skillId: skill.id, favorite: !skill.favorite }, skill.favorite ? '已取消收藏，原分类保持不变。' : '已收藏，可从左侧“收藏”快速访问。');
  }
  function chooseRow(skillId: string, value: boolean, range = false) {
    setCheckedSkills(previous => range ? rangeSelection(pageSkillIds, previous, selectionAnchor.current, skillId, value) : value ? [...new Set([...previous.filter(id => pageSkillIds.includes(id)), skillId])] : previous.filter(id => id !== skillId));
    if (!range || !selectionAnchor.current) selectionAnchor.current = skillId;
  }
  function tableKeys(event: React.KeyboardEvent<HTMLDivElement>) {
    if (modal || detailOpen || running.current || (event.target as Element).closest('input,button,a,select,textarea,[contenteditable="true"]')) return;
    const row = (event.target as Element).closest<HTMLTableRowElement>('tr[data-skill-id]');
    const id = row?.dataset.skillId; const index = id ? pageSkillIds.indexOf(id) : -1;
    if (event.ctrlKey && !event.altKey && !event.metaKey && event.key.toLowerCase() === 'a') { event.preventDefault(); setCheckedSkills(pageSkillIds); selectionAnchor.current ||= pageSkillIds[0] || null; }
    else if (event.key === ' ' && id) { event.preventDefault(); chooseRow(id, !visibleChecked.includes(id), event.shiftKey); }
    else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key) && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? pageSkillIds.length - 1 : Math.max(0, Math.min(pageSkillIds.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)));
      const nextId = pageSkillIds[next]; if (!nextId) return;
      if (event.shiftKey) { selectionAnchor.current ||= id || nextId; chooseRow(nextId, true, true); }
      setSelectedId(nextId); tableRef.current?.querySelector<HTMLElement>(`tr[data-skill-id="${CSS.escape(nextId)}"]`)?.focus();
    }
  }
  function overCategory(event: React.DragEvent, id: string) {
    if (running.current || !event.dataTransfer.types.includes('application/x-skill-manager-skills')) return;
    event.preventDefault(); event.dataTransfer.dropEffect = 'move'; setDropCategory(id);
  }
  function dropIntoCategory(event: React.DragEvent, id: string) {
    if (!event.dataTransfer.types.includes('application/x-skill-manager-skills')) return;
    event.preventDefault();
    try {
      const ids: unknown = JSON.parse(event.dataTransfer.getData('application/x-skill-manager-skills'));
      if (Array.isArray(ids) && ids.every(value => typeof value === 'string')) void assignCategory(ids.filter(value => state.skills.some(skill => skill.id === value)), id === 'uncategorized' ? null : id);
    } catch { notify('error', '无法读取本次拖动，请使用“归入分类”菜单。'); }
    endSkillDrag();
  }
  function toggleSidebar() { const collapsed = !sidebarCollapsed; setSidebarCollapsed(collapsed); setSidebarFlyout(false); queueLayout({ sidebarCollapsed: collapsed }); }
  function resizeSidebar(width: number, persist = false) {
    const value = displayNumber(width, 224, 200, Math.min(420, Math.max(200, window.innerWidth / (currentLayout.current.zoom / 100) - 520)));
    currentLayout.current.width = value; setSidebarWidth(value); if (persist) queueLayout({ sidebarWidth: value });
  }
  function cancelSidebarResize() {
    const original = resizeStart.current?.originalWidth;
    resizeStart.current = null; setResizingSidebar(false);
    if (original !== undefined) { currentLayout.current.width = original; setSidebarWidth(original); queueLayout({ sidebarWidth: original }); void flushLayout(); }
  }
  function resetLayout() { setSidebarWidth(224); setSidebarCollapsed(false); setSidebarFlyout(false); queueLayout({ sidebarWidth: 224, sidebarCollapsed: false }); }
  async function saveLibraryPreference(settings: Partial<{ libraryMode: LibraryMode; librarySort: LibrarySort; libraryPageSize: LibraryPageSize }>) {
    const sequence = ++libraryPreferenceSequence.current;
    const previous = { libraryMode, librarySort, libraryPageSize };
    if (settings.libraryMode) { setLibraryMode(settings.libraryMode); setDetailOpen(false); }
    if (settings.librarySort) setLibrarySort(settings.librarySort);
    if (settings.libraryPageSize) setLibraryPageSize(settings.libraryPageSize);
    try { const result = await runTask<Snapshot>('正在保存列表偏好', 'set_settings', { settings }, { view: 'skills' }); snapshot(result); }
    catch (error) { if (sequence === libraryPreferenceSequence.current) { setLibraryMode(previous.libraryMode); setLibrarySort(previous.librarySort); setLibraryPageSize(previous.libraryPageSize); } notify('error', `列表偏好保存失败：${errText(error)}`); }
  }
  async function checkSelected() {
    const skillIds = checkedSkills.filter(id => pageSkillIds.includes(id));
    if (!skillIds.length) return;
    const result = await perform<Snapshot>('正在检查所选 Skill', 'refresh', { skillIds });
    if (!result) return;
    const failed = skillIds.filter(id => { const item = result.skills.find(skill => skill.id === id); return !item || item.error || item.remoteMissing || item.remoteCheckRequired || result.sources.find(source => source.id === item.sourceId)?.error; }).length;
    notify(failed ? 'info' : 'success', failed ? `所选 ${skillIds.length} 项中，${skillIds.length - failed} 项已核对，${failed} 项检查未完成。请查看对应行的异常；更新仍需逐项预览。` : `已核对所选 ${skillIds.length} 项。请根据状态决定下一步，更新仍需逐项预览。`);
  }

  const closeModal = useCallback(() => { setModal(null); }, []);
  async function chooseLocal() { try { const chosen = await open({ directory: true, multiple: false, title: '选择一个 Skill 文件夹或包含多个 Skill 的目录' }); if (typeof chosen === 'string') { setScanPath(chosen); setScanResult(null); setScanSelected([]); } } catch (error) { notify('error', errText(error)); } }
  async function scanLocal() { const result = await perform<{ skills: Skill[]; warnings: string[] }>('正在扫描 Skill 目录', 'scan_local', { path: scanPath }); if (result) { setScanResult(result); setScanSelected(result.skills.filter(skill => !skill.readOnly).map(skill => skill.localPath || skill.path || '').filter(Boolean)); } }
  async function addLocal() { const result = await perform<Snapshot>('正在登记本地 Skill', 'add_local', { paths: scanSelected }, '已登记原有目录，Skill 文件仍保留在原位置。'); if (result) setModal(null); }
  async function addGithub() { const result = await perform<Snapshot>('正在读取 GitHub 来源', 'add_github', { url: githubUrl }, 'GitHub 来源已添加。'); if (result) { setGithubUrl(''); setModal(null); } }
  async function selectOperation(skill: Skill, mode: 'install' | 'push' | 'upload') {
    const result = await perform<FileResult>('正在读取文件范围', 'get_files', { skillId: skill.id }, undefined, undefined, true, { kind: 'files', operation: mode }); if (!result) return;
    if (mode === 'push') for (const path of Object.keys(skill.baselineHashes || {})) { if (!result.files.some(file => file.path === path)) result.files.push({ path, runtime: true, missing: true }); }
    setSelectedFiles(mode === 'push' || mode === 'upload' ? requiredExportPaths(result) : result.files.filter(file => file.runtime || requiredFile(file.path) || result.requiredFiles?.includes(file.path) || result.suggestedFiles?.includes(file.path)).map(file => file.path));
    const source = state.sources.find(item => item.id === skill.sourceId) || state.sources[0];
    setUploadSource(source?.id || ''); setUploadBranch(source?.branch || 'main'); setUploadPath(skill.remotePath || skill.name); setTrimOptional(true);
    setTargetPath(''); setDestinationChoice(destinationDraft()); setRuntimeConsent(result.runtimeConfirmed); setModal({ kind: 'files', skill, operation: mode, result });
  }
  async function auditSkills() {
    const value = await perform<Snapshot & { auditSummary: AuditSummary }>('正在核对运行文件与比较基准', 'audit_skills');
    if (value?.auditSummary) setModal({ kind: 'audit', summary: value.auditSummary });
  }
  async function restoreAttachments(skill: Skill) {
    const value = await perform<{ receipts: CleanupReceipt[] }>('正在读取附件恢复记录', 'get_cleanup_receipts', { skillId: skill.id });
    if (value) setModal({ kind: 'receipts', skill, receipts: value.receipts });
  }
  async function previewRestore(skill: Skill, receiptId: string) {
    const value = await perform<Preview>('正在预览从 GitHub 恢复附件', 'preview_restore', { skillId: skill.id, receiptId });
    if (value) showPreview(value);
  }
  async function previewManageConnections(connections: ClientConnection[], mode: ClientManagementMode) {
    if (!connections.length) return;
    const value = await perform<Preview>(mode === 'stop' ? '正在预览忽略连接' : '正在预览断开连接', 'preview_disconnect', { connectionIds: connections.map(connection => connection.id), mode, ...(connections.every(connection => connection.client === connections[0].client) ? { client: connections[0].client } : {}) });
    if (value) showPreview(value);
  }
  async function connectSkills(client: string, skillIds: string[]) {
    const value = await perform<Snapshot & { connectionResults: ConnectionResult[] }>('正在批量连接客户端', 'connect_batch', { skillIds, client });
    if (!value) throw new Error('本批操作已转入操作记录，请在对应界面查看结果。');
    return value.connectionResults;
  }
  function showPreview(preview: Preview) { setPreviewFile(preview.files?.find(file => file.change !== 'unchanged')?.path || preview.files?.[0]?.path || ''); setCommitMessage(''); setModal({ kind: 'preview', preview }); }
  function runtimeSelection(preview: Preview, skill: Skill) {
    const entries = (preview.availableFiles || []).map(file => typeof file === 'string' ? { path: file } : file);
    const suggested = new Set([...(preview.suggestedFiles || []), ...(preview.requiredFiles || [])]);
    setSelectedFiles(entries.filter(file => file.runtime || suggested.has(file.path) || requiredFile(file.path)).map(file => file.path));
    setRuntimeConsent(false);
    setModal({ kind: 'files', skill, operation: 'update', result: { files: entries, runtimeConfirmed: false, requiredFiles: preview.requiredFiles, optionalFiles: preview.optionalFiles, requiredReasons: preview.requiredReasons, suggestedFiles: preview.suggestedFiles } });
    for (const warning of preview.warnings || []) notify('info', warning);
  }
  async function previewUpdate(skill: Skill) { const value = await perform<Preview>('正在比较更新内容', 'preview_update', { skillId: skill.id }); if (value?.needsRuntimeSelection) runtimeSelection(value, skill); else if (value) showPreview(value); }
  async function filePreview() {
    if (modal?.kind !== 'files') return;
    if (modal.operation === 'install' && destinationPreparing.current) return;
    const selection = modal;
    let installTarget = '';
    if (modal.operation === 'install') {
      destinationPreparing.current = true; setPreparingDestination(true);
      try { const choice = await checkedDestination(); assertDestinationCurrent(selection, choice); installTarget = choice.path; }
      catch (cause) { if (mounted.current && modalRef.current === selection) setModalError(errText(cause)); destinationPreparing.current = false; if (mounted.current) setPreparingDestination(false); return; }
    }
    const value = await perform<Preview>(modal.operation === 'install' ? '正在准备下载预览' : modal.operation === 'update' ? '正在准备更新预览' : '正在准备上传预览', modal.operation === 'install' ? 'preview_install' : modal.operation === 'update' ? 'preview_update' : modal.operation === 'upload' ? 'preview_upload' : 'preview_push', { skillId: modal.skill.id, files: selectedFiles, ...(modal.operation === 'install' ? { targetPath: installTarget } : {}), ...(['push', 'upload'].includes(modal.operation) ? { trimOptional } : {}), ...(modal.operation === 'upload' ? { sourceId: uploadSource, branch: uploadBranch.trim(), remotePath: uploadPath.trim() } : {}) });
    destinationPreparing.current = false; if (mounted.current) setPreparingDestination(false);
    if (value?.needsRuntimeSelection) {
      const entries = value.availableFiles?.map(file => typeof file === 'string' ? { path: file } : file) || modal.result.files;
      setSelectedFiles(previous => [...new Set([...previous.filter(path => entries.some(file => file.path === path)), ...(value.requiredFiles || []), ...entries.filter(file => requiredFile(file.path)).map(file => file.path)])]);
      setRuntimeConsent(false); setModal({ ...modal, result: { files: entries, runtimeConfirmed: false, requiredFiles: value.requiredFiles, optionalFiles: value.optionalFiles, requiredReasons: value.requiredReasons, suggestedFiles: value.suggestedFiles } });
      for (const warning of value.warnings || ['请重新核对运行文件范围，并保留必要资源。']) notify('info', warning);
      return;
    }
    if (value) showPreview(value);
  }
  async function applyPreview() {
    if (modal?.kind !== 'preview' || !modal.preview.canApply || !modal.preview.id) return;
    const original = modal.preview;
    const kind = original.kind.replace(/^preview_/, '');
    const upload = kind === 'push' || kind === 'upload';
    const result = await perform<Snapshot & { transferResult?: TransferResult; connectionResults?: ConnectionResult[] }>(upload ? '正在上传选定文件' : kind === 'disconnect' ? original.mode === 'stop' ? '正在忽略连接' : '正在解除预览中的目录链接' : kind === 'restore' ? '正在恢复附件' : '正在应用文件变化', `apply_${kind}`, { previewId: original.id, ...(upload ? { message: commitMessage.trim() } : {}) });
    if (result) {
      if (kind === 'disconnect' && result.connectionResults) { setModal({ kind: 'client-result', mode: original.mode || 'disconnect', results: result.connectionResults }); return; }
      setModal(null);
      refreshInventory(original.skillId);
      if (result.transferResult) setTransferResult({ ...result.transferResult, skillId: original.skillId });
    } else setModal(previous => previous?.kind === 'preview' && previous.preview.id === original.id ? { kind: 'preview', preview: { ...original, canApply: false, warnings: [...(original.warnings || []), '此预览已失效。关闭后检查当前状态，再重新生成预览。'] } } : previous);
  }
  async function external(url: string) { try { const target = new URL(url); if (!['https:', 'http:'].includes(target.protocol)) throw new Error('仅支持打开 HTTP 或 HTTPS 链接'); await openUrl(target.toString()); notify('success', `已在浏览器打开 ${target.hostname} 链接。`); return true; } catch (error) { notify('error', `无法打开链接：${errText(error)}`); return false; } }
  async function openFolder(skill: Skill) { await perform('正在打开文件夹', 'open_folder', { skillId: skill.id }, `已打开 ${skillName(skill)} 的本地文件夹。`); }
  async function createTokenLink() {
    const opened = await external(tokenCreationUrl(tokenRepo, tokenAccess));
    setTokenLinkStatus(opened ? '已交给默认浏览器打开。请在 GitHub 仅选择目标仓库，再生成令牌。' : '浏览器未能打开。请复制创建链接，在浏览器地址栏粘贴打开。');
  }
  async function copy(text: string) { try { await navigator.clipboard.writeText(text); notify('success', '已复制。'); } catch (error) { notify('error', `复制失败：${errText(error)}`); } }
  function share(skill: Skill) { const commit = skill.baselineCommit || skill.commit; if (!skill.repo || !commit) { notify('info', '此 Skill 尚无远端固定版本，可导出运行包分享。'); return; } const suffix = skill.remotePath ? `/${skill.remotePath.split('/').map(encodeURIComponent).join('/')}` : ''; void copy(`https://github.com/${skill.repo}/tree/${encodeURIComponent(commit)}${suffix}`); if (skill.private) notify('info', '这是私有仓库链接，接收者需要具备仓库访问权限。'); }
  function resetAutoMatch(skillId: string) { for (const key of autoMatchAttempts.current) if (JSON.parse(key)[0] === skillId) autoMatchAttempts.current.delete(key); }
  function refreshInventory(skillId: string) { inventoryGeneration.current += 1; resetAutoMatch(skillId); for (const cache of [inventoryCache.current, inventoryPending.current]) for (const key of cache.keys()) if (JSON.parse(key)[0] === skillId) cache.delete(key); fileRequestId.current += 1; setFileContent(null); setInventoryRefresh(value => value + 1); }
  async function autoMatchExternal(skill: Skill, inventory: FileResult) {
    if (!skill.sourceId || !inventory.remoteCommit || skill.readOnly || !sessionRef.current.owned || sessionRef.current.isTransferring()) return;
    const references = (inventory.externalDependencies || []).filter(item => !['command', 'skill_alias'].includes(item.referenceKind || ''));
    if (!references.length) return;
    const key = JSON.stringify([skill.id, inventoryStamp(skill), inventory.remoteCommit, references.map(item => [item.id, item.referenceFingerprint, item.role, item.local?.status, item.canonicalPath, item.hash, item.readable]).sort((a, b) => String(a[0]).localeCompare(String(b[0])))]);
    if (autoMatchAttempts.current.has(key)) return;
    autoMatchAttempts.current.add(key); while (autoMatchAttempts.current.size > 160) autoMatchAttempts.current.delete(autoMatchAttempts.current.values().next().value!);
    const sequence = ++autoMatchSequence.current;
    const generation = inventoryGeneration.current;
    const stamp = inventoryStamp(skill);
    const current = () => mounted.current && selectedIdRef.current === skill.id && selectedContentStampRef.current === stamp && generation === inventoryGeneration.current && sequence === autoMatchSequence.current && sessionRef.current.owned && !sessionRef.current.isTransferring();
    let acknowledgeRegistration = () => {};
    let registeredTaskId: string | undefined;
    const registration = new Promise<void>(resolve => { acknowledgeRegistration = resolve; });
    registrationAcks.current.add(registration);
    setAutoMatchFeedback('正在自动匹配本地与 GitHub 文件…');
    try {
      const value = await runTask<{ items: ExternalDependency[]; stateSnapshot?: Snapshot; autoMatchSummary: { matched: number; needsSelection: number; missing: number; error: number; recordsUpdated?: number; recordsRepaired?: number; remoteReady?: number; localPreviewRequired?: number; contentDiffers?: number } }>('正在自动匹配文件来源', 'batch_external_dependencies', { skillId: skill.id, operation: 'auto_match', remoteCommit: inventory.remoteCommit, items: references.map(item => ({ id: item.id, expectedFingerprint: item.referenceFingerprint })) }, { paneId: pane.id, skillId: skill.id, view: 'skills', tab: 'files' }, false, task => { registeredTaskId = task.id; startedTasks.current.add(task.id); acknowledgeRegistration(); registrationAcks.current.delete(registration); });
      if (value.stateSnapshot) snapshot(value.stateSnapshot);
      if (!current()) return;
      const counts = value.autoMatchSummary;
      setAutoMatchFeedback([
        `自动匹配 ${counts.matched} 项`,
        counts.recordsRepaired ? `记录已修复 ${counts.recordsRepaired} 项` : '',
        counts.remoteReady ? `远端资源已就绪 ${counts.remoteReady} 项` : '',
        counts.localPreviewRequired ? `本地补齐需预览 ${counts.localPreviewRequired} 项` : '',
        counts.contentDiffers ? `本地差异已保留 ${counts.contentDiffers} 项` : '',
        `需要选择 ${counts.needsSelection} 项 · 未找到 ${counts.missing} 项 · 读取失败 ${counts.error} 项`,
      ].filter(Boolean).join(' · '));
      setFiles(previous => previous && previous.remoteCommit === inventory.remoteCommit ? { ...previous, externalDependencies: value.items } : previous);
    } catch (cause) { if (current()) setAutoMatchFeedback(`自动匹配未完成：${errText(cause)}。可重新检查后重试。`); }
    finally { acknowledgeRegistration(); registrationAcks.current.delete(registration); if (registeredTaskId) requestAnimationFrame(() => { if (!sessionRef.current.isTransferring()) startedTasks.current.delete(registeredTaskId!); }); }
  }
  async function manageRuntime(skill: Skill) {
    const result = await perform<FileResult>('正在读取本地运行文件', 'get_files', { skillId: skill.id }, undefined, undefined, true, { kind: 'runtime' });
    if (!result) return;
    setSelectedFiles(result.files.filter(file => file.runtime || requiredRuntimeFile(result, file.path) || !result.runtimeConfirmed && result.suggestedFiles?.includes(file.path)).map(file => file.path));
    setModal({ kind: 'runtime', skill, result });
  }
  function exportSkill(skill: Skill) { setDestinationChoice(destinationDraft()); setModal({ kind: 'export', skill }); }
  function prepareExport(skill: Skill, _inventory?: FileResult) { exportSkill(skill); }
  async function saveExport() {
    if (modal?.kind !== 'export' || !exportPlanState.current || !exportPlan?.canExport || destinationPreparing.current) return;
    const selection = modal;
    destinationPreparing.current = true; setPreparingDestination(true);
    try {
      const choice = await checkedDestination();
      assertDestinationCurrent(selection, choice);
      const draft = await exportSelection.flush();
      assertDestinationCurrent(selection, choice);
      if (JSON.stringify([...draft.selectedIds].sort()) !== JSON.stringify([...exportSelection.selectedIds].sort())) { exportPlanState.refresh(); throw new Error('导出选择已更新，请核对当前清单后再导出。'); }
      const planId = exportPlan.id;
      const destination = choice.path;
      const latest = await exportSelection.flush();
      assertDestinationCurrent(selection, choice);
      if (latest.id !== draft.id || latest.revision !== draft.revision) throw new Error('准备导出期间，文件选择发生变化。请核对最新清单后再次导出。');
      // Capture transfer recovery without replacing the modal used by perform's result guard.
      inFlightExportSelection.current = draft;
      const value = await perform<{ path: string }>('正在导出 Skill', 'export', { skillId: selection.skill.id, planId, destination, destinationMode: choice.mode }, undefined, () => exportPlanState.refresh());
      if (value) {
        let resetError = '';
        try { await exportSelection.clearIfCurrent(draft); } catch (cause) { resetError = errText(cause); }
        inFlightExportSelection.current = undefined;
        setModal(null);
        notify('success', `Skill 已导出：${value.path}${resetError ? `。本次选择未能重置：${resetError}` : ''}`);
      }
    } catch (cause) { if (mounted.current && modalRef.current === selection) setModalError(errText(cause)); }
    finally { destinationPreparing.current = false; if (mounted.current) setPreparingDestination(false); }
  }
  async function checkExternal(skill: Skill) {
    const generation = ++inventoryGeneration.current;
    resetAutoMatch(skill.id);
    const stamp = inventoryStamp(skill);
    inventoryCache.current.delete(stamp); inventoryPending.current.delete(stamp);
    const value = await perform<FileResult>('正在核验本地与 GitHub 文件', 'get_files', { skillId: skill.id, includeRemote: true });
    if (value && generation === inventoryGeneration.current) { inventoryCache.current.set(stamp, value); setFiles(value); setRuntimeFiles(requiredExportPaths(value)); setFileBusy(false); }
  }
  async function bindExternal(skill: Skill, dependencyId: string, role: string, path?: string, options?: ExternalBindingOptions) {
    const value = await perform<{ items: ExternalDependency[] }>('正在保存外部文件用途', 'bind_external_dependency', { skillId: skill.id, dependencyId, role, ...(path ? { path } : {}), ...options });
    if (value) { setFiles(previous => previous ? { ...previous, externalDependencies: value.items } : previous); refreshInventory(skill.id); }
    else throw new Error('绑定未保存，请查看本页错误或操作记录。');
  }
  const [externalBatchResults, setExternalBatchResults] = useState<{ id?: string; error?: string; reason?: string; status?: string; ok?: boolean }[]>([]);
  async function batchExternal(skill: Skill, batch: ExternalBatchRequest) {
    const value = await perform<{ items: ExternalDependency[]; results: { id?: string; error?: string; reason?: string; status?: string; ok?: boolean }[] }>('正在批量管理外部文件', 'batch_external_dependencies', { skillId: skill.id, ...batch, ...(batch.operation === 'auto_match' && files?.remoteCommit ? { remoteCommit: files.remoteCommit } : {}) });
    if (!value) throw new Error('批量操作未完成，请查看操作记录。');
    setExternalBatchResults(value.results || []);
    setFiles(previous => previous ? { ...previous, externalDependencies: value.items } : previous);
    refreshInventory(skill.id);
    return value;
  }
  async function openTask(item: ManagerTask) {
    try { const task = await inspectTask(item.id); setModal({ kind: 'task', task }); }
    catch (error) { notify('error', `无法读取任务记录：${errText(error)}`); }
  }
  async function locateTask(task: ManagerTask) {
    if (task.origin.paneId && task.origin.paneId !== pane.id) {
      const current = await request<{ workspace: WorkspaceNavigation }>('workspace_get');
      const target = current.workspace.windows.flatMap(window => window.panes).find(item => item.id === task.origin.paneId);
      if (target) {
        sessionRef.current.focusPane(target.id, target.kind === 'home' ? { view: task.origin.clientId ? 'clients' : ['skills', 'changes', 'clients', 'settings'].includes(task.origin.view || '') ? task.origin.view : 'skills', clientId: task.origin.clientId } : { section: ['docs', 'files', 'source'].includes(task.origin.tab || '') ? task.origin.tab : undefined });
        return;
      }
    }
    if (task.origin.clientId) { sessionRef.current.goHome('clients', task.origin.clientId); return; }
    const skill = stateRef.current.skills.find(item => item.id === task.origin.skillId);
    if (task.origin.skillId && !skill) { notify('info', '该 Skill 已移除，记录仍可查看。'); sessionRef.current.goHome(); return; }
    if (skill) { setQuery(''); setFilter('all'); setStatusFilter('all'); setCategoryFilter('all'); openDetail(skill, ['docs', 'files', 'source'].includes(task.origin.tab || '') ? task.origin.tab as 'docs' | 'files' | 'source' : 'docs'); }
    else setView(['skills', 'changes', 'clients', 'settings'].includes(task.origin.view || '') ? task.origin.view as View : 'skills');
  }
  async function downloadAttachments(skill: Skill) {
    const paths = extraFiles.filter(path => { const file = files?.files.find(item => item.path === path); return file && !localFile(file, skill) && remoteFile(file, skill); });
    if (!paths.length || !files?.remoteCommit) return;
    const value = await perform<Preview>('正在预览附件下载', 'preview_attachments', { skillId: skill.id, files: paths, remoteCommit: files.remoteCommit });
    if (value) showPreview(value);
  }
  useEffect(() => { fileRequestId.current += 1; }, [selectedId]);
  async function readFile(path: string, source?: 'local' | 'github') {
    if (!selected || !files) return;
    const entry = files.files.find(file => file.path === path);
    const from = source || (entry && localFile(entry, selected) ? 'local' : 'github');
    if (from === 'github' && !files.remoteCommit) { setFileError('远端版本尚未读取，请刷新文件清单。'); return; }
    const skillId = selected.id;
    const scrollAtRequest = scrollTop.current;
    const requestedCommit = from === 'github' ? files.remoteCommit : undefined;
    setFileError(''); const requestId = ++fileRequestId.current;
    try {
      const result = await request<{ content: string; hash?: string; remoteCommit?: string }>('read_file', { skillId, path, ...(from === 'github' ? { source: 'github', remoteCommit: requestedCommit } : {}) });
      if (fileRequestId.current === requestId && selectedIdRef.current === skillId) {
        const remoteCommit = from === 'github' ? result.remoteCommit || requestedCommit : undefined;
        setFileContent({ path, content: result.content, source: from, remoteCommit });
        if (sessionRef.current.active && Math.abs(scrollTop.current - scrollAtRequest) < 24) requestAnimationFrame(() => { if (sessionRef.current.active && fileRequestId.current === requestId) paneRoot.current?.querySelector('.file-reader')?.scrollIntoView({ block: 'nearest', behavior: 'smooth' }); });
        if (from === 'github' && result.hash && entry?.localHash) setFiles(previous => previous && previous.remoteCommit === remoteCommit ? { ...previous, files: previous.files.map(file => file.path === path && file.localHash === entry.localHash ? { ...file, contentDifferent: file.localHash !== result.hash } : file) } : previous);
      }
    } catch (error) { if (fileRequestId.current === requestId && selectedIdRef.current === skillId) { setFileContent(null); setFileError(`读取${from === 'github' ? ' GitHub ' : '本地'}文件失败：${errText(error)}`); } }
  }
  async function confirmRuntime() {
    if (modal?.kind !== 'runtime') return;
    const skill = modal.skill;
    const confirmedFiles = [...selectedFiles];
    const result = await perform<Snapshot>('正在确认运行文件清单', 'set_runtime_files', { skillId: skill.id, files: confirmedFiles }, '运行文件清单已保存；未选文件继续保留在原位置。');
    if (result) { setModal(null); refreshInventory(skill.id); }
  }
  function openAssociate(skill: Skill) { setAssociateSource(state.sources[0]?.id || ''); setAssociateSkill(''); setModal({ kind: 'associate', skill }); }
  async function associate() { if (modal?.kind !== 'associate') return; const remote = state.skills.find(skill => skill.id === associateSkill); if (!remote) return; const result = await perform<Snapshot>('正在关联来源', 'associate', { skillId: modal.skill.id, sourceId: associateSource, remotePath: remote.remotePath || '' }, '已建立关联；基准需要通过实际内容核对。'); if (result) setModal(null); }
  const sourceSkill = modal?.kind === 'associate' ? state.skills.filter(skill => skill.sourceId === associateSource && !skill.localPath) : [];
  useEffect(() => { if (view !== 'settings') setToken(''); }, [view]);
  const selectedConnections = selected ? state.connections.filter(connection => connection.skillId === selected.id) : [];
  const busy = !!modal && (pendingContexts.some(item => item.modal === modal) || resumingIds.length > 0);
  const managementBusy = pendingContexts.some(item => item.metadata);
  const displayChanged = uiTheme !== (state.settings.uiTheme || 'light') || uiFontSize !== (state.settings.uiFontSize || 16);
  function changeZoom(value: number) { const zoom = displayNumber(value, 100, 80, 160); currentLayout.current.zoom = zoom; setUiZoom(zoom); setZoomInput(String(zoom)); queueLayout({ uiZoom: zoom }); }
  function resetDisplay() { setUiTheme('light'); changeZoom(100); setUiFontSize(16); }
  const title = { skills: '技能库', changes: '变化', clients: '客户端', settings: '设置' }[view];
  function menuToggle(event: React.MouseEvent<HTMLButtonElement> | React.KeyboardEvent<HTMLButtonElement>, menu: Omit<OpenMenu, 'anchor'>) {
    if ('key' in event && event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    event.preventDefault(); event.stopPropagation();
    const anchor = event.currentTarget;
    setFloatingMenu(previous => previous?.id === menu.id ? null : { ...menu, anchor, last: 'key' in event && event.key === 'ArrowUp' });
  }
  const categoryPanel = <section className="category-panel" aria-label="Skill 分类">
    <div className="category-heading"><strong>分类</strong><button className="icon-button" aria-label="创建分类" title="创建分类" disabled={busy} onClick={() => editCategory()}><Plus size={16} /></button>{compactSidebar && <button className="icon-button" aria-label="关闭分类面板" onClick={() => setSidebarFlyout(false)}><X size={15} /></button>}</div>
    <div className="category-list">{[{ id: 'all', name: '全部 Skills' }, { id: 'favorites', name: '收藏' }, { id: 'uncategorized', name: '未分类' }, ...categories].map(category => {
      const count = state.skills.filter(skill => category.id === 'all' || (category.id === 'favorites' ? skill.favorite : category.id === 'uncategorized' ? !skill.categoryId : skill.categoryId === category.id)).length;
      const custom = !['all', 'uncategorized', 'favorites'].includes(category.id);
      const acceptsSkills = category.id !== 'all' && category.id !== 'favorites';
      const menu = { id: `category:${category.id}`, label: `管理分类 ${category.name}`, side: 'right' as const, items: [
        { id: 'rename', label: '重命名', disabled: busy, action: () => editCategory(category) },
        { id: 'color', label: '设置颜色', disabled: busy, action: () => editCategory(category) },
        { id: 'up', label: <><ArrowUp size={13} />上移</>, disabled: busy || categories[0]?.id === category.id, action: () => void moveCategory(category.id, -1) },
        { id: 'down', label: <><ArrowDown size={13} />下移</>, disabled: busy || categories.at(-1)?.id === category.id, action: () => void moveCategory(category.id, 1) },
        { id: 'remove', label: '移除分类', disabled: busy, danger: true, action: () => setModal({ kind: 'delete-category', category }) },
      ] };
      return <div key={category.id} data-category-id={category.id} data-category-color={resolvedCategoryColors.get(category.id)} data-drop-position={categoryOrderTarget?.id === category.id ? categoryOrderTarget.position : undefined} className={`category-item ${custom ? 'is-sortable' : ''} ${categoryFilter === category.id && view === 'skills' ? 'is-active' : ''} ${dropCategory === category.id ? 'is-drop-target' : ''} ${categoryOrderTarget?.id === category.id ? 'is-category-drop-target' : ''}`} onDragOver={event => { if (custom && event.dataTransfer.types.includes('application/x-skill-manager-category')) overCategoryOrder(event, category.id); else if (acceptsSkills) overCategory(event, category.id); }} onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) { setDropCategory(null); setCategoryOrderTarget(null); } }} onDrop={event => { if (custom && event.dataTransfer.types.includes('application/x-skill-manager-category')) void dropCategoryOrder(event, category.id); else if (acceptsSkills) dropIntoCategory(event, category.id); }}>
        {custom && <button className="icon-button category-order-handle" draggable={!busy} disabled={busy} aria-label={`拖动调整 ${category.name} 顺序`} title="拖动调整分类顺序；也可在菜单中上移或下移" onDragStart={event => startCategoryDrag(event, category)} onDragEnd={endCategoryDrag}><GripVertical size={13} /></button>}
        <button className="category-select" title={`${category.name} · ${count} 个 Skill`} aria-pressed={categoryFilter === category.id && view === 'skills'} onClick={() => selectCategory(category.id)}>{category.id === 'favorites' ? <Star size={15} /> : custom ? <span className="category-color-dot" aria-hidden="true" /> : <Folder size={15} />}<span>{category.name}</span><small>{count}</small></button>
        {custom && <button className="icon-button category-actions" aria-label={`管理分类 ${category.name}`} title={`管理分类 ${category.name}`} aria-haspopup="menu" aria-expanded={floatingMenu?.id === menu.id} disabled={busy} onClick={event => menuToggle(event, menu)} onKeyDown={event => menuToggle(event, menu)}><Ellipsis size={16} /></button>}
      </div>;
    })}</div>
    {!!dragSkillIds.length && <p className="category-drop-hint" role="status">{dropCategory ? `移入“${dropCategory === 'uncategorized' ? '未分类' : categories.find(category => category.id === dropCategory)?.name || '此分类'}” · ${dragSkillIds.length} 项` : `将 ${dragSkillIds.length} 个 Skill 拖入分类`}</p>}
  </section>;
  const assignmentMenu = { id: 'assignment', label: '归入分类', items: [...[{ id: 'uncategorized', name: '未分类' }, ...categories].map(category => ({ id: category.id, label: category.name, disabled: busy || !visibleChecked.length, action: () => void assignCategory(visibleChecked, category.id === 'uncategorized' ? null : category.id) })), { id: 'create', label: <><Plus size={13} />创建分类</>, disabled: busy, action: () => editCategory() }] };
  const categoryAssignmentMenu = <Button className="assign-category-menu" aria-label="归入分类" aria-haspopup="menu" aria-expanded={floatingMenu?.id === 'assignment'} disabled={busy || !visibleChecked.length} onClick={event => menuToggle(event, assignmentMenu)} onKeyDown={event => menuToggle(event, assignmentMenu)}><Folder size={14} />归入分类</Button>;

  const runtimeInventory = files && selected ? files.files.filter(file => {
    const resource = exportResources.find(item => item.id === `file:${file.path}`);
    return resource ? resource.group === 'required' : requiredExportPaths(files).includes(file.path) || !selected.localPath && requiredRuntimeFile(files, file.path);
  }) : [];
  const optionalInventory = files ? files.files.filter(file => {
    const resource = exportResources.find(item => item.id === `file:${file.path}`);
    return resource ? resource.group === 'optional_package' : !runtimeInventory.some(runtime => runtime.path === file.path);
  }) : [];
  const remoteExtras = selected && files ? extraFiles.filter(path => { const file = files.files.find(item => item.path === path); return file && !localFile(file, selected) && remoteFile(file, selected); }) : [];
  const currentFile = files?.files.find(file => file.path === fileContent?.path);

  const [outlineExpanded, setOutlineExpanded] = useState(seed('outlineExpanded', pane.state.outlineExpanded === true));
  const scrollTop = useRef(Number(session.initialDraft.scrollTop ?? pane.state.scrollTop ?? 0));
  const homeRestorePending = useRef(isHome && scrollTop.current > 0);
  const scrollSaveTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  function saveScroll(top: number) {
    scrollTop.current = top; clearTimeout(scrollSaveTimer.current);
    scrollSaveTimer.current = setTimeout(() => sessionRef.current.updateNavigation(pane.id, { scrollTop: top }), 500);
  }
  useEffect(() => () => clearTimeout(scrollSaveTimer.current), []);
  useEffect(() => {
    if (!isHome || !homeRestorePending.current) return;
    const root = paneRoot.current, scroller = root?.querySelector<HTMLElement>(':scope > .main');
    if (!root || !scroller) return;
    let frame = 0;
    const desired = scrollTop.current;
    const attempt = () => {
      if (!homeRestorePending.current || !scroller.clientHeight) return;
      if (scroller.scrollHeight - scroller.clientHeight + 1 >= desired) { homeRestorePending.current = false; scroller.scrollTo({ top: desired, behavior: 'instant' }); }
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(attempt); };
    const stop = () => { homeRestorePending.current = false; };
    const wheel = (event: WheelEvent) => { if (!event.ctrlKey) stop(); };
    root.addEventListener('pointerdown', stop, true); root.addEventListener('keydown', stop, true); scroller.addEventListener('wheel', wheel, { passive: true }); scroller.addEventListener('touchstart', stop, { passive: true });
    const resize = new ResizeObserver(schedule); resize.observe(scroller);
    const changes = new MutationObserver(schedule); changes.observe(scroller, { childList: true, subtree: true }); schedule();
    return () => { resize.disconnect(); changes.disconnect(); cancelAnimationFrame(frame); root.removeEventListener('pointerdown', stop, true); root.removeEventListener('keydown', stop, true); scroller.removeEventListener('wheel', wheel); scroller.removeEventListener('touchstart', stop); };
  }, [isHome]);
  function captureFolds() { return paneRoot.current ? captureSectionFolds(paneRoot.current) : []; }
  const initialFolds = useRef(seed('expandedSections', pane.state.expandedSections || []));
  const foldsRestored = useRef(new Set<string>());
  useEffect(() => {
    const root = paneRoot.current; if (!root || !initialFolds.current.length) return;
    const restore = () => restoreSectionFolds(root, initialFolds.current, foldsRestored.current);
    restore(); const observer = new MutationObserver(restore); observer.observe(root, { childList: true, subtree: true }); return () => observer.disconnect();
  }, []);
  captureRef.current = () => ({ view, filter, query, libraryPage, statusFilter, categoryFilter, categoryName, categoryDraftColor, checkedSkills, detailOpen, selectedId, tab, version, documentPath, documentCommit, documentLocal, documentCacheKey: docKey, documentValue: doc, files, runtimeFiles, fileContent, inventoryFailed, uploadSource, uploadBranch, uploadPath, trimOptional, transferResult, scanPath, scanResult, scanSelected, githubUrl, associateSource, associateSkill, targetPath, destinationChoice, selectedFiles, runtimeConsent, commitMessage, previewFile, activeClient, modal, inFlightExportSelection: inFlightExportSelection.current, originPaneId: session.initialDraft.originPaneId || pane.id, expandedSections: captureFolds(), historyDraft: historyDraft.current, externalManagementDraft: externalManagementDraft.current, clientConnectDraft: clientConnectDraft.current, storageDraft: storageDraft.current, networkDraft: networkDraft.current, settingsOutline, autoMatchFeedback, deferredModal: deferredModal.current, pendingTaskIntents: Object.fromEntries(pendingTaskIntents.current), pendingTaskIds: [...startedTasks.current], scrollTop: scrollTop.current, outlineExpanded });
  useEffect(() => {
    if (!ready) return;
    const timer = setTimeout(() => sessionRef.current.updateNavigation(pane.id, { view, clientId: activeClient, section: tab, documentPath, documentVersion: version, outlineExpanded }), 250);
    return () => clearTimeout(timer);
  }, [ready, pane.id, view, activeClient, tab, documentPath, version, outlineExpanded]);
  useEffect(() => {
    if (!session.owned) return;
    let cancelled = false;
    for (const id of inheritedTasks.current) void (async () => {
      while (!cancelled) {
        try {
          const result = await inspectTask(id);
          if (cancelled) return;
          if (!terminalTask(result)) { await new Promise(resolve => setTimeout(resolve, 700)); continue; }
          snapshot(result.result); const value = result.result as { stateSnapshot?: Snapshot; transferResult?: TransferResult; connectionResults?: ConnectionResult[]; mode?: ClientManagementMode; files?: unknown[]; runtimeConfirmed?: boolean; path?: string; canApply?: boolean } | undefined;
          if (value?.stateSnapshot) snapshot(value.stateSnapshot);
          if (result.action === 'batch_external_dependencies' && result.origin.skillId && result.origin.skillId === selectedIdRef.current) {
            refreshInventory(result.origin.skillId);
            setAutoMatchFeedback(result.status === 'failed' ? `自动匹配未完成：${result.error || result.stage}` : '自动匹配已完成，正在读取最新文件来源。');
          }
          if (value?.transferResult) setTransferResult(value.transferResult);
          if (result.action === 'apply_disconnect' && value?.connectionResults && pane.kind === 'operation') { setLocalModal({ kind: 'client-result', mode: value.mode || 'disconnect', results: value.connectionResults }); sessionRef.current.updateNavigation(pane.id, { operation: 'client-result' }); }
          if (value?.runtimeConfirmed !== undefined && Array.isArray(value.files)) setFiles(value as FileResult);
          const intent = pendingTaskIntents.current.get(id);
          const skill = stateRef.current.skills.find(item => item.id === (intent?.skillId || result.origin.skillId || pane.skillId));
          if (value && typeof value === 'object') taskForResult.current.set(value, id);
          if (['succeeded', 'partial'].includes(result.status) && intent && value) {
            if (intent.kind === 'runtime' && skill) setModal({ kind: 'runtime', skill, result: value as FileResult });
            else if (intent.kind === 'files' && skill && intent.operation) setModal({ kind: 'files', skill, operation: intent.operation, result: value as FileResult });
            else if (intent.kind === 'export' && skill) prepareExport(skill, value as FileResult);
            else if (intent.kind === 'audit' && 'auditSummary' in value) { const summary = value.auditSummary as AuditSummary; taskForResult.current.set(summary, id); setModal({ kind: 'audit', summary }); }
            else if (intent.kind === 'receipts' && skill && 'receipts' in value) { const receipts = value.receipts as CleanupReceipt[]; taskForResult.current.set(receipts, id); setModal({ kind: 'receipts', skill, receipts }); }
            else if (intent.kind === 'scan' && 'skills' in value) { const scan = value as { skills: Skill[]; warnings: string[] }; setScanResult(scan); setScanSelected(scan.skills.filter(item => !item.readOnly).map(item => item.localPath || item.path || '').filter(Boolean)); }
          }

          if (result.action.startsWith('apply_')) setLocalModal(previous => previous?.kind === 'preview' ? { kind: 'preview', preview: { ...previous.preview, canApply: false, warnings: [...(previous.preview.warnings || []), '此操作已结束，原预览不能再次执行。请查看结果或重新生成预览。'] } } : previous);
          if (value?.canApply !== undefined && pane.kind === 'operation') { setLocalModal({ kind: 'preview', preview: value as Preview }); setPreviewFile((value as Preview).files?.[0]?.path || ''); sessionRef.current.updateNavigation(pane.id, { operation: 'preview' }); }
          if (result.action === 'export' && result.status === 'succeeded') {
            const exported = modalRef.current?.kind === 'export' ? modalRef.current : undefined;
            const selection = inFlightExportSelection.current || exported?.inFlightSelection;
            if (selection) await exportSelection.clearIfCurrent(selection).catch(() => false);
            inFlightExportSelection.current = undefined;
          }
          setResumedTasks(previous => [...previous.filter(item => item.id !== result.id), result]);
          notify(result.status === 'failed' ? 'error' : 'success', `${result.label}：${taskStatus[result.status] || result.status}`);
          startedTasks.current.delete(id); pendingTaskIntents.current.delete(id); setResumingIds(previous => previous.filter(item => item !== id)); return;
        } catch (cause) { if (!cancelled) { notify('error', `读取移窗前的操作结果失败：${errText(cause)}`); setResumingIds(previous => previous.filter(item => item !== id)); } return; }
      }
    })();
    return () => { cancelled = true; };
  }, [pane.id, pane.kind, session.owned, inspectTask, snapshot, notify]);


  useEffect(() => {
    if (!session.owned || session.isTransferring() || !deferredModal.current) return;
    const deferred = deferredModal.current;
    if ((!deferred.value && inheritedTasks.current.length) || (deferred.outcomeTaskId && inheritedTasks.current.includes(deferred.outcomeTaskId))) { deferredModal.current = undefined; return; }
    deferredModal.current = undefined;
    if (deferred.value && deferred.outcomeTaskId) taskForResult.current.set(deferred.value, deferred.outcomeTaskId);
    setModal(deferred.value);
  }, [session.owned, session.active, session.pane, session.isTransferring]);

  const detailMoreMenu: Omit<OpenMenu, 'anchor'> | null = selected ? {
    id: 'detail-more', label: 'Skill 更多操作', side: 'bottom', items: [
      ...(selected.localPath ? [{ id: 'file-scope', label: '调整文件范围', title: '调整管理器检查、更新和上传的长期文件范围；不影响 Skill 是否启用。', disabled: !!selected.readOnly, action: () => { void manageRuntime(selected); } }] : []),
      { id: 'share', label: '复制分享链接', action: () => { void share(selected); } },
      ...(selected.localPath ? [{ id: 'restore', label: '恢复附件', title: !selected.sourceId ? '需先关联 GitHub，才能读取上传后的附件恢复记录。' : '读取已上传附件的恢复记录，核对后再恢复到本地。', disabled: !selected.sourceId, action: () => { void restoreAttachments(selected); } }] : []),
    ],
  } : null;
  const detailPanel = <section className="skill-detail">{!selected ? <Empty title="选择一个 Skill 查看详情" /> : <>
            {transferResult?.skillId === selected.id && <TransferNotice result={transferResult} onClose={() => setTransferResult(null)} />}
            <div className="detail-header"><div className="detail-title"><span className="detail-icon"><FileCode2 size={24} /></span><div><h2>{skillName(selected)}</h2><span className="monospace muted">{selected.name}</span></div><Badge skill={selected} /></div><p className="detail-description">{selected.description || '此 Skill 尚未提供用途说明。'}</p><div className="detail-meta"><span><GitBranch size={13} />{selected.version || '版本未知'}</span><span><Link2 size={13} />{selectedConnections.filter(item => item.linked).length} 个客户端连接</span>{selected.readOnly && <span><LockKeyhole size={13} />应用托管 · 只读</span>}</div>
            <div className="detail-organization"><label className="compact-select detail-category" data-category-color={resolvedCategoryColors.get(selected.categoryId || "")}><span>分类</span><span className="category-color-dot" aria-hidden="true" /><select aria-label={`为 ${skillName(selected)} 选择分类`} value={selected.categoryId || ''} disabled={busy} onChange={event => void assignCategory([selected.id], event.target.value || null)}><option value="">未分类</option>{categories.map(category => <option key={category.id} value={category.id}>{category.name}</option>)}</select></label>
            <Button className={`favorite-button ${selected.favorite ? 'is-favorite' : ''}`} aria-label={`${selected.favorite ? '取消收藏' : '收藏'} ${skillName(selected)}`} aria-pressed={!!selected.favorite} disabled={busy} onClick={() => void toggleFavorite(selected)}><Star size={15} fill={selected.favorite ? 'currentColor' : 'none'} />{selected.favorite ? '已收藏' : '收藏'}</Button></div>
            {selected.associationNote && <details className="association-note"><summary><Link2 size={14} />已关联 GitHub<span>查看关联依据</span></summary><p>{selected.associationNote}</p></details>}
            {selected.error && <div className="inline-error"><CircleAlert size={16} />{selected.error}</div>}
            {selected.remoteCheckRequired && <div className="inline-warning remote-pending"><CircleHelp size={14} />远端状态尚未核实。请“检查更新”后判断内容是否一致。</div>}
            {(selected.localNotice || !!selected.untrackedFiles?.length) && <div className="helper-note detail-local-notice" title={selected.localNotice || undefined}><Folder size={14} />有 {selected.untrackedFiles?.length || '额外'} 个本地文件未纳入已确认运行清单，请在“文件”页核对。原文件继续保留。</div>}
            {!!runtimeAttentionCount(selected.runtimeIssues) && <div className="inline-warning"><CircleAlert size={14} />有 {runtimeAttentionCount(selected.runtimeIssues)} 项运行文件需要核对，请在“文件”页查看。</div>}
            <div className="detail-actions">{!selected.localPath ? <Button variant="primary" onClick={() => void selectOperation(selected, 'install')} disabled={busy || selected.readOnly}><Download size={15} />下载 Skill</Button> : <>{selected.sourceId && <Button variant="primary" onClick={() => void previewUpdate(selected)} disabled={busy || selected.readOnly}><ArrowDownToLine size={15} />预览更新</Button>}<Button disabled={busy || selected.readOnly} onClick={() => void selectOperation(selected, selected.sourceId ? 'push' : 'upload')}><Upload size={15} />{selected.sourceId ? '上传本地修改' : '上传到 GitHub'}</Button><Button onClick={() => void perform('正在打开编辑器', 'open_local', { skillId: selected.id })} disabled={busy || selected.readOnly}><Code2 size={15} />外部编辑器</Button></>}{selected.localPath && <Button variant="ghost" onClick={() => void exportSkill(selected)} disabled={busy}><Box size={15} />导出</Button>}<Button variant="ghost" aria-haspopup="menu" aria-expanded={floatingMenu?.id === 'detail-more'} disabled={busy} onClick={event => menuToggle(event, detailMoreMenu!)} onKeyDown={event => menuToggle(event, detailMoreMenu!)}><Ellipsis size={15} />更多</Button></div></div>
            <div className="detail-tabs" role="tablist" aria-label="Skill 详情">{([{ id: 'docs', label: '说明', icon: <BookOpen size={15} /> }, { id: 'files', label: '文件', icon: <File size={15} /> }, { id: 'source', label: '来源与连接', icon: <GitBranch size={15} /> }] as const).map(item => <button role="tab" aria-selected={tab === item.id} className={tab === item.id ? 'active' : ''} key={item.id} onClick={() => setTab(item.id)}>{item.icon}{item.label}</button>)}</div>
            <div className="detail-content">
              {tab === 'docs' && <><div className="document-toolbar"><div className="segmented small"><button className={version === 'latest' ? 'selected' : ''} onClick={() => { setVersion('latest'); setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); }}>最新说明</button><button disabled={!selected.localPath || !!selected.sourceId && !selected.installedCommit} title={!selected.installedCommit && selected.sourceId ? '没有可核实的历史安装记录' : undefined} className={version === 'installed' ? 'selected' : ''} onClick={() => { setVersion('installed'); setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); }}>安装版本</button></div><Button variant="ghost" aria-label="刷新说明" disabled={docBusy || documentState.refreshing} onClick={documentState.refresh}><RefreshCw size={14} className={documentState.refreshing ? 'spin' : ''} />{documentState.refreshing ? '正在刷新' : '刷新说明'}</Button>{documentPath && <Button variant="ghost" onClick={() => { setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); }}>返回首页</Button>}</div>
              {selected.localPath && selected.sourceId && !selected.installedCommit && <div className="helper-note document-baseline-note"><CircleHelp size={14} /><span>历史安装版本未知。当前比较基准只说明文件匹配，不代表当初的安装版本。</span><button className="text-button" onClick={() => { setVersion('latest'); setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); }}>阅读最新说明<ArrowUpRight size={12} /></button></div>}
              {documentState.refreshing && <p className="document-read-status" role="status"><LoaderCircle size={14} className="spin" />正在刷新，当前显示已读取版本。</p>}
              {docError && doc && <div className="inline-warning" role="status"><CircleAlert size={14} />{docError} 当前仍显示上次成功读取的版本。</div>}
              {docBusy ? <Empty icon={<LoaderCircle className="spin" size={25} />} title="正在读取说明" /> : docError && !doc ? <Empty icon={<BookOpen size={26} />} title="暂时无法读取说明"><p className="break-text">{docError}</p>{!selected.sourceId && <p>本地文件仍可在“文件”中查看；关联 GitHub 后可读取远端完整说明。</p>}{selected.sourceId && version === 'installed' && <Button onClick={() => { setVersion('latest'); setDocumentPath(undefined); setDocumentCommit(undefined); setDocumentLocal(false); }}><BookOpen size={14} />切换到最新说明</Button>}</Empty> : doc ? <><div className="document-caption"><span>{doc.documentPath || doc.source || 'Skill 文档'}{(doc.commit || documentCommit) && ` · ${(doc.commit || documentCommit)!.slice(0, 12)}`}</span><span>{doc.private && <LockKeyhole size={12} />} {dateText(doc.checkedAt)}</span></div><article className="markdown" data-outline-document={`${doc.documentPath || doc.source || selected.id}:${doc.commit || documentCommit || selected.commit || version}`}><ReactMarkdown remarkPlugins={[remarkGfm]} urlTransform={defaultUrlTransform} components={{
                a: ({ href, children }) => <a href={href || '#'} onClick={event => { event.preventDefault(); if (!href) return; if (/^https?:\/\//i.test(href)) { void external(href); return; } if (href.startsWith('#')) { if (paneRoot.current) scrollToPaneAnchor(paneRoot.current, decodeURIComponent(href.slice(1))); return; } try { const path = resolveDocPath(doc.documentPath || `${selected.remotePath || ''}/README.md`, href); if (/\.(md|markdown)$/i.test(path)) { setDocumentPath(path); setDocumentCommit(doc.commit || documentCommit); } else if (selected.repo) void external(`https://github.com/${selected.repo}/blob/${encodeURIComponent(doc.commit || documentCommit || (version === 'installed' ? selected.installedCommit || selected.commit || 'HEAD' : selected.commit || 'HEAD'))}/${path.split('/').map(encodeURIComponent).join('/')}`); else notify('info', '请在文件页或外部编辑器中打开该文件。'); } catch (error) { notify('error', errText(error)); } }}>{children}<ArrowUpRight size={11} className="inline-link-icon" /></a>,
                img: ({ src, alt }) => <AuthImage src={src} alt={alt} skill={selected} version={version} documentPath={doc.documentPath || `${selected.remotePath || ''}/README.md`} remoteCommit={doc.commit || documentCommit} localOrigin={documentLocal} />,
                pre: ({ children }) => <CodeBlock onCopy={text => void copy(text)}>{children}</CodeBlock>,
                ...markdownHeadings,
              }}>{doc.markdown}</ReactMarkdown></article></> : null}</>}
              {tab === 'files' && <>
                <div data-outline-key="inventory" data-outline-label="文件清单" data-outline-level="1" className="section-caption file-inventory-toolbar"><span>{files ? `本地 ${files.files.filter(file => localFile(file, selected)).length} 项${files.remoteStatus === 'ok' ? ` · GitHub ${files.files.filter(file => remoteFile(file, selected)).length} 项 · 远端独有 ${files.files.filter(file => !localFile(file, selected) && remoteFile(file, selected)).length} 项` : selected.sourceId ? ' · GitHub 待读取' : ''}` : '正在读取文件范围'}</span><Button variant="ghost" aria-label="刷新文件清单" disabled={busy || fileBusy} onClick={() => refreshInventory(selected.id)}><RefreshCw size={14} />刷新</Button>{selected.localPath && selected.sourceId && <Button variant="ghost" disabled={busy || selected.readOnly} onClick={() => void selectOperation(selected, 'push')}><Upload size={14} />上传本地修改</Button>}</div>
                {fileError && <div className="inline-error"><CircleAlert size={15} />{fileError}</div>}
                {files && <>
                  {fileBusy && selected.sourceId && <div className="helper-note" role="status"><LoaderCircle size={14} className="spin" />本地清单已显示，正在补充 GitHub 文件。</div>}
                  {files.remoteStatus === 'error' && <div className="inventory-remote-error" role="alert"><CircleAlert size={15} /><span>GitHub 文件读取失败，远端文件数量未知。{files.remoteError}</span><Button variant="ghost" disabled={busy || fileBusy} onClick={() => refreshInventory(selected.id)}>重试</Button></div>}
                  {files.remoteStatus === 'ok' && <div className="inventory-evidence"><Github size={13} /><span>固定提交 <span className="monospace" title={files.remoteCommit}>{files.remoteCommit?.slice(0, 12) || '未知'}</span> · {dateText(files.remoteCheckedAt)}</span></div>}
                  <div className="file-scope-summary" role="status"><ShieldCheck size={16} /><span>{inventoryFailed ? '文件读取未完成，请刷新核对；已有文件范围记录保持。' : !selected.localPath ? '按来源识别必要文件，下载时核对实际内容。' : files.runtimeConfirmed ? (files.runtimeConfirmationKind === 'automatic' ? `已自动识别 ${files.runtimeFiles?.length ?? selected.runtimeFiles?.length ?? runtimeFiles.length} 个包内管理文件` : `已保留文件范围 · ${files.runtimeFiles?.length ?? selected.runtimeFiles?.length ?? runtimeFiles.length} 个包内文件`) : files.runtimeAudit?.status === 'failed' ? '文件识别未完成，请查看下方具体问题。' : '已自动检查文件范围，仍有具体引用需要处理。'}<small>用于管理器检查、更新和上传；不是 Skill 的启用开关。</small></span><button className="text-button" onClick={() => setTab('source')}>查看比较依据<ChevronRight size={13} /></button></div>
                  {(exportSelection.error || exportPlanState.error) && <div className="inline-error" role="alert">{exportSelection.error || exportPlanState.error}<Button onClick={() => void recoverExport()}>重新核对导出清单</Button></div>}
                  {exportPlan && !exportPlan.canExport && <div className="inline-error" role="alert"><strong>导出前需要处理</strong>{exportPlan.issues.map((issue, index) => <p key={index}>{issue}</p>)}</div>}
                  <div className="export-file-summary" role="status">{exportPlanState.loading || exportPlanState.refreshing ? '正在核对导出清单，可继续浏览和选择。' : exportPlan ? `可选包内：已选 ${exportPlan.summary.optionalPackageSelected}／共 ${exportPlan.summary.optionalPackageTotal} 项 · 可选外部：已选 ${exportPlan.summary.optionalExternalSelected}／共 ${exportPlan.summary.optionalExternalTotal} 项` : '导出清单尚未读取'}</div>
                  <ExternalDependencies evidencePending={inventoryFailed || !!exportPlanState.error} layout="sections" runtimeIssues={files.issues ?? selected.runtimeIssues} items={files.externalDependencies || []} selectedIds={selectedExternalIds} onSelectionChange={() => undefined} exportResources={exportResources} onExportToggle={toggleExport} onCheck={() => void checkExternal(selected)} onBind={(id, role, path, options) => bindExternal(selected, id, role, path, options)} initialDraft={externalManagementDraft.current} onDraftChange={draft => { externalManagementDraft.current = draft; }} onBatch={batch => batchExternal(selected, batch)} onLocate={item => { const target = item.referenceKind === "package_file" || item.referenceKind === "skill_alias" ? item.packagePath || item.source : item.source; void readFile(target); }} busy={busy} readOnly={selected.readOnly} inventory={files.files.map(file => ({ ...file, localAvailable: localFile(file, selected), remoteAvailable: remoteFile(file, selected) }))} identity={{ localPath: selected.localPath, repo: selected.repo, remotePath: selected.remotePath, remoteCommit: files.remoteCommit }} onRevealFile={(path, referenceId) => {
                    const scope = paneRoot.current;
                    const row = Array.from(scope?.querySelectorAll<HTMLElement>('[data-file-path]') || []).find(node => node.dataset.filePath === path);
                    if (!row || !scope) return;
                    for (let parent = row.parentElement; parent && scope.contains(parent); parent = parent.parentElement) if (parent instanceof HTMLDetailsElement) { parent.dispatchEvent(new Event('workspace-expand')); parent.open = true; }
                    row.querySelector<HTMLDetailsElement>('[data-file-references]')?.setAttribute('open', '');
                    scrollToPaneAnchor(scope, `file:${path}`, 'instant', false);
                    if (referenceId) requestAnimationFrame(() => Array.from(row.querySelectorAll<HTMLElement>('[data-external-reference]')).find(node => node.dataset.externalReference === referenceId)?.focus({ preventScroll: true }));
                  }}>{({ referencesFor, toolbar, requiredExternal, optionalExternal, environment, counts }) => {
                    const missingRequired = exportResources.filter(resource => resource.group === 'required' && resource.id.startsWith('file:') && !files.files.some(file => resource.id === `file:${file.path}`));
                    const packageRow = (file: FileEntry, required: boolean) => <FileResourceRow evidencePending={inventoryFailed || !!exportPlanState.error} key={file.path} resource={exportResources.find(resource => resource.id === `file:${file.path}`)} required={required} title={file.path} filePath={file.path} choiceLabel={`加入 ZIP ${file.path}`} sizeText={bytes(file.size)} subtitle={required ? file.runtime && !requiredRuntimeFile(files, file.path) ? '已纳入文件范围' : requiredReason(files, file.path) : '可选说明或附加文件'} source={<>{!localFile(file, selected) && required ? 'GitHub · 导出时读取，本地未安装' : localFile(file, selected) && remoteFile(file, selected) ? '本地＋GitHub' : localFile(file, selected) ? '本地' : 'GitHub'}{file.contentDifferent && <span className="file-different"> · 内容不同，导出保留本地</span>}</>} onOpen={() => void readFile(file.path)} onToggle={toggleExport} disabled={busy || exportSelection.loading}>{referencesFor(file.path)}</FileResourceRow>;
                    return <>
                      <FileSection sectionKey="files-required" title="必要文件" count={`${runtimeInventory.length + missingRequired.length + counts.requiredExternal} 项 · 自动包含`} description="运行入口、已识别依赖和许可证默认随包；外部必要文件也在此统一列出。" initiallyOpen><div className="file-list">{runtimeInventory.map(file => packageRow(file, true))}{missingRequired.map(resource => <FileResourceRow evidencePending={inventoryFailed || !!exportPlanState.error} key={resource.id} resource={resource} required title={resource.path} filePath={resource.path} subtitle={resource.reason} sizeText={bytes(resource.size)}>{referencesFor(resource.path)}</FileResourceRow>)}{requiredExternal}{!runtimeInventory.length && !missingRequired.length && !counts.requiredExternal && <p className="runtime-group-empty">未识别到必要文件，请查看来源与具体问题。</p>}</div></FileSection>
                      <FileSection sectionKey="files-optional-package" title="可选包内文件" hasIssues={exportResources.some(resource => resource.group === 'optional_package' && resource.included && (resource.source === 'none' || resource.materializable === false))} count={`${optionalInventory.length} 项 · 已选 ${exportResources.filter(resource => resource.group === 'optional_package' && resource.included).length}`} description="按需勾选本次 ZIP 的附加内容，和导出页同步；不改变长期文件范围。"><div className="file-list">{optionalInventory.map(file => packageRow(file, false))}{!optionalInventory.length && <p className="runtime-group-empty">{files.remoteStatus === 'error' ? '本地没有其他可选文件；远端读取失败，情况未知。' : files.remoteStatus === 'ok' ? '本地与此 GitHub 提交均没有其他可选文件。' : selected.sourceId ? '本地没有其他可选文件；GitHub 清单仍待核对。' : '本地没有其他可选文件。'}</p>}</div></FileSection>
                      <FileSection sectionKey="files-optional-external" title="可选外部文件" hasIssues={exportResources.some(resource => resource.group === 'optional_external' && resource.included && (resource.source === 'none' || resource.materializable === false))} count={`${counts.optionalExternal} 项 · 已选 ${exportPlan?.summary.optionalExternalSelected || 0}`} description="可携带的外部文件按需加入本次 ZIP；原文件保持原位置。">{optionalExternal}{!counts.optionalExternal && <p className="runtime-group-empty">没有可选外部文件。</p>}</FileSection>
                      <FileSection sectionKey="environment" title="环境与参考" hasIssues={runtimeAttentionCount(files.issues ?? selected.runtimeIssues) > 0} count={`${counts.environment} 项引用`} description="命令、目录、环境要求和参考信息不直接加入 ZIP。可打包资源已归入上面的文件组。">{environment}<RuntimeChecks embedded references={files.externalDependencies || []} warnings={[...(files.warnings || []), ...(selected.runtimeWarnings || [])]} issues={files.issues ?? selected.runtimeIssues} /></FileSection>
                      <div className="file-export-footer"><span>必要文件默认包含，可选文件仅本次携带。</span><div><Button variant="primary" disabled={busy || !selected.localPath} onClick={() => void exportSkill(selected)}><Box size={15} />预览并导出 ZIP</Button><Button disabled={busy || !selected.localPath || selected.readOnly || !remoteExtras.length || !files.remoteCommit} onClick={() => void downloadAttachments(selected)} title="将所选 GitHub 包内附件保存到本地，与直接加入 ZIP 分开操作"><Download size={14} />下载到本地</Button></div></div>
                      <details className="file-management" data-fold-key="file-management"><summary>文件范围与来源管理</summary><p>必要文件默认自动识别。不需要逐个配置；只有特殊情况才调整长期文件范围，本次 ZIP 的勾选与此独立。</p>{selected.localPath && <Button disabled={busy || selected.readOnly} onClick={() => void manageRuntime(selected)}><Settings2 size={14} />调整文件范围</Button>}{toolbar}{autoMatchFeedback && <p className="inventory-evidence" role="status">{autoMatchFeedback}</p>}{!!externalBatchResults.length && <div className="external-batch-results" role="status"><strong>批量结果：{externalBatchResults.filter(item => item.status === 'matched' || item.status === 'success' || item.ok === true).length} / {externalBatchResults.length} 项完成</strong>{externalBatchResults.filter(item => item.error || ['needs_selection', 'missing', 'error'].includes(item.status || '')).map((item, index) => <p className="error-text" key={item.id || index}>{item.id}：{item.reason || item.error}</p>)}</div>}{(selected.localNotice || !!selected.untrackedFiles?.length) && <p className="helper-note">未纳入长期文件范围的本地文件继续保留；仅本次分享时，选择对应可选文件即可。</p>}</details>
                    </>;
                  }}</ExternalDependencies>
                  {fileContent && <div className="file-reader" data-outline-key="file-preview" data-outline-label="文件预览"><div><strong>{fileContent.path}</strong><span className="mini-tag">{fileContent.source === 'local' ? '本地内容' : `GitHub · ${fileContent.remoteCommit?.slice(0, 12) || '未知'}`}</span><button className="icon-button" onClick={() => void copy(fileContent.content)} aria-label="复制文件内容"><Copy size={14} /></button></div>{currentFile && <div className="file-source-switch segmented small"><button aria-label="本地版本" disabled={!localFile(currentFile, selected)} className={fileContent.source === 'local' ? 'selected' : ''} onClick={() => void readFile(fileContent.path, 'local')}>本地版本</button><button aria-label="GitHub 版本" disabled={!remoteFile(currentFile, selected) || !files.remoteCommit} className={fileContent.source === 'github' ? 'selected' : ''} onClick={() => void readFile(fileContent.path, 'github')}>GitHub 版本</button></div>}<pre>{fileContent.content}</pre></div>}
                </>}
              </>}
              {tab === 'source' && <><ComparisonBasis key={selected.id} skill={selected} /><dl className="info-grid" data-outline-key="source" data-outline-label="来源与比较记录"><dt>本地目录</dt><dd className="monospace">{selected.localPath ? <button className="text-button detail-source-link" title={selected.localPath} disabled={busy} onClick={() => void openFolder(selected)}><FolderOpen size={14} /><span>{selected.localPath}</span><ExternalLink size={12} /></button> : '尚未下载到本地'}</dd><dt>GitHub 仓库</dt><dd>{selected.repo ? <button className="text-button" onClick={() => void external(`https://github.com/${selected.repo}`)}>{selected.repo}<ExternalLink size={12} /></button> : '未关联'}{selected.private && <span className="mini-tag"><LockKeyhole size={11} />私有</span>}</dd><dt>仓库路径</dt><dd className="monospace">{selected.repo ? <button className="text-button detail-source-link" title={remoteDirectoryUrl(selected)} onClick={() => void external(remoteDirectoryUrl(selected))}><Github size={13} />{selected.remotePath || '仓库根目录'}<ExternalLink size={12} /></button> : '未关联'}</dd><dt>分支</dt><dd className="monospace">{selected.branch || '未知'}</dd><dt>比较基准</dt><dd className="monospace">{selected.baselineCommit || '待核对当前内容'}</dd><dt>历史安装提交</dt><dd className="monospace">{selected.installedCommit || '未知 · 没有可核实的安装记录'}</dd><dt>远端提交</dt><dd className="monospace">{selected.commit || '未知'}</dd><dt>最近本地检查</dt><dd>{selected.lastLocalCheckedAt ? dateText(selected.lastLocalCheckedAt) : '未知'}</dd><dt>最近远端检查</dt><dd>{selected.lastRemoteCheckedAt ? dateText(selected.lastRemoteCheckedAt) : '未知'}</dd></dl><SkillInformation readGeneration={state.readGeneration} sourceKey={JSON.stringify([selected.sourceId, selected.repo, selected.remotePath, selected.branch])} skillId={selected.id} localDependencies={selected.dependencies} localVerification={selected.verification} remoteCommit={selected.commit} onReadOriginal={(path, commit, source) => { setDocumentPath(path); setDocumentCommit(commit); setDocumentLocal(source === 'local'); setVersion('latest'); setTab('docs'); }} />
              <div className="section-caption"><strong data-outline-key="clients" data-outline-label="客户端连接">本机调用验证与客户端连接</strong><Button variant="ghost" onClick={() => { setView('clients'); }}><Link2 size={14} />管理连接</Button></div>{!selectedConnections.length ? <p className="muted small-text">还没有通过管理器建立连接。</p> : selectedConnections.map(connection => <div className="connection-summary" key={connection.id}><Monitor size={16} /><span><strong>{CLIENTS.find(client => client.id === connection.client)?.name || connection.client}</strong><small>{connection.path}</small></span><span className={`badge ${connection.linked ? 'green' : 'amber'}`}>{connection.health === 'ready' ? '连接正常' : '需处理'}</span></div>)}
              <div className="source-actions">{selected.localPath && !selected.sourceId && <Button onClick={() => openAssociate(selected)} disabled={busy || selected.readOnly || !state.sources.length}><Link2 size={14} />关联 GitHub</Button>}<Button variant="ghost" disabled={busy || selectedConnections.length > 0} title={selectedConnections.length ? '请先断开此 Skill 的客户端连接' : '仅移除管理记录，保留原文件'} onClick={() => setModal({ kind: 'forget', skill: selected })}><Ellipsis size={15} />移除管理记录</Button></div>{selected.readOnly && <p className="helper-note"><LockKeyhole size={14} />应用托管目录采用只读管理，请通过所属应用更新。</p>}</>}
            </div>
          </>}</section>;

  const paneDialogs = <>
    {modal?.kind === 'client-result' && <ModalFrame title="连接操作结果" subtitle={modal.mode === 'stop' ? '停止管理' : '断开连接'} onClose={closeModal} busy={busy} footer={<><Button onClick={closeModal}>关闭</Button>{modal.results.some(item => !!item.error) && <Button onClick={() => { const ids = new Set(modal.results.filter(item => item.error).map(item => item.id)); void previewManageConnections(state.connections.filter(connection => ids.has(connection.id)), modal.mode); }}>重新预览失败项</Button>}</>}>
      <p>成功 {modal.results.filter(item => item.success && !item.error).length}／共 {modal.results.length} 项。Skill 原文件保持原位。</p><div className="connection-preview-list">{modal.results.map(result => <article className="connection-preview-entry" key={result.id}><strong>{result.label}</strong><p className={result.error ? 'error-text' : undefined}>{result.error || '已完成'}</p></article>)}</div>
    </ModalFrame>}

    {modal?.kind === 'client-connect' && <ClientConnectPage skills={localSkills} connections={state.connections} client={modal.client} initialDraft={clientConnectDraft.current} onDraftChange={draft => { clientConnectDraft.current = draft; }} onConnect={connectSkills} onClose={closeModal}
      pendingBatch={resumingIds.length > 0} resumedBatch={(() => { const task = [...resumedTasks].reverse().find(task => task.action === 'connect_batch' && task.origin.clientId === modal.client && !!task.result); const value = task?.result as { connectionResults?: ConnectionResult[] } | undefined; return task && value?.connectionResults ? { taskId: task.id, client: modal.client, results: value.connectionResults } : undefined; })()} />}

    {modal?.kind === 'category' && <ModalFrame title={modal.category ? '编辑分类' : '创建分类'} subtitle="分类只整理管理记录，不会移动或修改 Skill 文件。" onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" disabled={busy || !categoryName.trim()} onClick={() => void saveCategory()}>{modal.category ? '保存分类' : '创建分类'}</Button></>}><label className="field-label">分类名称<input value={categoryName} maxLength={60} placeholder="例如：科研、程序开发、日常工具" onChange={event => setCategoryName(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && categoryName.trim()) void saveCategory(); }} aria-label="分类名称" /></label><CategoryColorPicker value={categoryDraftColor} onChange={setCategoryDraftColor} disabled={busy} /></ModalFrame>}
    {modal?.kind === 'delete-category' && <ModalFrame title="移除分类" subtitle={modal.category.name} onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button onClick={() => void removeCategory()} disabled={busy}>移除分类</Button></>}><p>此分类包含 {state.skills.filter(skill => skill.categoryId === modal.category.id).length} 个 Skill。移除后，它们会转为“未分类”。</p><p>Skill 管理记录、原文件和客户端连接继续保留。</p></ModalFrame>}
    {modal?.kind === 'task' && (() => { const task = tasks.find(item => item.id === modal.task.id) || modal.task; const result = modal.task.result as { path?: string; summary?: string; auditSummary?: AuditSummary; transferResult?: TransferResult; connectionResults?: ConnectionResult[] } | undefined; return <ModalFrame title="任务记录" subtitle={task.label} onClose={closeModal} busy={false} footer={<><Button onClick={() => { void locateTask(task).catch(error => notify('error', errText(error))); }}>返回对应界面</Button>{task.cancellable && !terminalTask(task) && <Button onClick={() => void cancelTask(task.id)}>取消任务</Button>}<Button onClick={closeModal}>关闭</Button></>}><dl className="info-grid compact"><dt>状态</dt><dd>{taskStatus[task.status] || task.status}</dd><dt>阶段</dt><dd>{task.stage}</dd><dt>开始时间</dt><dd>{dateText(task.startedAt || task.createdAt)}</dd><dt>结束时间</dt><dd>{task.finishedAt ? dateText(task.finishedAt) : '尚未结束'}</dd></dl>{task.error && <div className="inline-error">{task.error}</div>}{result?.path && <p>输出：<code>{result.path}</code></p>}{result?.summary && <p>{result.summary}</p>}{result?.auditSummary && <div className="audit-items">{result.auditSummary.items.map(item => <p key={item.skillId}>{state.skills.find(skill => skill.id === item.skillId)?.name || item.skillId}：{item.status} {item.reason}</p>)}</div>}{result?.connectionResults && <div className="client-batch-results">{result.connectionResults.map(item => <p key={item.id}>{item.label}：{item.error || '连接成功'}</p>)}</div>}{result?.transferResult && <TransferNotice result={result.transferResult} onClose={closeModal} />}<p className="muted small-text">点击记录只查看结果或返回界面。更新预览需要重新生成；不会重复执行上传、下载或删除。</p>{task.resultExpired && <p className="helper-note">这是重启前的记录，临时预览已过期。请返回原界面重新检查。</p>}</ModalFrame>; })()}
    {modal?.kind === 'local' && <ModalFrame title="添加本地 Skill" subtitle="选择一个 Skill 或包含多个 Skill 的父目录，先预览再登记。" onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" disabled={busy || !scanSelected.length} onClick={() => void addLocal()}><Plus size={16} />登记 {scanSelected.length || ''} 个 Skill</Button></>}><label className="field-label">本地目录<div className="input-action"><input value={scanPath} onChange={event => { setScanPath(event.target.value); setScanResult(null); setScanSelected([]); }} placeholder="输入目录绝对路径" /><Button onClick={() => void chooseLocal()} disabled={busy}><FolderOpen size={16} />选择</Button><Button onClick={() => void scanLocal()} disabled={busy || !scanPath.trim()}><Search size={15} />扫描</Button></div></label><div className="helper-note"><Link2 size={15} />登记后直接使用原目录，不迁移或复制已有 Skill。</div>{scanResult && <div className="scan-result"><div className="section-caption"><strong>发现 {scanResult.skills.length} 个 Skill</strong><span>{scanSelected.length} 个已选择</span></div>{scanResult.warnings?.map((warning, index) => <div className="inline-warning" key={index}><CircleAlert size={15} />{warning}</div>)}{!scanResult.skills.length ? <Empty title="没有发现 SKILL.md"><p>请选择正确目录，或检查文件夹中的 Skill 结构。</p></Empty> : scanResult.skills.map((skill, index) => { const path = skill.localPath || skill.path || ''; return <label className="scan-row" key={path || index}><input type="checkbox" checked={scanSelected.includes(path)} disabled={busy || !path} onChange={event => setScanSelected(previous => event.target.checked ? [...previous, path] : previous.filter(item => item !== path))} /><div><strong>{skillName(skill)}</strong><span>{path}</span>{skill.description && <p>{skill.description}</p>}</div>{skill.readOnly && <span className="mini-tag"><LockKeyhole size={11} />只读</span>}</label>; })}</div>}</ModalFrame>}
    {modal?.kind === 'github' && <ModalFrame title="添加 GitHub 来源" subtitle="读取仓库中的 Skill 和完整说明，不立即下载运行文件。" onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" disabled={busy || !githubUrl.trim()} onClick={() => void addGithub()}><Github size={16} />读取来源</Button></>}><label className="field-label">仓库地址<input value={githubUrl} onChange={event => setGithubUrl(event.target.value)} placeholder="https://github.com/owner/repository" autoCapitalize="none" spellCheck={false} onKeyDown={event => { if (event.key === 'Enter' && githubUrl.trim()) void addGithub(); }} /></label><div className="helper-note"><LockKeyhole size={16} />私有仓库请先在“设置”中保存该仓库的访问令牌。</div><p className="muted small-text">支持标准 SKILL.md 目录及带 skill-catalog.json 的仓库。将核对真实内部标识与目录或运行内容，明确对应时自动关联。</p></ModalFrame>}
    {modal?.kind === 'associate' && <ModalFrame title="关联 GitHub 来源" subtitle={`为 ${skillName(modal.skill)} 指定远端位置。相同名称不代表相同来源。`} onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" onClick={() => void associate()} disabled={busy || !associateSkill}><Link2 size={15} />确认关联</Button></>}><label className="field-label">GitHub 来源<select value={associateSource} onChange={event => { setAssociateSource(event.target.value); setAssociateSkill(''); }}><option value="">选择仓库</option>{state.sources.map(source => <option key={source.id} value={source.id}>{source.repo}</option>)}</select></label><label className="field-label">远端 Skill<select value={associateSkill} onChange={event => setAssociateSkill(event.target.value)}><option value="">选择对应的 Skill 目录</option>{sourceSkill.map(skill => <option key={skill.id} value={skill.id}>{skillName(skill)} — {skill.remotePath || '/'}</option>)}</select></label><dl className="info-grid compact"><dt>本地目录</dt><dd className="monospace">{modal.skill.localPath}</dd><dt>远端目录</dt><dd className="monospace">{state.skills.find(skill => skill.id === associateSkill)?.remotePath ?? '待选择'}</dd></dl><div className="helper-note"><CircleHelp size={15} />关联后先核对实际内容；不会自动覆盖文件或把当前内容认定为最新版本。</div></ModalFrame>}
    {modal?.kind === 'runtime' && <ModalFrame title="调整文件范围" subtitle={skillName(modal.skill)} onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" disabled={busy || !selectedFiles.length} onClick={() => void confirmRuntime()}><Check size={15} />确认运行文件</Button></>}>
      <div className="helper-note"><ShieldCheck size={15} />这里调整管理器检查、更新和上传的长期文件范围，不是 Skill 的启用步骤。默认已自动识别；手动保存后保留你的选择。原文件继续留在原位置，ZIP 附件在导出页选择。</div>
      {runtimeGroups(modal.result).map(group => <RuntimeFileGroup key={`manage:${modal.skill.id}:${group.required}`} required={group.required} total={group.files.length} selected={group.files.filter(file => selectedFiles.includes(file.path)).length} description={group.required ? '必要入口、引用资源和许可证必须保留。' : '仅在实际运行需要或长期随包保留时纳入运行清单。'}><div className="file-checklist runtime-checklist">{group.files.map(file => <label className="file-check" key={file.path}><input type="checkbox" aria-label={`将 ${file.path} 纳入运行清单`} checked={selectedFiles.includes(file.path)} disabled={busy || group.required} onChange={event => setSelectedFiles(previous => event.target.checked ? [...new Set([...previous, file.path])] : previous.filter(path => path !== file.path))} /><span className="runtime-file-description"><span>{file.path}</span><small className="runtime-file-reason">{group.required ? requiredReason(modal.result, file.path) : '本地可选文件'}</small></span><em>{bytes(file.size)}</em></label>)}</div>{!group.files.length && <p className="runtime-group-empty">本地没有此类文件。</p>}</RuntimeFileGroup>)}
      <RuntimeWarnings warnings={modal.result.warnings} issues={modal.result.issues} />
    </ModalFrame>}
    {modal?.kind === 'export' && <ModalFrame title="导出 Skill" subtitle={`${modal.skill.name}.zip`} onClose={closeModal} busy={busy} error={modalError} footer={<><Button onClick={closeModal} disabled={busy}>取消</Button><Button variant="primary" onClick={() => void saveExport()} disabled={busy || choosingDestination || preparingDestination || !destinationReady(destinationChoice, destinationDefaultPath) || exportSelection.pending || !exportPlanState.current || !exportPlan?.canExport} title={!exportPlanState.current ? '等待当前选择对应的清单核验完成' : !exportPlan?.canExport ? '请先处理必要或已选文件的问题' : '按当前清单导出 ZIP'}><Box size={15} />导出 ZIP</Button></>}>
      <DestinationChoice kind="export" value={destinationChoice} defaultPath={destinationDefaultPath} loading={destinationLoading} error={destinationError} busy={busy || choosingDestination || preparingDestination} onChange={changeDestination} onChoose={() => void chooseDestination()} onRetry={() => setDestinationRefresh(value => value + 1)} />
      <ExportSelection plan={exportPlan} selectedIds={exportSelection.selectedIds} updating={exportPlanState.loading || exportPlanState.refreshing || exportSelection.loading} busy={busy || exportSelection.pending} error={exportSelection.error || exportPlanState.error} onToggle={toggleExport} onSet={setExportChoices} onRefresh={() => void recoverExport()} view="export" />
      <div className="helper-note"><ShieldCheck size={15} />与文件页共享本次选择。远端资源直接进入 ZIP，原文件保持原位置；本次选择不改变运行清单、长期用途或比较基准。</div>
    </ModalFrame>}
    {modal?.kind === 'files' && <ModalFrame title={modal.operation === 'install' ? '选择下载范围' : modal.operation === 'update' ? '确认更新文件范围' : modal.operation === 'upload' ? '首次上传到 GitHub' : '选择上传范围'} subtitle={skillName(modal.skill)} onClose={closeModal} busy={busy} error={modalError} footer={<><Button disabled={busy} onClick={closeModal}>取消</Button><Button variant="primary" onClick={() => void filePreview()} disabled={busy || !selectedFiles.length || modal.operation === 'install' && (choosingDestination || preparingDestination || !destinationReady(destinationChoice, destinationDefaultPath)) || !['push', 'upload'].includes(modal.operation) && !runtimeConsent || modal.operation === 'upload' && (!uploadSource || !uploadBranch.trim() || !uploadPath.trim())}><ArrowLeftRight size={15} />预览{modal.operation === 'install' ? '下载' : modal.operation === 'update' ? '更新' : '上传'}</Button></>}>
      {modal.operation === 'upload' && <div className="upload-destination"><label className="field-label">上传到仓库<select aria-label="上传目标仓库" value={uploadSource} onChange={event => { const source = state.sources.find(item => item.id === event.target.value); setUploadSource(event.target.value); setUploadBranch(source?.branch || 'main'); }}><option value="">选择已配置的 GitHub 来源</option>{state.sources.map(source => <option key={source.id} value={source.id}>{source.repo}{source.private ? ' · 私有' : ''}</option>)}</select></label><label className="field-label">目标分支<input aria-label="上传目标分支" value={uploadBranch} onChange={event => setUploadBranch(event.target.value)} spellCheck={false} /></label><label className="field-label">仓库内 Skill 目录<input aria-label="上传目标目录" value={uploadPath} onChange={event => setUploadPath(event.target.value)} spellCheck={false} /></label>{!state.sources.length && <div className="helper-note">请先在设置中添加 GitHub 来源并保存具有 Contents 写入权限的令牌。</div>}</div>}
      {['push', 'upload'].includes(modal.operation) && <label className="consent"><input type="checkbox" checked={trimOptional} onChange={event => setTrimOptional(event.target.checked)} /><span>上传后仅在 GitHub 保留本次选中的可选文件。预览会列出本地删除路径；必要文件、校验失败或上传期间改动的文件保留。</span></label>}
      {modal.operation === 'install' && <DestinationChoice kind="download" value={destinationChoice} defaultPath={destinationDefaultPath} loading={destinationLoading} error={destinationError} busy={busy || choosingDestination || preparingDestination} onChange={changeDestination} onChoose={() => void chooseDestination()} onRetry={() => setDestinationRefresh(value => value + 1)} />}
      <div className="section-caption"><strong>{selectedFiles.length} / {modal.result.files.length} 个文件已选择</strong><Button variant="ghost" onClick={() => setSelectedFiles(['push', 'upload'].includes(modal.operation) ? requiredExportPaths(modal.result) : modal.result.files.filter(file => file.runtime || requiredRuntimeFile(modal.result, file.path) || modal.result.suggestedFiles?.includes(file.path)).map(file => file.path))} disabled={busy}>恢复建议范围</Button></div>
      <div className="runtime-scope-note"><ShieldCheck size={15} /><span>{['push', 'upload'].includes(modal.operation) ? '本次只提交勾选文件；运行依赖分类供参考，不强制全部提交。' : '所选范围用于下载、更新和导出 ZIP，不会删除未选的本地文件。'}</span></div>
      <div className="file-checklist runtime-checklist">{runtimeGroups(modal.result).map(group => <RuntimeFileGroup key={`modal:${modal.skill.id}:${modal.operation}:${group.required}`} required={group.required} total={group.files.length} selected={group.files.filter(file => selectedFiles.includes(file.path)).length} description={group.required ? ['push', 'upload'].includes(modal.operation) ? '本次提交仍可独立选择文件。' : '必需文件已勾选，包含已识别的运行依赖。' : '按需加入完整说明或其他附加内容。'}>{group.files.map(file => { const required = modal.operation !== 'push' && group.required; return <label key={file.path}><input type="checkbox" checked={selectedFiles.includes(file.path)} disabled={busy || required} onChange={event => setSelectedFiles(previous => event.target.checked ? [...previous, file.path] : previous.filter(item => item !== file.path))} /><FileCode2 size={14} /><span className="runtime-file-description"><span>{file.path}</span><small className="runtime-file-reason">{group.required ? requiredReason(modal.result, file.path) : '可选说明或附加文件'}</small></span>{file.missing && <small className="change-deleted">本地已移除</small>}{required && <small>必要</small>}<em>{bytes(file.size)}</em></label>; })}{!group.files.length && <p className="runtime-group-empty">{group.required ? '未识别到必要文件。' : '没有其他可选文件。'}</p>}</RuntimeFileGroup>)}</div>
      {!['push', 'upload'].includes(modal.operation) && !modal.result.runtimeConfirmed && <label className="consent"><input type="checkbox" checked={runtimeConsent} onChange={event => setRuntimeConsent(event.target.checked)} /><span>我已核对 SKILL.md、必要依赖与许可证；勾选的可选说明也会随包保留。</span></label>}{['push', 'upload'].includes(modal.operation) && <div className="helper-note"><ShieldCheck size={15} />仅提交选定变化。未下载的远端 README 等文件不会因本地缺失而被删除。</div>}
    </ModalFrame>}
    {modal?.kind === 'preview' && <ModalFrame title={['push', 'upload'].includes(modal.preview.kind) ? '预览上传文件' : modal.preview.kind === 'disconnect' ? modal.preview.mode === 'stop' ? '预览忽略连接' : '预览断开连接' : modal.preview.kind === 'restore' ? '预览恢复附件' : modal.preview.kind.includes('install') ? '预览下载 Skill' : modal.preview.kind === 'attachments' ? '预览下载附件' : '预览更新内容'} subtitle={modal.preview.summary} onClose={closeModal} busy={busy} error={modalError} footer={<><span className="preview-apply-note">{modal.preview.canApply ? modal.preview.kind === 'attachments' ? '仅下载预览中的缺失文件，不覆盖已有文件' : '应用前将再次核对文件和远端是否变化' : '当前预览不可应用，请先处理提示'}</span><Button onClick={closeModal} disabled={busy}>关闭</Button><Button variant="primary" onClick={() => void applyPreview()} disabled={busy || !modal.preview.canApply || !modal.preview.id || ['push', 'upload'].includes(modal.preview.kind) && !commitMessage.trim()}>{['push', 'upload'].includes(modal.preview.kind) ? <Upload size={15} /> : <ArrowDownToLine size={15} />}{['push', 'upload'].includes(modal.preview.kind) ? '确认上传' : modal.preview.kind === 'disconnect' ? modal.preview.mode === 'stop' ? '确认忽略连接' : '确认断开连接' : modal.preview.kind === 'restore' ? '确认恢复附件' : modal.preview.kind.includes('install') ? '确认下载' : modal.preview.kind === 'attachments' ? '确认下载附件' : '应用更新'}</Button></>}>
      {modal.preview.kind !== 'disconnect' && <dl className="info-grid compact"><dt>当前 Skill</dt><dd>{state.skills.find(skill => skill.id === modal.preview.skillId) ? skillName(state.skills.find(skill => skill.id === modal.preview.skillId)!) : modal.preview.skillId}</dd>{modal.preview.targetPath && <><dt>保存目录</dt><dd className="monospace">{modal.preview.targetPath}</dd></>}{modal.preview.commit && <><dt>远端版本</dt><dd className="monospace">{modal.preview.commit}</dd></>}</dl>}
      {(modal.preview.targetRepo || modal.preview.targetBranch) && <dl className="info-grid compact"><dt>上传目标仓库</dt><dd>{modal.preview.targetRepo}</dd><dt>目标分支</dt><dd>{modal.preview.targetBranch}</dd></dl>}
      {modal.preview.kind === 'disconnect' && <>
        <div className="connection-preview-list">{(modal.preview.entries || [{ connectionId: 'single', label: modal.preview.skillId, path: modal.preview.path, target: modal.preview.target, mode: modal.preview.mode || 'disconnect', canApply: modal.preview.canApply }]).map(entry => <article key={entry.connectionId} className="connection-preview-entry"><strong>{entry.label}</strong><span>{CLIENT_MANAGEMENT_COPY[entry.mode].label}</span><dl className="info-grid compact"><dt>客户端入口</dt><dd><code>{entry.path}</code></dd><dt>Skill 原目录</dt><dd><code>{entry.target}</code></dd></dl>{entry.error && <p className="error-text">{entry.error}</p>}</article>)}</div>
        <div className="helper-note"><ShieldCheck size={15} />{modal.preview.mode === 'stop' ? '保留客户端入口和原文件；自动发现将忽略这一条入口。' : '仅解除列出的目录链接，Skill 原目录和文件继续保留。'}入口身份或目标变化后，预览自动失效。</div>
      </>}
      {!!modal.preview.cleanupFiles?.length && <section className="cleanup-preview"><h3>上传并校验成功后，从本地删除以下可选文件</h3><p>这些文件保留在本次 GitHub 提交，可从“恢复附件”取回。实际删除前会再次检查必要性与本地内容。</p><ul>{modal.preview.cleanupFiles.map(file => <li key={file.path}><code>{file.absolutePath}</code><small>{file.reason}</small></li>)}</ul></section>}
      {modal.preview.warnings?.map((warning, index) => <div className="inline-warning" key={index}><CircleAlert size={16} />{warning}</div>)}{modal.preview.kind !== 'disconnect' && <div className="preview-layout"><div className="preview-file-list">{modal.preview.files?.map(file => <button className={previewFile === file.path ? 'active' : ''} key={file.path} onClick={() => setPreviewFile(file.path)}><span>{file.path}</span><small className={`change-${file.change}`}>{CHANGES[file.change] || file.change}</small></button>)}</div><div className="diff-view">{(() => { const file = modal.preview.files?.find(item => item.path === previewFile); return !file ? <Empty title="没有文件变化" /> : <><div className="diff-title"><FileCode2 size={14} />{file.path}<span className={`change-${file.change}`}>{CHANGES[file.change] || file.change}</span></div><div className="diff-columns"><div><strong>本地内容</strong><pre>{file.local ?? (file.change === 'added' ? '本地尚无此文件' : '无文本预览，或文件内容未变化')}</pre></div><div><strong>远端内容</strong><pre>{file.remote ?? (file.change === 'deleted' ? '远端已移除此文件' : '无文本预览，或文件内容未变化')}</pre></div></div></>; })()}</div></div>}{['push', 'upload'].includes(modal.preview.kind) && <label className="field-label">提交说明<input value={commitMessage} onChange={event => setCommitMessage(event.target.value)} placeholder="描述这次选定修改" maxLength={500} /></label>}{!modal.preview.canApply && <div className="helper-note"><Code2 size={16} />请按提示核对来源或处理本地修改，然后重新生成预览。关闭窗口会保留所有原文件。</div>}
    </ModalFrame>}
    {modal?.kind === 'audit' && <ModalFrame title="运行依赖与比较基准核对" subtitle={`本轮核对 ${modal.summary.checked} 个 Skill`} onClose={closeModal} busy={busy} error={modalError} footer={<Button onClick={closeModal}>关闭</Button>}>
      <div className="audit-counts"><span><strong>{modal.summary.passed}</strong> 结构核对通过</span><span><strong>{modal.summary.needsReview}</strong> 需要处理</span><span><strong>{modal.summary.failed}</strong> 检查失败</span></div>
      <p className="muted small-text">明确项目已自动记录运行范围；完整同范围内容一致才建立比较基准。历史安装版本和实际调用验证分别保留。</p>
      <div className="audit-items">{modal.summary.items.map(item => <div key={item.skillId}><strong>{skillName(state.skills.find(skill => skill.id === item.skillId) || { id: item.skillId, name: item.skillId })}</strong><span>{item.status === 'passed' ? '结构核对通过' : item.status === 'failed' ? '检查失败' : '需要处理'}</span>{item.reason && <p>{item.reason}</p>}<Button variant="ghost" onClick={() => { setModal(null); setView('skills'); openDetail(state.skills.find(skill => skill.id === item.skillId)!, 'files'); }}>查看文件与结果</Button></div>)}</div>
    </ModalFrame>}
    {modal?.kind === 'receipts' && <ModalFrame title="从 GitHub 恢复附件" subtitle={skillName(modal.skill)} onClose={closeModal} busy={busy} error={modalError} footer={<Button onClick={closeModal}>关闭</Button>}>
      <p className="muted small-text">按上传时的固定提交恢复当前缺失的文件；预览后出现同名本地文件会停止应用。恢复不属于 Ctrl＋Z 的分类与收藏范围。</p>
      {!modal.receipts.length ? <Empty title="还没有上传后精简记录" /> : modal.receipts.map(receipt => <div className="restore-receipt" key={receipt.id}><strong>{dateText(receipt.createdAt)}</strong><code>{receipt.commit}</code><ul>{receipt.files?.map(file => <li key={file.path}>{file.path}</li>)}</ul><Button disabled={busy} onClick={() => void previewRestore(modal.skill, receipt.id)}><Undo2 size={14} />预览恢复附件</Button></div>)}
    </ModalFrame>}
    {modal?.kind === 'forget' && <ModalFrame title="移除管理记录" subtitle={skillName(modal.skill)} onClose={closeModal} busy={busy} error={modalError} footer={<><Button disabled={busy} onClick={closeModal}>取消</Button><Button disabled={busy} onClick={async () => { const result = await perform<Snapshot>('正在移除管理记录', 'forget', { skillId: modal.skill.id }, '管理记录已移除，原文件未删除。'); if (result) { setModal(null); setDetailOpen(false); } }}>确认移除记录</Button></>}><p>这会从管理器列表中移除此项，其原文件和 GitHub 内容继续保留。</p><div className="path-note">{modal.skill.localPath || `${modal.skill.repo}/${modal.skill.remotePath || ''}`}</div></ModalFrame>}
  </>;
  if (!isHome) return <div className="session-document" ref={paneRoot} data-session-pane={pane.id} data-read-generation={state.readGeneration ?? 'unknown'}>
    <PaneFrame paneId={pane.id} outline={pane.kind !== 'history'} outlineExpanded={outlineExpanded} onOutlineExpandedChange={setOutlineExpanded} initialScrollTop={Number(session.initialDraft.scrollTop ?? pane.state.scrollTop ?? 0)} onScrollPosition={saveScroll} toolbar={<><strong title={pane.title || 'Skill'}>{pane.title || 'Skill'}</strong><button className="button ghost" onClick={() => sessionRef.current.goHome()}>返回工作台</button></>}>
      {notices[0] && <WorkspaceFeedback message={notices[0].message} tone={notices[0].tone} onDismiss={() => setNotices(previous => previous.slice(1))} onOpenHistory={() => sessionRef.current.openHistory()} />}
      {pane.kind === 'operation' && transferResult && <TransferNotice result={transferResult} onClose={() => setTransferResult(null)} />}{pane.kind === 'history' ? <OperationHistory tasks={tasks} initialDraft={historyDraft.current} onDraftChange={draft => { historyDraft.current = draft; }} onInspect={openTask} onLocate={locateTask} onCancel={task => cancelTask(task.id).then(() => undefined)} onClear={() => clearTasks().then(() => undefined)} /> : pane.kind === 'detail' ? <>{detailPanel}{modal && ['category', 'delete-category', 'forget'].includes(modal.kind) && paneDialogs}</> : modal ? paneDialogs : <div className="restored-operation"><h2>重新核对操作</h2><p>操作页已恢复。请重新读取当前文件或生成预览，核对后再执行。</p><Button onClick={() => { const skill = state.skills.find(item => item.id === pane.skillId); if (skill) openDetail(skill, 'files'); else sessionRef.current.goHome(); }}>查看当前文件</Button></div>}
      {resumedTasks.map(task => <div className="resumed-task-result" key={task.id}><strong>{task.label} · {taskStatus[task.status]}</strong>{!!task.result && typeof task.result === "object" && "path" in task.result && <code>{String(task.result.path)}</code>}<Button onClick={() => setModal({ kind: 'task', task })}>查看结果</Button>{!!task.result && typeof task.result === 'object' && 'canApply' in task.result && <Button onClick={() => showPreview(task.result as Preview)}>查看预览</Button>}</div>)}
    </PaneFrame>
    {floatingMenu && <FloatingMenu menu={floatingMenu} onClose={closeFloatingMenu} />}
  </div>;

  return <div ref={paneRoot} data-session-pane={pane.id} data-read-generation={state.readGeneration ?? 'unknown'} className={`app-shell ${compactSidebar ? 'sidebar-collapsed' : ''} ${sidebarFlyout && compactSidebar ? 'sidebar-flyout' : ''}`} style={{ '--sidebar-width': `${compactSidebar ? 72 : expandedSidebarWidth}px`, '--sidebar-expanded-width': `${expandedSidebarWidth}px` } as React.CSSProperties}>
    <aside className="sidebar" ref={sidebarRef}>
      <div className="sidebar-top"><div className="brand"><span className="brand-icon"><Layers3 size={23} strokeWidth={1.8} /></span><div><strong>Skill Manager</strong></div></div><button className="icon-button sidebar-toggle" title={compactSidebar && !sidebarFlyout ? '展开侧栏' : '收起侧栏'} aria-label={compactSidebar && !sidebarFlyout ? '展开侧栏' : '收起侧栏'} aria-expanded={!compactSidebar || sidebarFlyout} onClick={() => { if (effectiveWidth < 720) setSidebarFlyout(value => !value); else toggleSidebar(); }}>{compactSidebar ? <PanelLeftOpen size={19} /> : <PanelLeftClose size={19} />}</button></div>
      
      <nav aria-label="主要导航">
        {([{ id: 'skills', label: 'Skills', icon: <Box size={19} />, count: state.skills.length }, { id: 'changes', label: '变化', icon: <ArrowLeftRight size={19} />, count: changedSkills.length || undefined }, { id: 'clients', label: '客户端', icon: <Monitor size={19} /> }, { id: 'settings', label: '设置', icon: <Settings2 size={19} /> }] as const).map(item => <button key={item.id} title={item.label} aria-label={item.label} className={`nav-item ${view === item.id ? 'active' : ''}`} onClick={() => { setView(item.id); setAddMenu(false); setSidebarFlyout(false); }}>{item.icon}<span>{item.label}</span>{'count' in item && item.count !== undefined && <span className="nav-count">{item.count}</span>}</button>)}
      </nav>
      <button className="icon-button category-rail-toggle" title="打开分类" aria-label="打开分类" aria-expanded={sidebarFlyout} onClick={() => setSidebarFlyout(value => !value)} onDragEnter={event => { if (event.dataTransfer.types.includes('application/x-skill-manager-skills')) { clearTimeout(dragCategoryTimer.current); dragCategoryTimer.current = setTimeout(() => setSidebarFlyout(true), 350); } }} onDragOver={event => { if (event.dataTransfer.types.includes('application/x-skill-manager-skills')) event.preventDefault(); }} onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node)) clearTimeout(dragCategoryTimer.current); }}><Folder size={20} /></button>
      {categoryPanel}
      <Button className="management-undo" variant="ghost" aria-label="撤销管理操作" title={managementBusy ? '正在保存管理操作，完成后可撤销' : state.managementUndo?.available ? `撤销${state.managementUndo.label || '管理操作'} (Ctrl+Z)` : '暂无可撤销的分类或收藏操作'} disabled={busy || managementBusy || !state.managementUndo?.available} onClick={() => void undoManagement()}><Undo2 size={14} />撤销</Button>
      <OperationSummary tasks={tasks} visible={session.visible} onShowAll={() => sessionRef.current.openHistory()} onDetails={openTask} onReturn={locateTask} />
      
    </aside>
    {!compactSidebar && <div className={`sidebar-resizer ${resizingSidebar ? 'is-resizing' : ''}`} role="separator" aria-label="调整侧栏宽度" aria-orientation="vertical" aria-valuemin={200} aria-valuemax={Math.floor(Math.min(420, effectiveWidth - 520))} aria-valuenow={Math.round(expandedSidebarWidth)} tabIndex={0} title="拖动调整侧栏宽度；双击恢复默认" onDoubleClick={() => { setSidebarWidth(224); queueLayout({ sidebarWidth: 224 }); }} onKeyDown={event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); resizeSidebar(expandedSidebarWidth + (event.key === 'ArrowRight' ? 10 : -10), true); } else if (event.key === 'Home') { event.preventDefault(); setSidebarWidth(224); queueLayout({ sidebarWidth: 224 }); } }} onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); resizeStart.current = { x: event.clientX, width: expandedSidebarWidth, originalWidth: sidebarWidth }; queueLayout({ sidebarWidth }); setResizingSidebar(true); }} onPointerMove={event => { if (resizeStart.current) resizeSidebar(resizeStart.current.width + (event.clientX - resizeStart.current.x) / (currentLayout.current.zoom / 100), true); }} onPointerUp={event => { if (!resizeStart.current) return; resizeStart.current = null; setResizingSidebar(false); queueLayout({ sidebarWidth: currentLayout.current.width }); void flushLayout(); event.currentTarget.releasePointerCapture(event.pointerId); }} onPointerCancel={cancelSidebarResize} onLostPointerCapture={cancelSidebarResize} />}
    <main className="main" data-pane-scroll onScroll={event => { if (!homeRestorePending.current && event.currentTarget.clientHeight > 0) saveScroll(event.currentTarget.scrollTop); }}>
      <header className="page-header"><h1>{title}</h1><div className="header-actions">{view !== 'clients' && view !== 'settings' && <Button onClick={() => void refresh()} disabled={busy || !ready}><RefreshCw size={16} className={busy ? 'spin' : ''} />检查更新</Button>}{(view === 'skills' || view === 'changes') && <Button disabled={busy || !state.skills.length} onClick={() => void auditSkills()}><ShieldCheck size={15} />核对运行依赖</Button>}{view === 'skills' && <div className="add-menu"><Button variant="primary" onClick={() => setAddMenu(!addMenu)} disabled={busy}><Plus size={17} />添加 Skill</Button>{addMenu && <><button className="menu-dismiss" aria-label="关闭添加菜单" onClick={() => setAddMenu(false)} /><div className="dropdown"><button onClick={() => { setModal({ kind: 'local' }); setAddMenu(false); }}><FolderOpen size={17} /><span>添加本地目录<small>直接读取原位置</small></span></button><button onClick={() => { setModal({ kind: 'github' }); setAddMenu(false); }}><Github size={17} /><span>添加 GitHub 来源<small>读取仓库与说明</small></span></button></div></>}</div>}</div></header>
      {notices[0] && <WorkspaceFeedback message={notices[0].message} tone={notices[0].tone} onDismiss={() => setNotices(previous => previous.slice(1))} onOpenHistory={() => sessionRef.current.openHistory()} />}
      {!detailVisible && transferResult && <TransferNotice result={transferResult} onClose={() => setTransferResult(null)} />}


      {view === 'skills' && <>
        <div className="library-summary"><span><span className="summary-number">{state.skills.length}</span> 全部 Skill</span><span title="已有本地运行目录的 Skill，包含已关联项"><Folder size={15} /><strong>{localSkills.length}</strong> 本地</span><span title="已登记的 GitHub Skill 条目，包含已关联项"><Github size={15} /><strong>{libraryTotals.github}</strong> 远端 Skill</span><span title="同时具有本地目录与 GitHub 来源"><Link2 size={15} /><strong>{libraryTotals.linked}</strong> 已关联</span><span className="repository-total" title="按仓库地址去重统计，不是 Skill 数量"><GitBranch size={15} /><strong>{libraryTotals.repositories}</strong> GitHub 仓库</span></div>
        {(categoryFilter !== 'all' || categoryUndo) && <div className="category-toolbar"><div className="category-context"><Folder size={17} /><strong title={categoryTitle}>{categoryTitle}</strong><span>· {categoryTotal} 个 Skill</span>{categoryFilter !== 'all' && <button className="text-button" onClick={() => selectCategory('all')}>查看全部</button>}</div>{categoryUndo && <Button className="category-undo" variant="ghost" disabled={busy} onClick={() => void undoCategory()}><Undo2 size={14} />撤销归类</Button>}</div>}
        {categoryFeedback && <div className={`category-assignment-feedback ${categoryFeedback.error ? 'error' : ''}`} role={categoryFeedback.error ? 'alert' : 'status'}><span>{categoryFeedback.message}</span>{!categoryFeedback.error && categoryUndo && <button className="text-button" disabled={busy} onClick={() => void undoCategory()}><Undo2 size={13} />撤销归类</button>}{categoryFeedback.targetId && <button className="text-button" onClick={() => selectCategory(categoryFeedback.targetId!)}>查看分类</button>}<button className="icon-button" aria-label="关闭归类提示" onClick={() => setCategoryFeedback(null)}><X size={13} /></button></div>}
        {(query || filter !== 'all' || statusFilter !== 'all') && <div className="active-filter-chips" aria-label="当前筛选">{query && <button className="filter-chip" onClick={() => setQuery('')}>搜索：{query}<X size={12} /></button>}{filter !== 'all' && <button className="filter-chip" onClick={() => setFilter('all')}>来源：{filter === 'local' ? '本地' : filter === 'github' ? 'GitHub' : '已关联'}<X size={12} /></button>}{statusFilter !== 'all' && <button className="filter-chip" onClick={() => setStatusFilter('all')}>状态：{statusFilterLabel(statusFilter)}<X size={12} /></button>}</div>}
        <div className={`library-panel library-${libraryMode}`}>
          <div className="library-controls">
            <div className="library-search-row"><label className="search library-search"><Search size={17} /><input value={query} onChange={event => setQuery(event.target.value)} placeholder={`搜索${categoryFilter === 'all' ? '全部 Skill' : '当前分类'}：名称、用途、仓库、路径…`} aria-label="搜索 Skill" />{query && <button className="icon-button" aria-label="清空搜索" onClick={() => setQuery('')}><X size={15} /></button>}</label><div className="segmented library-mode" aria-label="技能库视图"><button className={libraryMode === 'list' ? 'selected' : ''} aria-pressed={libraryMode === 'list'} disabled={busy} onClick={() => void saveLibraryPreference({ libraryMode: 'list' })}><List size={16} />列表</button><button className={libraryMode === 'cards' ? 'selected' : ''} aria-pressed={libraryMode === 'cards'} disabled={busy} onClick={() => void saveLibraryPreference({ libraryMode: 'cards' })}><LayoutGrid size={16} />卡片</button><button className={libraryMode === 'details' ? 'selected' : ''} aria-pressed={libraryMode === 'details'} disabled={busy} onClick={() => void saveLibraryPreference({ libraryMode: 'details' })}><LayoutPanelLeft size={16} />详情</button></div></div>
            <div className="library-filter-row"><div className="segmented source-filters" aria-label="来源筛选">{([{ id: 'all', label: '全部' }, { id: 'local', label: '本地' }, { id: 'github', label: 'GitHub' }, { id: 'linked', label: '已关联' }] as const).map(item => <button key={item.id} className={filter === item.id ? 'selected' : ''} aria-pressed={filter === item.id} onClick={() => setFilter(item.id)}>{item.label}</button>)}</div><label className="compact-select"><span>状态</span><select aria-label="按状态筛选" value={statusFilter} onChange={event => setStatusFilter(event.target.value as LibraryStatusFilter)}>{libraryStatusOptions(state.skills, { categoryFilter, sourceFilter: filter, query, statusFilter }).map(option => <option key={option.value} value={option.value}>{option.label} ({option.count})</option>)}</select></label><label className="compact-select library-sort"><span>排序</span><select aria-label="Skill 排序" value={librarySort} disabled={busy} onChange={event => void saveLibraryPreference({ librarySort: event.target.value as LibrarySort })}><option value="name">名称 A–Z</option><option value="status">状态优先</option><option value="checked">最近检查</option></select></label>{(query || filter !== 'all' || statusFilter !== 'all') && <button className="text-button clear-filters" onClick={clearLibraryFilters}><X size={13} />清除筛选</button>}</div>
            {ready && matching.length > 0 && libraryMode !== 'details' && <div className="library-selection-bar">
              <span><strong>{libraryResult.total}</strong> 项结果{visibleChecked.length > 0 && ` · 已选 ${visibleChecked.length} 项（仅本页）`}</span>
              <ShortcutHelp mode={libraryMode === 'cards' ? 'cards' : 'list'} visible={session.visible} />
              {libraryMode === 'cards' && <button className="text-button" disabled={busy} onClick={() => { setCheckedSkills(visibleChecked.length === pageSkillIds.length ? [] : pageSkillIds); selectionAnchor.current = pageSkillIds[0] || null; }}>{visibleChecked.length === pageSkillIds.length ? '取消本页选择' : '选择本页全部'}</button>}
              {visibleChecked.length > 0 && <div className="library-batch-actions">{categoryAssignmentMenu}<Button disabled={busy} onClick={() => void checkSelected()}><RefreshCw size={14} />检查所选 ({visibleChecked.length})</Button><Button variant="ghost" onClick={() => setCheckedSkills([])} disabled={busy}>取消选择</Button></div>}
            </div>}
          </div>
          {!ready ? <Empty icon={<LoaderCircle className="spin" size={28} />} title="正在读取技能库" /> : !state.skills.length ? <Empty title="从你的第一个 Skill 开始"><p>连接本地文件夹，或添加 GitHub 仓库。<br />已有文件留在原位置，由你决定每次变化。</p><div className="empty-actions"><Button variant="primary" onClick={() => setModal({ kind: 'local' })}><FolderOpen size={16} />添加本地目录</Button><Button onClick={() => setModal({ kind: 'github' })}><Github size={16} />连接 GitHub</Button></div><span className="microcopy">本地管理无需登录 GitHub</span></Empty> : !categoryTotal ? <Empty icon={<Folder size={27} />} title={categoryFilter === 'favorites' ? '还没有收藏 Skill' : '分类中还没有 Skill'}><p>{categoryFilter === 'favorites' ? '点击 Skill 名称旁的星标即可收藏，原分类保持不变。' : '从“全部 Skills”拖入 Skill，或勾选后使用“归入分类”。'}</p><Button variant="ghost" onClick={() => selectCategory('all')}>浏览全部 Skills</Button></Empty> : !matching.length ? <Empty icon={<Search size={27} />} title="当前筛选无结果"><p>当前分类中有 {categoryTotal} 个 Skill，试试其他关键词、来源或状态。</p><Button variant="ghost" onClick={clearLibraryFilters}>清除筛选</Button></Empty> : libraryMode === 'list' ? <>
            <div className="skill-table-scroll" ref={tableRef} tabIndex={0} role="region" aria-label="可横向滚动的 Skill 列表" onKeyDown={tableKeys}>
              <table className="skill-table"><thead><tr><th className="select-column"><input type="checkbox" aria-label="选择本页全部 Skill" checked={!!pageSkillIds.length && visibleChecked.length === pageSkillIds.length} disabled={busy} onChange={event => { setCheckedSkills(event.target.checked ? pageSkillIds : []); selectionAnchor.current = pageSkillIds[0] || null; }} /></th><th scope="col">名称与用途</th><th scope="col">来源</th><th scope="col">状态</th><th scope="col">版本</th><th scope="col" className="actions-column">操作</th></tr></thead>
              <tbody>{libraryResult.items.map(skill => <tr key={skill.id} data-skill-id={skill.id} tabIndex={0} className={`skill-table-row ${selectedId === skill.id ? 'is-current' : ''} ${visibleChecked.includes(skill.id) ? 'is-checked' : ''}`} draggable={!busy && visibleChecked.includes(skill.id)} onPointerDownCapture={dragPointer} onDragStart={event => startSkillDrag(event, skill)} onDragEnd={endSkillDrag} onClick={() => { if (Date.now() >= dragSuppressedUntil.current) setSelectedId(skill.id); }} onFocus={event => { if (event.target === event.currentTarget) setSelectedId(skill.id); }}>
                <td className="select-column"><input type="checkbox" draggable={false} aria-label={`选择 ${skillName(skill)}`} checked={visibleChecked.includes(skill.id)} disabled={busy} onClick={event => { event.stopPropagation(); chooseRow(skill.id, event.currentTarget.checked, event.shiftKey); }} onChange={() => {}} /></td>
                <td className="skill-name-cell skill-drag-zone"><button className="icon-button skill-drag-handle" draggable={!busy} disabled={busy} aria-label={`拖动 ${skillName(skill)} 归类`} title="拖入侧栏分类；勾选后拖动选中行可批量归类" onClick={event => event.stopPropagation()} onDragStart={event => startSkillDrag(event, skill)} onDragEnd={endSkillDrag}><GripVertical size={16} /></button>
                  <div className="skill-name-heading"><button className="skill-name-button" title={skillName(skill)} onClick={() => openDetail(skill)}>{skill.readOnly ? <LockKeyhole size={15} /> : <FileCode2 size={15} />}<strong>{skillName(skill)}</strong></button><button className={`icon-button favorite-button ${skill.favorite ? 'is-favorite' : ''}`} draggable={false} aria-label={`${skill.favorite ? '取消收藏' : '收藏'} ${skillName(skill)}`} aria-pressed={!!skill.favorite} title={skill.favorite ? '取消收藏；原分类保持不变' : '收藏此 Skill'} disabled={busy} onClick={event => { event.stopPropagation(); void toggleFavorite(skill); }}><Star size={15} fill={skill.favorite ? 'currentColor' : 'none'} /></button>{skill.categoryId && !categories.some(item => item.id === categoryFilter) && <CategoryChip category={categories.find(category => category.id === skill.categoryId)} color={resolvedCategoryColors.get(skill.categoryId)} className="table-category-chip" />}</div>
                  <span className="skill-table-description" title={skill.description || undefined}>{skill.description || ''}</span>
                </td>
                <td className="skill-source-cell">{skill.repo && <button className="source-path-link" draggable={false} title={remoteDirectoryUrl(skill)} onClick={event => { event.stopPropagation(); void external(remoteDirectoryUrl(skill)); }}><Github size={13} /><span>{skill.repo}{skill.remotePath ? `/${skill.remotePath}` : ''}</span>{skill.private && <LockKeyhole size={10} />}</button>}{skill.localPath && <button className={`source-path-link local-source-link ${skill.repo ? 'is-icon' : ''}`} draggable={false} title={skill.localPath} aria-label="打开本地文件夹" disabled={busy} onClick={event => { event.stopPropagation(); void openFolder(skill); }}><FolderOpen size={13} />{!skill.repo && <span>{skill.localPath}</span>}</button>}{!skill.repo && !skill.localPath && <span className="muted">未知来源</span>}</td>
                <td className="skill-status-cell"><Badge skill={skill} />{['changed', 'review', 'problem'].includes(displayStatus(skill).group) && <span className="table-row-notice status-detail" title={statusExplanation(skill)}>{skill.error ? skill.error.split('\n')[0] : displayStatus(skill).detail}</span>}{!skill.error && (!!runtimeAttentionCount(skill.runtimeIssues) || !!skill.localNotice || !!skill.untrackedFiles?.length) && <span className="table-row-notice" title={skill.localNotice || '详情中有待核对的信息'}><CircleHelp size={11} />{skill.untrackedFiles?.length ? `额外文件 ${skill.untrackedFiles.length}` : '待核对'}</span>}</td>
                <td className="skill-version-cell" title={skill.version || '来源未提供版本信息'}>{skill.version ? `v${skill.version.replace(/^v/, '')}` : <span aria-label="版本未知">—</span>}</td>
                <td className="skill-row-actions"><div>{skill.localPath && <Button className="table-action" draggable={false} onClick={event => { event.stopPropagation(); void perform('正在打开编辑器', 'open_local', { skillId: skill.id }); }} disabled={busy || skill.readOnly} title={`在外部编辑器中编辑 ${skillName(skill)}`}><Code2 size={14} />编辑</Button>}{skill.sourceId && <Button className="table-action" draggable={false} onClick={event => { event.stopPropagation(); void (skill.localPath ? previewUpdate(skill) : selectOperation(skill, 'install')); }} disabled={busy || skill.readOnly || skill.remoteMissing} title={skill.localPath ? '先预览差异，再决定是否应用更新' : '选择运行文件并预览下载'}><Download size={14} />{skill.localPath ? '更新' : '下载'}</Button>}<Button className="table-action view-action" draggable={false} variant="ghost" onClick={event => { event.stopPropagation(); openDetail(skill); }}>查看<ChevronRight size={14} /></Button></div></td>
              </tr>)}</tbody></table>
            </div>
          </> : libraryMode === 'cards' ? <LibraryCards items={libraryResult.items} selectedIds={visibleChecked} categories={categories} showCategory={!categories.some(item => item.id === categoryFilter)} onChoose={chooseRow} onSelectAll={() => { setCheckedSkills(pageSkillIds); selectionAnchor.current ||= pageSkillIds[0] || null; }} onOpenSkill={openDetail} onToggleFavorite={skill => void toggleFavorite(skill)} onDragStart={(event, skill) => { blockedDrag.current = false; startSkillDrag(event, skill); }} onDragEnd={endSkillDrag} disabled={busy} connectionCount={skill => state.connections.filter(item => item.skillId === skill.id && item.linked).length} renderActions={skill => <div>{skill.localPath && <Button className="table-action" draggable={false} onClick={event => { event.stopPropagation(); void perform('正在打开编辑器', 'open_local', { skillId: skill.id }); }} disabled={busy || skill.readOnly} title={`在外部编辑器中编辑 ${skillName(skill)}`}><Code2 size={14} />编辑</Button>}{skill.sourceId && <Button className="table-action" draggable={false} onClick={event => { event.stopPropagation(); void (skill.localPath ? previewUpdate(skill) : selectOperation(skill, 'install')); }} disabled={busy || skill.readOnly || skill.remoteMissing} title={skill.localPath ? '先预览差异，再决定是否应用更新' : '选择运行文件并预览下载'}><Download size={14} />{skill.localPath ? '更新' : '下载'}</Button>}<Button className="table-action view-action" draggable={false} variant="ghost" onClick={event => { event.stopPropagation(); openDetail(skill); }}>查看<ChevronRight size={14} /></Button></div>} renderSource={skill => <>{skill.repo && <button className="source-path-link" draggable={false} title={remoteDirectoryUrl(skill)} onClick={event => { event.stopPropagation(); void external(remoteDirectoryUrl(skill)); }}><Github size={13} /><span>{skill.repo}{skill.remotePath ? `/${skill.remotePath}` : ''}</span>{skill.private && <LockKeyhole size={10} />}</button>}{skill.localPath && <button className={`source-path-link local-source-link ${skill.repo ? 'is-icon' : ''}`} draggable={false} title={skill.localPath} aria-label="打开本地文件夹" disabled={busy} onClick={event => { event.stopPropagation(); void openFolder(skill); }}><FolderOpen size={13} />{!skill.repo && <span>{skill.localPath}</span>}</button>}{!skill.repo && !skill.localPath && <span className="muted">未知来源</span>}</>} /> : <div className="library-body library-detail-layout"><section className="skill-list" aria-label="Skill 列表">{libraryResult.items.map(skill => <div className="detail-row-drag-handle" key={skill.id}><button className="icon-button skill-drag-handle" draggable={!busy} disabled={busy} aria-label={`拖动 ${skillName(skill)} 归类`} title="拖入侧栏分类" onDragStart={event => startSkillDrag(event, skill)} onDragEnd={endSkillDrag}><GripVertical size={16} /></button><button className={`skill-row ${selectedId === skill.id ? 'selected' : ''}`} onClick={() => { setSelectedId(skill.id); setTab('docs'); }}><div className="skill-row-top"><span className={`skill-avatar ${skill.readOnly ? 'readonly' : ''}`}>{skill.readOnly ? <LockKeyhole size={18} /> : <FileCode2 size={18} />}</span><div className="skill-row-name"><strong>{skillName(skill)}</strong></div><ChevronRight size={15} className="row-chevron" /></div>{skill.description && <p>{skill.description}</p>}<div className="skill-row-bottom"><Badge skill={skill} />{skill.categoryId && !categories.some(item => item.id === categoryFilter) && <CategoryChip category={categories.find(category => category.id === skill.categoryId)} color={resolvedCategoryColors.get(skill.categoryId)} />}<span className="row-origin">{skill.localPath && <Folder size={12} />}{skill.sourceId && <Github size={12} />}{skill.version ? `v${skill.version.replace(/^v/, '')}` : ''}</span></div>{skill.error && <div className="row-error"><CircleAlert size={12} />检查失败</div>}</button></div>)}</section>{detailPanel}</div>}
          {!!state.skills.length && <div className="library-pagination"><span className="library-result-count" role="status">第 {libraryResult.start}–{libraryResult.end} 项 / {libraryResult.total} 项{matching.length !== state.skills.length && `（全库 ${state.skills.length} 项）`}</span><label className="compact-select page-size-select"><span>每页</span><select aria-label="每页 Skill 数量" value={libraryPageSize} disabled={busy} onChange={event => void saveLibraryPreference({ libraryPageSize: Number(event.target.value) as LibraryPageSize })}><option value={20}>20 项</option><option value={50}>50 项</option><option value={100}>100 项</option></select></label><div className="page-navigation"><Button disabled={libraryResult.page <= 1} aria-label="上一页" onClick={() => setLibraryPage(libraryResult.page - 1)}><ChevronLeft size={15} /></Button><span>{libraryResult.page} / {libraryResult.pageCount} 页</span><Button disabled={libraryResult.page >= libraryResult.pageCount} aria-label="下一页" onClick={() => setLibraryPage(libraryResult.page + 1)}><ChevronRight size={15} /></Button></div></div>}
        </div>
        {libraryMode === 'list' && detailOpen && selected && <DetailDrawer onClose={closeDetail} suspended={!!modal} feedback={<div className="drawer-feedback" aria-live="polite">{busy && <div className="operation-bar"><LoaderCircle size={15} className="spin" />{operation}</div>}{notices.filter(notice => notice.id >= detailOpenedAt.current && notice.skillId === selected.id).slice(0, 2).map(notice => <div className={`notice ${notice.tone}`} key={notice.id}><span>{notice.message}</span><button className="icon-button" aria-label="关闭详情消息" onClick={() => setNotices(previous => previous.filter(item => item.id !== notice.id))}><X size={14} /></button></div>)}</div>}>{detailPanel}</DetailDrawer>}
      </>}

      {view === 'changes' && <ChangesPanel skills={state.skills} sourceErrors={remoteErrors} busy={busy}
        onOpen={(skill, section) => openDetail(skill, section)} onCompare={previewUpdate}
        onUpload={skill => selectOperation(skill, 'push')} onDocs={skill => openDetail(skill, 'docs')}
        onFiles={skill => openDetail(skill, 'files')}
        onCheck={skill => perform('正在检查 Skill', 'refresh', { skillId: skill.id })}
        onCheckSource={() => refresh()} />}

      {view === 'clients' && <ClientPanel onOpenSkill={id => { const skill = state.skills.find(item => item.id === id); if (skill) openDetail(skill); }} skills={localSkills} connections={state.connections} client={activeClient} onClientChange={setActiveClient}
        onCheck={async () => { const value = await perform<Snapshot>('正在检查客户端连接', 'refresh_clients'); const ids = value?.connections.filter(connection => connection.autoRepairable).map(connection => connection.id) || []; if (ids.length) await perform('正在安全修复客户端入口', 'repair_client_connections', { connectionIds: ids }); }}
        onOpenConnect={client => setModal({ kind: 'client-connect', client })}
        onManage={(connections, mode) => { void previewManageConnections(connections, mode); }}
        onConfirmUsage={async connection => { const value = await perform('正在记录已成功使用', 'confirm_client_usage', { connectionId: connection.id, client: connection.client }); if (!value) throw new Error('使用记录未保存，请查看操作记录。'); return value; }} />}
      {view === 'settings' && <SettingsLayout paneId={pane.id} scopeRef={paneRoot} expanded={settingsOutline} onExpandedChange={setSettingsOutline}>
        <SettingsSection sectionKey="general" title="使用偏好"><section className="card"><div className="card-header"><div><h2>使用偏好</h2></div><SlidersHorizontal size={20} className="muted" /></div><div className="settings-form"><label>外部编辑器<div className="input-action"><input value={editorPath} placeholder="留空使用系统默认编辑器" onChange={event => setEditorPath(event.target.value)} /><Button onClick={async () => { try { const result = await open({ directory: false, multiple: false, title: '选择外部编辑器程序' }); if (typeof result === 'string') setEditorPath(result); } catch (error) { notify('error', errText(error)); } }}>选择</Button></div></label><div className="settings-readonly"><strong>自动检查</strong><p>启动时检查，此后每 30 分钟一次</p><small>管理器运行时生效；返回窗口后补查本地变化。</small></div><div className="form-actions"><Button variant="primary" disabled={busy || !ready} onClick={() => void perform('正在保存设置', 'set_settings', { settings: { checkIntervalMinutes: 30, editorPath: editorPath.trim(), uiTheme, uiFontSize } }, '设置已保存。')}><Check size={15} />保存设置</Button></div></div></section></SettingsSection>
        <SettingsSection sectionKey="display" title="显示"><section className="card display-settings"><div className="card-header"><div><h2>显示设置</h2><p>缩放自动保存；主题和字号预览后点击保存。</p></div><Monitor size={20} className="muted" /></div><div className="settings-form">
        <fieldset className="theme-fieldset"><legend>界面模式</legend><div className="theme-options">{THEMES.map(theme => <button type="button" key={theme.id} className={`theme-option ${uiTheme === theme.id ? 'selected' : ''}`} aria-pressed={uiTheme === theme.id} onClick={() => setUiTheme(theme.id)}><span className={`theme-swatch theme-swatch-${theme.id}`} aria-hidden="true"><i /><i /><i /></span><span><strong>{theme.label}</strong><small>{theme.description}</small></span>{uiTheme === theme.id && <CheckCircle2 size={17} />}</button>)}</div></fieldset>
        <div className="display-controls"><label>界面缩放<div className="zoom-control"><input type="range" min="80" max="160" step="1" value={uiZoom} onChange={event => changeZoom(Number(event.target.value))} aria-label="界面缩放比例" /><div className="number-suffix"><input type="number" min="80" max="160" step="1" value={zoomInput} onChange={event => { setZoomInput(event.target.value); const value = Number(event.target.value); if (event.target.value && value >= 80 && value <= 160) { const zoom = Math.round(value); currentLayout.current.zoom = zoom; setUiZoom(zoom); queueLayout({ uiZoom: zoom }); } }} onBlur={() => changeZoom(zoomInput.trim() ? Number(zoomInput) : 100)} onKeyDown={event => { if (event.key === 'Enter') changeZoom(zoomInput.trim() ? Number(zoomInput) : 100); }} aria-label="自定义界面缩放百分比" /><span>%</span></div></div><small>80%–160%，自动保存。Ctrl＋滚轮或 Ctrl＋加减号调整，Ctrl＋0 恢复 100%。</small></label><label>基础字号<select value={uiFontSize} onChange={event => setUiFontSize(Number(event.target.value))} aria-label="基础字号">{Array.from({ length: 9 }, (_, index) => index + 14).map(size => <option value={size} key={size}>{size} px{size === 16 ? '（默认）' : ''}</option>)}</select><small>14–22 px。独立调整文字大小，不改变界面缩放比例。</small></label></div>
        <div className="display-preview"><div><FileCode2 size={18} /><strong>显示预览</strong><span>{THEMES.find(theme => theme.id === uiTheme)?.label} · {uiZoom}% · {uiFontSize} px</span></div><p>Skill 说明与文件内容</p><small>说明文字与次要信息</small></div>
        <div className="form-actions"><Button variant="primary" disabled={busy || !ready || !displayChanged} onClick={() => void perform('正在保存显示设置', 'set_settings', { settings: { checkIntervalMinutes: 30, editorPath: state.settings.editorPath || '', uiTheme, uiFontSize } }, '显示设置已保存，下次启动会继续使用。')}><Check size={15} />保存显示设置</Button><Button variant="ghost" onClick={resetDisplay} disabled={busy}>恢复显示默认值</Button><Button variant="ghost" onClick={resetLayout} disabled={!ready}>恢复默认布局</Button><span className="display-save-hint" role="status">{displayChanged ? '主题和字号正在预览，尚未保存' : '主题和字号已保存；缩放自动保存'}</span></div>
      </div></section></SettingsSection>
        <SettingsSection sectionKey="storage" title="保存位置"><StorageSettings request={request} onRequestRestart={session.requestStorageRestart} initialDraft={storageDraft.current} onDraftChange={draft => { storageDraft.current = draft; }} /></SettingsSection>
        <SettingsSection sectionKey="network" title="网络"><NetworkSettings request={request} notify={notify} onContextChange={refreshReadContext} initialDraft={networkDraft.current} onDraftChange={draft => { networkDraft.current = draft; }} /></SettingsSection>
        <SettingsSection sectionKey="github" title="GitHub"><section className="card"><div className="card-header"><div><h2>GitHub 访问凭据</h2><p>按仓库保存到系统凭据管理器。</p></div><LockKeyhole size={20} className="muted" /></div><div className="settings-form"><label>仓库<input value={tokenRepo} list="known-repositories" placeholder="owner/repository" onChange={event => { setTokenRepo(event.target.value); setTokenLinkStatus(''); }} autoCapitalize="none" spellCheck={false} /><datalist id="known-repositories">{state.sources.map(source => <option key={source.id} value={source.repo} />)}</datalist></label><label>准备使用的权限<select value={tokenAccess} onChange={event => setTokenAccess(event.target.value as 'read' | 'write')}><option value="read">读取、检查和下载</option><option value="write">读取并提交选定修改</option></select><small>在 GitHub 创建仅授权目标仓库的细粒度令牌：Contents 选择 {tokenAccess === 'read' ? 'Read-only' : 'Read and write'}。此选择只提示所需权限，不修改令牌权限。</small></label><label>访问令牌<input type="password" value={token} onChange={event => setToken(event.target.value)} placeholder="输入后保存到系统凭据管理器" autoComplete="new-password" spellCheck={false} /><small>保存后清空输入；令牌不进入配置文件、项目或日志。</small></label><div className="form-actions"><Button variant="primary" disabled={busy || !tokenRepo.trim() || !token.trim()} onClick={async () => { setTokenLinkStatus('正在保存到系统凭据管理器…'); const result = await perform('正在保存仓库凭据', 'set_token', { repo: tokenRepo.trim(), token: token.trim() }, '凭据已保存到系统凭据管理器。'); setToken(''); if (result) { setTokenRepo(tokenRepo.trim()); setTokenLinkStatus('凭据已保存，可检查目标仓库以确认权限。'); } else setTokenLinkStatus('凭据保存未完成，请查看左侧操作记录中的异常。'); }}><LockKeyhole size={15} />保存凭据</Button><Button disabled={busy || !tokenRepo.trim()} onClick={async () => { setTokenLinkStatus('正在移除系统凭据…'); const result = await perform('正在移除仓库凭据', 'delete_token', { repo: tokenRepo.trim() }, '该仓库的系统凭据已移除。'); setToken(''); setTokenLinkStatus(result ? '该仓库的系统凭据已移除。' : '凭据移除未完成，请查看左侧操作记录中的异常。'); }}>移除凭据</Button><Button variant="ghost" onClick={() => void createTokenLink()}><ExternalLink size={14} />创建令牌</Button><Button variant="ghost" onClick={async () => { try { await navigator.clipboard.writeText(tokenCreationUrl(tokenRepo, tokenAccess)); setTokenLinkStatus('创建链接已复制，请在浏览器地址栏粘贴打开。'); } catch (error) { setTokenLinkStatus(`复制失败：${errText(error)}`); } }}><Copy size={14} />复制创建链接</Button></div><p className="muted small-text">这里使用访问令牌，不是 SSH 公钥。创建页选择 Only select repositories，仅勾选上方仓库。生成后将令牌粘贴到这里，再保存凭据；不要发送到聊天中。</p>{tokenLinkStatus && <p className="helper-note" role="status">{tokenLinkStatus}</p>}</div></section><section className="card sources-card"><div className="card-header"><h2 data-outline-key="settings:github-sources" data-outline-label="仓库来源" data-outline-level="2">GitHub 来源</h2><div className="form-actions"><Button disabled={busy || !ready} onClick={() => void refresh()}><RefreshCw size={15} />检查来源</Button><Button variant="ghost" disabled={busy} onClick={() => setModal({ kind: 'github' })}><Plus size={15} />添加来源</Button></div></div>{!state.sources.length ? <p className="muted settings-empty">尚未添加来源，本地 Skill 可独立管理。</p> : state.sources.map(source => <div className="source-item" key={source.id}><Github size={19} /><div><strong>{source.repo}</strong><span>{source.branch || '默认分支'} · {dateText(source.lastChecked)}</span><span>监视目录：{source.roots?.length ? source.roots.map(root => root === '' ? '整个仓库' : root).join('、') : '尚未确定'}</span>{source.rootsInferred && <span>监视范围由已有记录推断；如需调整，可重新添加对应的 GitHub URL。</span>}{source.error && <p className="error-text">{source.error}</p>}</div>{source.private && <span className="mini-tag"><LockKeyhole size={11} />私有</span>}<button className="icon-button" aria-label="打开仓库" onClick={() => void external(`https://github.com/${source.repo}`)}><ExternalLink size={15} /></button></div>)}</section></SettingsSection>
      </SettingsLayout>}
      <footer className="page-footer"><span>v{packageInfo.version}</span></footer>
    </main>

    {(zoomFeedback || layoutError) && <div className="zoom-feedback" role={layoutError ? 'alert' : 'status'}>{layoutError || zoomFeedback}{layoutError && <button className="icon-button" aria-label="关闭界面设置错误提示" onClick={() => setLayoutError('')}><X size={14} /></button>}</div>}
    {paneDialogs}
    {floatingMenu && <FloatingMenu menu={floatingMenu} onClose={closeFloatingMenu} />}
  </div>;
}





