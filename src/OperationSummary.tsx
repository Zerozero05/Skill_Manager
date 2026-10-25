import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ArrowLeftRight, LoaderCircle, X } from 'lucide-react';
import { operationSummaryPlacement, operationSummaryTasks } from './operation-history';
import { taskStatus, terminalTask, type ManagerTask } from './tasks';
import './operation-summary.css';

type Props = {
  tasks: readonly ManagerTask[];
  visible?: boolean;
  onShowAll: () => void | Promise<void>;
  onDetails: (task: ManagerTask) => void | Promise<void>;
  onReturn: (task: ManagerTask) => void | Promise<void>;
};

/** A view of the window's existing task service. Opening it performs no request. */
export function OperationSummary({ tasks, visible = true, onShowAll, onDetails, onReturn }: Props) {
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLElement>(null);
  const keyboardOpen = useRef(false), pending = useRef(false);
  const [open, setOpen] = useState(false), [error, setError] = useState('');
  const [working, setWorking] = useState('');
  const [position, setPosition] = useState({ left: 0, top: 0, width: 360, maxHeight: 400, compact: false, ready: false });
  const panelId = useId(), headingId = useId();
  const publicTasks = tasks.filter(task => !task.hidden);
  const activeCount = publicTasks.filter(task => !terminalTask(task)).length;
  const rows = operationSummaryTasks(publicTasks);
  const close = useCallback((restoreFocus = false) => {
    setOpen(false);
    if (restoreFocus && trigger.current?.getClientRects().length) trigger.current.focus({ preventScroll: true });
  }, []);

  useEffect(() => { if (!visible) close(); }, [visible, close]);
  useLayoutEffect(() => {
    const button = trigger.current, popup = panel.current;
    if (!open || !visible || !button || !popup) return;
    const shell = button.closest<HTMLElement>('.app-shell');
    const main = shell?.querySelector<HTMLElement>(':scope > .main');
    const sidebar = button.closest<HTMLElement>('.sidebar');
    if (!main || !sidebar) { close(); return; }
    let frame = 0;
    const measure = () => {
      if (!button.getClientRects().length || !main.getClientRects().length) { close(); return; }
      const zoom = Number(getComputedStyle(document.documentElement).getPropertyValue('--ui-zoom')) || 1;
      const source = button.getBoundingClientRect(), area = main.getBoundingClientRect(), side = sidebar.getBoundingClientRect();
      const bounds = { left: Math.max(area.left, side.right) / zoom, top: Math.max(0, area.top) / zoom, right: Math.min(window.innerWidth, area.right) / zoom, bottom: Math.min(window.innerHeight, area.bottom) / zoom };
      const anchor = { left: source.left / zoom, top: source.top / zoom, right: source.right / zoom, bottom: source.bottom / zoom };
      const value = operationSummaryPlacement(anchor, bounds, popup.getBoundingClientRect().height / zoom);
      setPosition(previous => {
        const next = { ...value, ready: true };
        return Object.keys(next).every(key => previous[key as keyof typeof previous] === next[key as keyof typeof next]) ? previous : next;
      });
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const outside = (event: PointerEvent) => {
      if (!popup.contains(event.target as Node) && !button.contains(event.target as Node)) close();
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault(); event.stopPropagation(); close(true);
    };
    const outsideScroll = (event: Event) => { if (!popup.contains(event.target as Node)) schedule(); };
    const resize = new ResizeObserver(schedule);
    for (const node of [button, popup, main, sidebar]) resize.observe(node);
    const appearance = new MutationObserver(schedule);
    appearance.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'data-theme'] });
    measure();
    document.addEventListener('pointerdown', outside, true); document.addEventListener('keydown', escape);
    window.addEventListener('resize', schedule); window.addEventListener('scroll', outsideScroll, true);
    return () => {
      cancelAnimationFrame(frame); resize.disconnect(); appearance.disconnect();
      document.removeEventListener('pointerdown', outside, true); document.removeEventListener('keydown', escape);
      window.removeEventListener('resize', schedule); window.removeEventListener('scroll', outsideScroll, true);
    };
  }, [open, visible, close]);
  useLayoutEffect(() => {
    if (open && position.ready && keyboardOpen.current) {
      keyboardOpen.current = false; panel.current?.focus({ preventScroll: true });
    }
  }, [open, position.ready]);

  const run = async (key: string, action: () => void | Promise<void>) => {
    if (pending.current) return;
    pending.current = true; setWorking(key); setError('');
    try { await action(); close(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { pending.current = false; setWorking(''); }
  };
  return <>
    <button ref={trigger} className="history-sidebar-entry" type="button" aria-label="操作记录" aria-expanded={open && visible} aria-controls={panelId} title={activeCount ? `操作记录 · ${activeCount} 项进行中` : '查看最近操作摘要'} onClick={event => {
      if (open) { close(); return; }
      keyboardOpen.current = event.detail === 0; setError(''); setPosition(previous => ({ ...previous, ready: false })); setOpen(true);
    }}>{activeCount ? <LoaderCircle className="spin" size={16} /> : <ArrowLeftRight size={16} />}<span>操作记录</span><small>{activeCount ? `${activeCount} 进行中` : publicTasks.length}</small></button>
    {open && visible && createPortal(<section ref={panel} id={panelId} className={`operation-summary ${position.compact ? 'is-compact' : ''}`} role="region" aria-labelledby={headingId} tabIndex={-1} style={{ position: 'fixed', left: position.left, top: position.top, width: position.width, maxHeight: position.maxHeight, visibility: position.ready ? 'visible' : 'hidden' }}>
      <header className="operation-summary-header"><div><h2 id={headingId}>最近操作</h2><p>{activeCount ? `${activeCount} 项进行中 · ` : ''}共 {publicTasks.length} 条</p></div><button type="button" className="icon-button" aria-label="关闭操作摘要" onClick={() => close(true)}><X size={17} /></button></header>
      {!position.compact && <div className="operation-summary-list">{rows.map(task => <article className={`operation-summary-row ${task.status}`} key={task.id}>
        <div className="operation-summary-title"><strong title={task.label}>{task.label}</strong><span>{taskStatus[task.status] || task.status}</span></div>
        {(task.stage !== task.label || task.progress) && <p className="operation-summary-stage" title={task.stage}>{task.stage}{task.progress && task.progress.total > 0 && ` · ${task.progress.completed}/${task.progress.total}`}</p>}
        <div className="operation-summary-actions"><button type="button" disabled={!!working} onClick={() => void run(`return:${task.id}`, () => onReturn(task))}>返回操作</button><button type="button" disabled={!!working} onClick={() => void run(`details:${task.id}`, () => onDetails(task))}>{working === `details:${task.id}` ? '正在打开…' : '详情'}</button></div>
      </article>)}{!rows.length && <p className="operation-summary-empty">暂无操作记录</p>}</div>}
      {error && <p className="operation-summary-error" role="alert">{error}</p>}
      <footer><button type="button" className="button" disabled={!!working} onClick={() => void run('all', onShowAll)}>查看全部记录</button></footer>
    </section>, document.body)}
  </>;
}
