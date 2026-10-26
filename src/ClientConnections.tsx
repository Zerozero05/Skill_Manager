import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { ChevronDown, ChevronLeft, ChevronRight, Info, Plus, RefreshCw, Search } from 'lucide-react';
import { CLIENT_MANAGEMENT_COPY, CLIENT_OPTIONS, clientConnectionRows, clientSelectionRange, clientSelectionShortcut, clientSkillName, clientUsageText, selectFilteredClientItems, type ClientConnection, type ClientManagementMode, type ClientSkill, type ConnectionResult } from './client-selection';
import './clients.css';

type Props = {
  skills: ClientSkill[]; connections: ClientConnection[]; client: string;
  onClientChange: (client: string) => void; onCheck: () => Promise<unknown>;
  onOpenConnect?: (client: string) => void;
  onOpenSkill?: (skillId: string) => void;
  onManage?: (connections: ClientConnection[], mode: ClientManagementMode) => void;
  onConfirmUsage?: (connection: ClientConnection) => Promise<unknown>;
  onConnect?: (client: string, skillIds: string[]) => Promise<ConnectionResult[]>;
  onRemove?: (connection: ClientConnection) => void; onDisconnect?: (connection: ClientConnection) => void;
};
const editable = (target: EventTarget | null) => target instanceof HTMLElement && (target.isContentEditable || target.matches('input:not([type="checkbox"]),textarea,select'));
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

function ClientEntryInformation({ name, folder }: { name: string; folder: string }) {
  const disclosure = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (event.target instanceof Node && !disclosure.current?.contains(event.target)) disclosure.current?.removeAttribute('open'); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, []);
  return <details ref={disclosure} className="client-entry-info" onKeyDown={event => {
    if (event.key === 'Escape' && disclosure.current?.open) { event.preventDefault(); event.stopPropagation(); disclosure.current.open = false; disclosure.current.querySelector('summary')?.focus(); }
  }}><summary className="icon-button" aria-label={`查看 ${name} 入口信息`} title={`查看 ${name} 入口信息`}><Info size={18} aria-hidden="true" /></summary><div className="client-entry-description" role="group" aria-label={`${name} 入口信息`}><strong>{name} 入口</strong><code>{folder}</code><p>接入检查核对目录链接；使用记录由你确认，原文件保留在原目录。</p></div></details>;
}

function AdvancedConnectionActions({ onIgnore, disabled = false, batch = false }: { onIgnore: () => void; disabled?: boolean; batch?: boolean }) {
  const disclosure = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (event.target instanceof Node && !disclosure.current?.contains(event.target)) disclosure.current?.removeAttribute('open'); };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, []);
  return <details ref={disclosure} className={`client-advanced-actions${batch ? ' client-batch-advanced' : ''}`} onKeyDown={event => {
    if (event.key === 'Escape' && disclosure.current?.open) { event.preventDefault(); event.stopPropagation(); disclosure.current.open = false; disclosure.current.querySelector('summary')?.focus(); }
  }}><summary>高级操作<ChevronDown size={13} aria-hidden="true" /></summary><div><button type="button" disabled={disabled} onClick={() => { disclosure.current?.removeAttribute('open'); onIgnore(); }}>{CLIENT_MANAGEMENT_COPY.stop.label}</button><small>{CLIENT_MANAGEMENT_COPY.stop.description}</small></div></details>;
}

export function ClientPanel({ skills, connections, client, onClientChange, onCheck, onOpenConnect, onOpenSkill, onManage, onConfirmUsage, onRemove, onDisconnect }: Props) {
  const [query, setQuery] = useState(''), [abnormal, setAbnormal] = useState(false), [page, setPage] = useState(1), [size, setSize] = useState(10);
  const [selected, setSelected] = useState<string[]>([]), [menu, setMenu] = useState(''), [usageConfirm, setUsageConfirm] = useState(''), [checking, setChecking] = useState(false), [error, setError] = useState('');
  const [usagePending, setUsagePending] = useState('');
  const anchor = useRef<string | undefined>(undefined), pointerShift = useRef(false), root = useRef<HTMLElement>(null);
  useEffect(() => { setSelected([]); anchor.current = undefined; setPage(1); setMenu(''); setUsageConfirm(''); }, [client]);
  useEffect(() => { const available = new Set(connections.filter(item => item.client === client).map(item => item.id)); setSelected(previous => previous.filter(id => available.has(id))); }, [connections, client]);
  useEffect(() => {
    if (!menu) return;
    const outside = (event: Event) => { if (event.target instanceof Element && !event.target.closest('.client-manage-menu')) setMenu(''); };
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); const id = menu; setMenu(''); root.current?.querySelector<HTMLElement>(`[data-client-menu="${CSS.escape(id)}"]`)?.focus(); } };
    document.addEventListener('pointerdown', outside); document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); };
  }, [menu]);
  const rows = useMemo(() => clientConnectionRows(skills, connections, client, query, abnormal), [skills, connections, client, query, abnormal]);
  const filteredIds = rows.map(item => item.id), pages = Math.max(1, Math.ceil(rows.length / size)), currentPage = Math.min(page, pages);
  const visible = rows.slice((currentPage - 1) * size, currentPage * size), chosen = connections.filter(item => item.client === client && selected.includes(item.id));
  const hidden = selected.filter(id => !filteredIds.includes(id)).length, current = CLIENT_OPTIONS.find(item => item.id === client) || CLIENT_OPTIONS[0];
  const choose = (id: string, checked: boolean, shift = false) => { setSelected(previous => clientSelectionRange(previous, filteredIds, anchor.current, id, checked, shift)); anchor.current = id; };
  const selectAll = () => setSelected(previous => selectFilteredClientItems(previous, filteredIds));
  const listKeys = (event: KeyboardEvent) => { if (clientSelectionShortcut(event, editable(event.target)) === 'all') { event.preventDefault(); selectAll(); } };
  const manage = (items: ClientConnection[], mode: ClientManagementMode) => { setMenu(''); if (onManage) onManage(items, mode); else if (items.length === 1) (mode === 'stop' ? onRemove : onDisconnect)?.(items[0]); };
  const check = async () => { setError(''); setChecking(true); try { await onCheck(); } catch (cause) { setError(errorText(cause)); } finally { setChecking(false); } };
  const confirmUsage = async (connection: ClientConnection) => { if (!onConfirmUsage) return; setError(''); setUsagePending(connection.id); try { await onConfirmUsage(connection); setUsageConfirm(''); } catch (cause) { setError(errorText(cause)); } finally { setUsagePending(''); } };
  return <section className="panel client-panel" ref={root}>
    <header className="client-panel-header" data-pane-sticky>
      <div className="client-header-tabs"><div className="client-tabs" role="tablist" aria-label="客户端">{CLIENT_OPTIONS.map(item => <button type="button" key={item.id} role="tab" aria-selected={client === item.id} className={client === item.id ? 'active' : ''} onClick={() => onClientChange(item.id)}>{item.name}<span>{connections.filter(row => row.client === item.id && row.health === 'ready').length}</span></button>)}</div><ClientEntryInformation name={current.name} folder={current.folder} /></div>
      <div className="client-toolbar"><label className="search-input"><Search size={16} aria-hidden="true" /><input aria-label="搜索客户端连接" placeholder="搜索 Skill 或目录" value={query} onChange={event => { setQuery(event.target.value); setPage(1); }} /></label><label className="client-abnormal"><input type="checkbox" checked={abnormal} onChange={event => { setAbnormal(event.target.checked); setPage(1); }} />仅入口异常</label><button type="button" className="button" disabled={checking} onClick={() => void check()}><RefreshCw size={15} />{checking ? '正在检查…' : '检查连接'}</button><button type="button" className="button primary" disabled={!onOpenConnect} onClick={() => onOpenConnect?.(client)}><Plus size={15} />连接 Skill</button></div>
    </header>
    {error && <p className="inline-error client-panel-error" role="alert">{error}</p>}
    <div className="client-selection-tools"><button type="button" className="text-button" disabled={!rows.length} onClick={selectAll}>选择当前筛选全部（{rows.length}）</button><button type="button" className="text-button" disabled={!selected.length} onClick={() => { setSelected([]); anchor.current = undefined; }}>清空选择</button><span role="status">已选 {selected.length} 项{hidden > 0 && ` · 筛选外 ${hidden} 项`}</span>{selected.length > 0 && <><button type="button" className="button" disabled={!onManage} title={CLIENT_MANAGEMENT_COPY.disconnect.description} onClick={() => manage(chosen, 'disconnect')}>预览断开所选</button><AdvancedConnectionActions batch disabled={!onManage} onIgnore={() => manage(chosen, 'stop')} /></>}</div>
    <div className="client-rows client-selectable-list" role="group" aria-label="当前筛选客户端连接" tabIndex={0} onKeyDown={listKeys}>{!visible.length && <div className="empty"><h3>{abnormal ? '没有入口异常' : '尚无匹配连接'}</h3><p>可调整筛选，或使用“连接 Skill”添加。</p></div>}{visible.map(connection => {
      const skill = skills.find(item => item.id === connection.skillId), label = clientSkillName(skill), isSelected = selected.includes(connection.id), ready = connection.health === 'ready';
      return <article className={`client-row ${isSelected ? 'is-selected' : ''}`} key={connection.id} data-client-connection={connection.id} tabIndex={0} onKeyDown={event => { if (event.target === event.currentTarget && event.key === ' ' && !event.ctrlKey && !event.altKey && !event.metaKey) { event.preventDefault(); choose(connection.id, !isSelected, event.shiftKey); } }}>
        <div className="client-row-main"><input type="checkbox" aria-label={`选择连接 ${label}`} checked={isSelected} onClick={event => { pointerShift.current = event.shiftKey; }} onChange={event => { choose(connection.id, event.target.checked, pointerShift.current); pointerShift.current = false; }} />{skill && onOpenSkill ? <button type="button" className="client-row-name client-skill-link" onClick={() => onOpenSkill(skill.id)} title={`查看 ${label} 的 Skill 详情`}>{label}</button> : <strong className="client-row-name">{label}</strong>}<span className={`badge ${ready ? 'green' : 'amber'}`}>{ready ? '入口正常' : '入口待处理'}</span>{connection.usageConfirmation && <span className={`client-usage-state ${connection.usageStatus === 'stale' ? 'is-stale' : ''}`} title="用户确认的当前内容使用记录">{clientUsageText(connection)}</span>}<div className="client-manage-menu"><button type="button" className="button ghost" data-client-menu={connection.id} aria-expanded={menu === connection.id} aria-label={`管理连接 ${label}`} onClick={() => setMenu(previous => previous === connection.id ? '' : connection.id)}>管理连接<ChevronDown size={13} /></button>{menu === connection.id && <div className="client-manage-options" role="group" aria-label={`${label}连接操作`}><button type="button" aria-label={CLIENT_MANAGEMENT_COPY.disconnect.label} disabled={connection.kind === 'direct' || !!skill?.readOnly} title={connection.kind === 'direct' ? '客户端直接使用此原目录，无法解除链接；可通过高级操作忽略记录。' : skill?.readOnly ? '系统或插件管理的 Skill 入口受保护；可通过高级操作忽略记录。' : CLIENT_MANAGEMENT_COPY.disconnect.description} onClick={() => manage([connection], 'disconnect')}>{CLIENT_MANAGEMENT_COPY.disconnect.label}<small>保留 Skill 原文件</small></button><button type="button" disabled={!ready || !onConfirmUsage} onClick={() => { setMenu(''); setUsageConfirm(connection.id); }}>记录我已使用当前内容</button><AdvancedConnectionActions onIgnore={() => manage([connection], 'stop')} /></div>}</div></div>
        <details className="client-row-details"><summary aria-label={`${label}的连接详情`}><code className="client-source" title={connection.target || skill?.localPath || undefined}>{connection.target || skill?.localPath || '原目录信息不可用'}</code><span className="client-details-toggle">连接详情<ChevronDown size={13} aria-hidden="true" /></span></summary><dl className="info-grid compact"><dt>客户端入口</dt><dd><code>{connection.path}</code></dd><dt>真实原目录</dt><dd><code>{connection.target}</code></dd><dt>连接来源</dt><dd>{connection.kind === 'direct' ? '原位目录' : connection.origin === 'created' ? '管理器创建' : '已存在的链接'}</dd><dt>最近接入检查</dt><dd>{connection.checkedAt ? new Date(connection.checkedAt).toLocaleString('zh-CN') : '尚未检查'}</dd><dt>使用记录</dt><dd>{clientUsageText(connection)}{connection.usageConfirmation && <span> · {new Date(connection.usageConfirmation.confirmedAt).toLocaleString('zh-CN')} · 用户确认</span>}</dd></dl></details>
        {connection.error && <p className="client-row-error" role="status">{connection.error}</p>}{connection.autoRepairable && <p className="client-row-note">{connection.repairReason || '管理器创建的入口缺失，接入检查后将尝试安全恢复。'}</p>}
        {usageConfirm === connection.id && <div className="client-usage-confirm"><p>确认你已在 {current.name} 中使用当前 Skill 内容。记录客户端、时间和内容指纹；这里不会自动运行模型，内容变化后记录会标为过期。</p><div className="button-row"><button type="button" className="button primary" disabled={usagePending === connection.id} onClick={() => void confirmUsage(connection)}>确认已使用当前内容</button><button type="button" className="button" disabled={usagePending === connection.id} onClick={() => setUsageConfirm('')}>取消</button></div></div>}
      </article>;
    })}</div>
    <div className="table-footer client-pagination"><span>{rows.length} 个连接 · 第 {currentPage} / {pages} 页</span><label>每页 <select aria-label="每页客户端连接数量" value={size} onChange={event => { setSize(Number(event.target.value)); setPage(1); }}>{[10, 20, 50].map(value => <option key={value} value={value}>{value} 项</option>)}</select></label><div><button type="button" className="icon-button" aria-label="上一页连接" disabled={currentPage <= 1} onClick={() => setPage(currentPage - 1)}><ChevronLeft size={17} /></button><button type="button" className="icon-button" aria-label="下一页连接" disabled={currentPage >= pages} onClick={() => setPage(currentPage + 1)}><ChevronRight size={17} /></button></div></div>
  </section>;
}
