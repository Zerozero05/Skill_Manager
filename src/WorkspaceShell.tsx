import { useCallback, useEffect, useId, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode, type RefObject } from 'react';
import { Check, ChevronDown, ChevronRight, CircleAlert, CircleHelp, Columns2, CornerUpLeft, GripVertical, Home, ListTree, LoaderCircle, PanelTop, RotateCcw, SquareArrowOutUpRight, X } from 'lucide-react';
import { DEFAULT_SPLIT_RATIO, MIN_SPLIT_PANE_WIDTH, SPLIT_DIVIDER_WIDTH, markdownHeadingAnchor, nextPaneId, outlineId, paneAnchorIndex, paneAnchorScrollTop, paneScrollScale, paneTitle, splitRatioForPosition, splitViewForPanes, visibleWorkspacePaneIds, workspaceShortcut, type OutlineEntry, type PaneTaskBadge, type ScreenPoint, type SplitView, type WorkspacePane } from './workspace';
import './workspace.css';

export type WorkspaceShellProps = {
  panes: readonly WorkspacePane[]; activePaneId: string;
  onActivate: (paneId: string) => void; onClose: (paneId: string) => void;
  onReorder: (paneId: string, beforePaneId?: string) => void;
  onDetach: (paneId: string, point?: ScreenPoint) => void;
  onReturn?: (paneId: string) => void;
  pendingTransfers?: ReadonlySet<string>; taskBadges?: ReadonlyMap<string, PaneTaskBadge>;
  splitView?: SplitView; onSplitChange?: (splitView?: SplitView) => void;
  renderPane: (pane: WorkspacePane, active: boolean, visible: boolean) => ReactNode;
  onResetLayout?: () => void; onShowHelp?: () => void; shortcutsBlocked?: boolean; checkControl?: ReactNode;
};

/** A native window has one shell. Pages remain mounted when their tab is inactive. */
export function WorkspaceShell({ panes, activePaneId, onActivate, onClose, onReorder, onDetach, onReturn, pendingTransfers, taskBadges, splitView, onSplitChange, renderPane, onResetLayout, onShowHelp, shortcutsBlocked = false, checkControl }: WorkspaceShellProps) {
  const strip = useRef<HTMLDivElement>(null);
  const pages = useRef<HTMLDivElement>(null);
  const gesture = useRef<{ paneId: string; pointerId: number; startX: number; startY: number; moved: boolean; beforeId?: string; detach: boolean } | null>(null);
  const dividerGesture = useRef<{ pointerId: number; ratio: number; rightPaneId: string } | null>(null);
  const suppressClick = useRef(false);
  const [draggedId, setDraggedId] = useState<string>(); const [dropId, setDropId] = useState<string>(); const [detaching, setDetaching] = useState(false);
  const [draftRatio, setDraftRatio] = useState<number>();
  const active = panes.find(pane => pane.id === activePaneId) || panes[0];
  const split = splitViewForPanes(splitView, panes);
  const visible = new Set(visibleWorkspacePaneIds(panes, active?.id || '', split));
  const ratio = draftRatio ?? split?.ratio ?? DEFAULT_SPLIT_RATIO;
  const home = panes.find(pane => pane.kind === 'home');
  const splitCandidate = active?.kind !== 'home' ? active : [...panes].reverse().find(pane => pane.kind !== 'home');
  const helpId = useId();
  useEffect(() => {
    const owner = pages.current; if (!owner) return;
    const guarded = new Map<HTMLElement, { value: string; priority: string; frame: number }>();
    const restore = (scroll: HTMLElement, entry: { value: string; priority: string; frame: number }) => {
      cancelAnimationFrame(entry.frame);
      if (entry.value) scroll.style.setProperty('overflow-anchor', entry.value, entry.priority);
      else scroll.style.removeProperty('overflow-anchor');
      guarded.delete(scroll);
    };
    const toggle = (event: MouseEvent) => {
      if (event.button !== 0 || event.defaultPrevented || !(event.target instanceof Element)) return;
      const summary = event.target.closest<HTMLElement>('summary');
      if (!summary || !(summary.parentElement instanceof HTMLDetailsElement) || summary.parentElement.querySelector(':scope > summary') !== summary) return;
      const control = event.target.closest('button,a[href],input,select,textarea,label,[contenteditable]:not([contenteditable="false"]),[role="button"],[role="link"]');
      if (control && control !== summary && summary.contains(control)) return;
      const scroll = summary.closest<HTMLElement>('[data-pane-scroll]');
      if (!scroll || !owner.contains(scroll)) return;
      // Chromium may anchor a focused disclosure's expanding body and move the
      // summary itself. Suspend anchoring only across this native toggle layout;
      // ordinary document/image anchoring resumes immediately afterward.
      const entry = guarded.get(scroll) || { value: scroll.style.getPropertyValue('overflow-anchor'), priority: scroll.style.getPropertyPriority('overflow-anchor'), frame: 0 };
      cancelAnimationFrame(entry.frame); guarded.set(scroll, entry);
      scroll.style.setProperty('overflow-anchor', 'none', entry.priority);
      entry.frame = requestAnimationFrame(() => { entry.frame = requestAnimationFrame(() => restore(scroll, entry)); });
    };
    owner.addEventListener('click', toggle, true);
    return () => { owner.removeEventListener('click', toggle, true); for (const [scroll, entry] of guarded) restore(scroll, entry); };
  }, []);
  useEffect(() => {
    if (dividerGesture.current && dividerGesture.current.rightPaneId !== split?.rightPaneId) dividerGesture.current = null;
    if (!dividerGesture.current) setDraftRatio(undefined);
  }, [split?.rightPaneId, split?.ratio]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      const blocking = shortcutsBlocked || Array.from(document.querySelectorAll<HTMLElement>('[aria-modal="true"]')).some(node => node.getClientRects().length > 0);
      const action = workspaceShortcut(event, blocking);
      if (!action || !active) return;
      event.preventDefault();
      if (action === 'close') { if (active.kind !== 'home' && !pendingTransfers?.has(active.id)) onClose(active.id); }
      else onActivate(nextPaneId(panes, active.id, action === 'previous'));
    };
    document.addEventListener('keydown', keydown);
    return () => document.removeEventListener('keydown', keydown);
  }, [panes, active, onActivate, onClose, pendingTransfers, shortcutsBlocked]);
  useEffect(() => { strip.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' }); }, [activePaneId]);
  const pointerStart = (event: ReactPointerEvent<HTMLButtonElement>, pane: WorkspacePane) => {
    if (event.button !== 0 || pane.kind === 'home' || pendingTransfers?.has(pane.id)) return;
    gesture.current = { paneId: pane.id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, moved: false, detach: false };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const pointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
    const current = gesture.current; if (!current || current.pointerId !== event.pointerId) return;
    if (!current.moved && Math.hypot(event.clientX - current.startX, event.clientY - current.startY) < 7) return;
    current.moved = true; setDraggedId(current.paneId);
    const rect = strip.current?.getBoundingClientRect();
    current.detach = event.clientX < 0 || event.clientX > window.innerWidth || event.clientY < 0 || event.clientY > window.innerHeight;
    setDetaching(current.detach);
    const tabElements = Array.from(strip.current?.querySelectorAll<HTMLElement>('[data-workspace-tab]') || []).filter(node => node.dataset.workspaceTab !== current.paneId && node.dataset.home !== 'true');
    current.beforeId = current.detach ? undefined : tabElements.find(node => { const tab = node.getBoundingClientRect(); return event.clientX < tab.left + tab.width / 2; })?.dataset.workspaceTab;
    setDropId(current.beforeId);
    if (rect && !current.detach) { if (event.clientX > rect.right - 35) strip.current?.scrollBy({ left: 22 }); else if (event.clientX < rect.left + 35) strip.current?.scrollBy({ left: -22 }); }
  };
  const finishDrag = (event: ReactPointerEvent<HTMLButtonElement>, cancelled = false) => {
    const current = gesture.current; if (!current || current.pointerId !== event.pointerId) return;
    gesture.current = null; setDraggedId(undefined); setDropId(undefined); setDetaching(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (!current.moved) return;
    suppressClick.current = true; setTimeout(() => { suppressClick.current = false; }, 0);
    if (cancelled) return;
    event.preventDefault();
    if (current.detach) onDetach(current.paneId, { screenX: event.screenX, screenY: event.screenY });
    else onReorder(current.paneId, current.beforeId);
  };
  const pointerRatio = (clientX: number) => {
    const node = pages.current; if (!node) return ratio;
    const rect = node.getBoundingClientRect(); const scale = node.clientWidth > 0 ? rect.width / node.clientWidth : 1;
    return splitRatioForPosition((clientX - rect.left) / (scale || 1) + node.scrollLeft, Math.max(node.clientWidth, MIN_SPLIT_PANE_WIDTH * 2 + SPLIT_DIVIDER_WIDTH) - SPLIT_DIVIDER_WIDTH);
  };
  const finishDivider = (event: ReactPointerEvent<HTMLDivElement>, cancelled = false) => {
    const current = dividerGesture.current; if (!current || current.pointerId !== event.pointerId) return;
    dividerGesture.current = null; setDraftRatio(undefined);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (!cancelled && split) onSplitChange?.({ ...split, ratio: current.ratio });
  };
  return <div className={`workspace-shell ${draftRatio !== undefined ? 'is-split-resizing' : ''}`}>
    <div className="workspace-bar">
      <div className="workspace-tabs" role="tablist" aria-label="工作区标签" aria-describedby={helpId} ref={strip}>
        {panes.map(pane => { const pending = !!pendingTransfers?.has(pane.id); const badge = taskBadges?.get(pane.id); return <div key={pane.id} className={`workspace-tab ${pane.id === active?.id ? 'is-active' : ''} ${draggedId === pane.id ? 'is-dragging' : ''} ${dropId === pane.id ? 'is-drop-target' : ''}`} data-workspace-tab={pane.id} data-home={pane.kind === 'home'}>
          <button className="workspace-tab-select" type="button" role="tab" id={`tab-${encodeURIComponent(pane.id)}`} aria-selected={pane.id === active?.id} aria-controls={`panel-${encodeURIComponent(pane.id)}`} tabIndex={pane.id === active?.id ? 0 : -1} title={pending ? `${paneTitle(pane)} · 正在移动，完成前保留此页` : `${paneTitle(pane)}${pane.kind === 'home' ? '' : ' · 拖动排序，向外拖出为独立窗口'}`} onClick={() => { if (suppressClick.current) { suppressClick.current = false; return; } onActivate(pane.id); }} onPointerDown={event => pointerStart(event, pane)} onPointerMove={pointerMove} onPointerUp={event => finishDrag(event)} onPointerCancel={event => finishDrag(event, true)} onKeyDown={event => {
            if (event.ctrlKey || event.metaKey || event.altKey) return;
            if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') { event.preventDefault(); const id = nextPaneId(panes, pane.id, event.key === 'ArrowLeft'); onActivate(id); strip.current?.querySelector<HTMLElement>(`[id="tab-${encodeURIComponent(id)}"]`)?.focus(); }
            if (event.key === 'Home' || event.key === 'End') { event.preventDefault(); const id = event.key === 'Home' ? panes[0]?.id : panes.at(-1)?.id; if (id) { onActivate(id); strip.current?.querySelector<HTMLElement>(`[id="tab-${encodeURIComponent(id)}"]`)?.focus(); } }
            if (event.key === 'Escape' && gesture.current?.moved) { gesture.current = null; setDraggedId(undefined); setDropId(undefined); setDetaching(false); suppressClick.current = true; setTimeout(() => { suppressClick.current = false; }, 0); }
          }}>
            {pending ? <LoaderCircle size={14} className="spin" /> : pane.kind === 'home' ? <Home size={15} /> : <GripVertical size={12} className="workspace-tab-grip" />}
            <span className="workspace-tab-title">{paneTitle(pane)}</span>{badge && <WorkspaceTaskBadge badge={badge} />}
          </button>
          {pane.kind !== 'home' && <button type="button" className="workspace-tab-close" disabled={pending} aria-label={`关闭 ${paneTitle(pane)}`} title="关闭标签（Ctrl＋W）" onClick={() => onClose(pane.id)}><X size={13} /></button>}
        </div>; })}
      </div>
      <div className="workspace-actions">
        {checkControl}
        {onSplitChange && <button className="icon-button" title={split ? '恢复单页，保留当前焦点与页面内容' : '工作台在左，当前业务页在右'} aria-label={split ? '恢复单页' : '左右分栏'} aria-pressed={!!split} disabled={!split && (!home || !splitCandidate)} onClick={() => onSplitChange(split ? undefined : { rightPaneId: splitCandidate!.id, ratio: DEFAULT_SPLIT_RATIO })}><Columns2 size={16} /></button>}
        {active?.kind !== 'home' && active && <>
          {onReturn && <button className="icon-button" title="移回主窗口" aria-label="将当前标签移回主窗口" disabled={pendingTransfers?.has(active.id)} onClick={() => onReturn(active.id)}><CornerUpLeft size={16} /></button>}
          <button className="icon-button" title="移至独立窗口" aria-label="将当前标签移至独立窗口" disabled={pendingTransfers?.has(active.id)} onClick={() => onDetach(active.id)}><SquareArrowOutUpRight size={15} /></button>
        </>}
        {onResetLayout && <button className="icon-button" title="恢复默认布局" aria-label="恢复默认布局" onClick={onResetLayout}><RotateCcw size={15} /></button>}
        {onShowHelp && <button className="icon-button" title="工作区快捷键" aria-label="工作区快捷键" onClick={onShowHelp}><CircleHelp size={16} /></button>}
      </div>
    </div>
    <span id={helpId} className="workspace-sr-only">Ctrl 加 Tab 切换，Ctrl 加 Shift 加 Tab 反向切换，Ctrl 加 W 关闭当前业务标签；拖出标签可单独打开窗口。左右分栏时点击工作台聚焦左侧，业务标签切换右侧；拖动分隔条调整大小，双击恢复默认比例。</span>
    {draggedId && <div className="workspace-drag-hint" role="status">{detaching ? '松开后移至独立窗口；原页面在新窗口就绪前保留' : '拖动调整标签顺序，向外拖动可移至独立窗口'}</div>}
    <div ref={pages} className={`workspace-pages ${split ? 'is-split' : ''}`} style={split ? { gridTemplateColumns: `minmax(${MIN_SPLIT_PANE_WIDTH}px,${ratio}fr) ${SPLIT_DIVIDER_WIDTH}px minmax(${MIN_SPLIT_PANE_WIDTH}px,${1 - ratio}fr)` } : undefined}>
      {panes.map(pane => { const isVisible = visible.has(pane.id); const isActive = pane.id === active?.id; return <section key={pane.id} className={`workspace-page ${isActive ? 'is-focused' : ''}`} id={`panel-${encodeURIComponent(pane.id)}`} role="tabpanel" aria-labelledby={`tab-${encodeURIComponent(pane.id)}`} hidden={!isVisible} data-pane-id={pane.id} data-pane-kind={pane.kind} data-pane-side={split && isVisible ? pane.kind === 'home' ? 'left' : 'right' : undefined} onPointerDownCapture={() => { if (isVisible && !isActive && !shortcutsBlocked) onActivate(pane.id); }} onFocusCapture={() => { if (isVisible && !isActive && !shortcutsBlocked) onActivate(pane.id); }}>{renderPane(pane, isActive, isVisible)}</section>; })}
      {split && <div className="workspace-divider" role="separator" aria-label="调整工作台与业务页宽度" aria-orientation="vertical" aria-valuemin={10} aria-valuemax={90} aria-valuenow={Math.round(ratio * 100)} aria-valuetext={`工作台 ${Math.round(ratio * 100)}%，业务页 ${Math.round((1 - ratio) * 100)}%`} tabIndex={0} title="拖动调整宽度；双击恢复 55% / 45%" onDoubleClick={() => onSplitChange?.({ ...split, ratio: DEFAULT_SPLIT_RATIO })} onPointerDown={event => { if (event.button !== 0 || !onSplitChange) return; event.preventDefault(); event.currentTarget.focus({ preventScroll: true }); event.currentTarget.setPointerCapture(event.pointerId); dividerGesture.current = { pointerId: event.pointerId, ratio, rightPaneId: split.rightPaneId }; setDraftRatio(ratio); }} onPointerMove={event => { const current = dividerGesture.current; if (!current || current.pointerId !== event.pointerId) return; current.ratio = pointerRatio(event.clientX); setDraftRatio(current.ratio); }} onPointerUp={event => finishDivider(event)} onPointerCancel={event => finishDivider(event, true)} onLostPointerCapture={event => finishDivider(event, true)} onKeyDown={event => { if (event.key === 'Escape' && dividerGesture.current) { event.preventDefault(); const pointerId = dividerGesture.current.pointerId; dividerGesture.current = null; setDraftRatio(undefined); if (event.currentTarget.hasPointerCapture(pointerId)) event.currentTarget.releasePointerCapture(pointerId); return; } if (!onSplitChange || event.ctrlKey || event.metaKey || event.altKey) return; if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); const width = Math.max(pages.current?.clientWidth || 0, MIN_SPLIT_PANE_WIDTH * 2 + SPLIT_DIVIDER_WIDTH) - SPLIT_DIVIDER_WIDTH; onSplitChange({ ...split, ratio: splitRatioForPosition(width * (ratio + (event.key === 'ArrowRight' ? 0.02 : -0.02)), width) }); } else if (event.key === 'Home') { event.preventDefault(); onSplitChange({ ...split, ratio: DEFAULT_SPLIT_RATIO }); } }} />}
    </div>
  </div>;
}

export function WorkspaceTaskBadge({ badge }: { badge: PaneTaskBadge }) {
  const running = badge.status === 'running' || badge.status === 'queued';
  const error = ['failed', 'partial', 'interrupted'].includes(badge.status);
  return <span className={`workspace-task-badge ${running ? 'is-running' : error ? 'is-error' : ''}`} title={badge.label || (running ? '后台任务进行中' : error ? '任务需要查看' : '任务已完成')} aria-label={badge.label || badge.status}>
    {running ? <LoaderCircle size={12} className="spin" /> : error ? <CircleAlert size={12} /> : <Check size={12} />}{badge.count && badge.count > 1 ? badge.count : null}
  </span>;
}

export function resolvePaneAnchor(scope: HTMLElement, anchor: string): HTMLElement | undefined {
  const nodes = Array.from(scope.querySelectorAll<HTMLElement>('[id],[data-workspace-original-id],[data-outline-anchor]'));
  const index = paneAnchorIndex(nodes.map(node => ({ id: node.id, originalId: node.dataset.workspaceOriginalId, outlineAnchor: node.dataset.outlineAnchor })), anchor);
  return nodes[index];
}

const pendingPaneJumps = new WeakMap<HTMLElement, () => void>();

/** The outer pane toolbar sits outside this scrollport and must not be counted twice. */
function paneOcclusion(scroller: HTMLElement, target?: HTMLElement): number {
  const viewport = scroller.getBoundingClientRect();
  const scale = paneScrollScale(viewport.height, scroller.offsetHeight);
  const paddingTop = Number.parseFloat(getComputedStyle(scroller).paddingTop) || 0;
  let covered = 0;
  for (const node of scroller.querySelectorAll<HTMLElement>('[data-pane-sticky],.detail-tabs')) {
    const style = getComputedStyle(node); const inset = Number.parseFloat(style.top);
    if (!['sticky', 'fixed'].includes(style.position) || style.position === 'sticky' && !Number.isFinite(inset) || !node.getClientRects().length) continue;
    if (target && !(node.compareDocumentPosition(target) & Node.DOCUMENT_POSITION_FOLLOWING)) continue;
    const rect = node.getBoundingClientRect();
    // Sticky insets start at the scrollport's inner padding edge. A negative inset
    // can cancel that padding; fixed elements instead use their actual overlap.
    const fixedOverlap = rect.top < viewport.bottom && rect.bottom > viewport.top && rect.left < viewport.right && rect.right > viewport.left;
    const bottom = style.position === 'fixed' ? fixedOverlap ? rect.bottom : viewport.top : target ? viewport.top + (scroller.clientTop + paddingTop + inset) * scale + rect.height : rect.top <= viewport.top + Math.max(0, inset * scale) + 1 ? rect.bottom : viewport.top;
    covered = Math.max(covered, bottom - viewport.top);
  }
  return Math.max(0, Math.min(covered, viewport.height));
}

/** Direct scoped navigation. A new jump or intentional user interaction cancels correction. */
export function scrollToPaneAnchor(scope: HTMLElement, anchor: string, _behavior: ScrollBehavior = 'instant', focusTarget = true): boolean {
  const scroller = scope.querySelector<HTMLElement>('[data-pane-scroll]') || scope;
  pendingPaneJumps.get(scroller)?.();
  const target = resolvePaneAnchor(scope, anchor); if (!target) return false;
  let active = true; let started = false; let frame = 0; let settle = 0; let highlightTimer = 0;
  const documentRoot = target.closest<HTMLElement>('[data-outline-document]');
  const documentKey = documentRoot?.dataset.outlineDocument;
  const body = scroller.querySelector<HTMLElement>('.workspace-pane-content') || scroller;
  const valid = () => active && target.isConnected && scope.contains(target) && !scope.closest('[hidden]') && documentRoot?.dataset.outlineDocument === documentKey;
  const cancel = () => {
    active = false; cancelAnimationFrame(frame); window.clearTimeout(settle); window.clearTimeout(highlightTimer);
    resize.disconnect(); mutation.disconnect(); target.classList.remove('pane-anchor-highlight');
    scroller.removeEventListener('wheel', interrupt); scroller.removeEventListener('touchstart', interrupt); scope.removeEventListener('pointerdown', interrupt, true); scope.removeEventListener('keydown', keyInterrupt, true);
    if (pendingPaneJumps.get(scroller) === cancel) pendingPaneJumps.delete(scroller);
  };
  const interrupt = () => cancel();
  const keyInterrupt = (event: KeyboardEvent) => { if (['ArrowDown', 'ArrowUp', 'PageDown', 'PageUp', 'Home', 'End', ' ', 'Tab', 'Escape'].includes(event.key)) cancel(); };
  const align = () => {
    if (!valid()) { cancel(); return; }
    const viewport = scroller.getBoundingClientRect();
    if (!viewport.height || !target.getClientRects().length) return;
    const scale = paneScrollScale(viewport.height, scroller.offsetHeight);
    // A settings card owns padding above its heading. Reveal the whole card header,
    // while the semantic heading keeps the focus and stable outline identity.
    const start = target.closest<HTMLElement>('[data-outline-scroll-start]') || target;
    const top = paneAnchorScrollTop(scroller.scrollTop, start.getBoundingClientRect().top, viewport.top, scale, paneOcclusion(scroller, target), Math.max(0, scroller.scrollHeight - scroller.clientHeight));
    scroller.scrollTo({ top, behavior: 'instant' });
  };
  const schedule = () => { if (!active) return; cancelAnimationFrame(frame); frame = requestAnimationFrame(() => { frame = requestAnimationFrame(() => {
    align(); if (!valid() || !target.getClientRects().length || started) return; started = true;
    if (focusTarget) { if (!target.hasAttribute('tabindex')) target.setAttribute('tabindex', '-1'); target.focus({ preventScroll: true }); }
    target.classList.add('pane-anchor-highlight');
    window.clearTimeout(highlightTimer); highlightTimer = window.setTimeout(cancel, 1200);
    // Allow late layout settling briefly, but never keep steering a user's reading position.
    settle = window.setTimeout(() => { resize.disconnect(); mutation.disconnect(); }, 650);
  }); }); };
  const resize = new ResizeObserver(schedule); resize.observe(body); resize.observe(scroller);
  const mutation = new MutationObserver(() => { if (!valid()) cancel(); else schedule(); });
  mutation.observe(body, { childList: true, subtree: true, attributes: true, attributeFilter: ['open', 'hidden', 'data-outline-document'] });
  pendingPaneJumps.set(scroller, cancel);
  scroller.addEventListener('wheel', interrupt, { passive: true }); scroller.addEventListener('touchstart', interrupt, { passive: true }); scope.addEventListener('pointerdown', interrupt, true); scope.addEventListener('keydown', keyInterrupt, true);
  let parent: HTMLElement | null = target;
  while (parent && scope.contains(parent)) {
    if (parent instanceof HTMLDetailsElement && !parent.open) { parent.dispatchEvent(new Event('workspace-expand')); parent.open = true; }
    parent = parent.parentElement;
  }
  highlightTimer = window.setTimeout(cancel, 1500);
  schedule();
  return true;
}

export type PaneOutlineProps = { paneId: string; scopeRef: RefObject<HTMLElement | null>; compact?: boolean; expanded?: boolean; onExpandedChange?: (expanded: boolean) => void; children?: (parts: { toggle: ReactNode; list: ReactNode; expanded: boolean; narrow: boolean }) => ReactNode };
/** Collect Markdown headings and explicit structural sections, scoped to one page only. */
export function PaneOutline({ paneId, scopeRef, compact = false, expanded: controlledExpanded, onExpandedChange, children }: PaneOutlineProps) {
  const [localExpanded, setLocalExpanded] = useState(false); const expanded = controlledExpanded ?? localExpanded;
  const [narrow, setNarrow] = useState(true); const toggleRef = useRef<HTMLButtonElement>(null), listRef = useRef<HTMLElement>(null);
  const [entries, setEntries] = useState<OutlineEntry[]>([]); const [currentId, setCurrentId] = useState<string>();
  const listId = useId(); const [navigationMessage, setNavigationMessage] = useState('');
  const setExpanded = useCallback((value: boolean) => { setLocalExpanded(value); onExpandedChange?.(value); }, [onExpandedChange]);
  useEffect(() => {
    const scope = scopeRef.current; if (!scope) return;
    // clientWidth is in logical pixels, including when the user changes interface zoom.
    const viewport = compact ? scope.querySelector<HTMLElement>('[data-pane-scroll]') || scope : scope;
    const update = () => { if (viewport.clientWidth > 0) setNarrow(viewport.clientWidth < (compact ? 700 : 760)); };
    const resize = new ResizeObserver(update); resize.observe(viewport); update();
    return () => resize.disconnect();
  }, [scopeRef, compact]);
  useEffect(() => {
    const scope = scopeRef.current; if (!scope || !expanded || !narrow) return;
    const close = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault(); event.stopPropagation(); setExpanded(false); toggleRef.current?.focus({ preventScroll: true });
    };
    const outside = (event: PointerEvent) => {
      if (!toggleRef.current?.contains(event.target as Node) && !listRef.current?.contains(event.target as Node)) setExpanded(false);
    };
    scope.addEventListener('keydown', close);
    document.addEventListener('pointerdown', outside, true);
    return () => { scope.removeEventListener('keydown', close); document.removeEventListener('pointerdown', outside, true); };
  }, [expanded, narrow, scopeRef, setExpanded]);
  useEffect(() => {
    const scope = scopeRef.current; if (!scope) return;
    let frame = 0;
    const collect = () => {
      const next: OutlineEntry[] = []; const occurrences = new Map<string, number>(); const fragments = new Map<string, number>();
      for (const node of scope.querySelectorAll<HTMLElement>('[data-outline-label],.markdown h1,.markdown h2,.markdown h3,.markdown h4')) {
        const hiddenParent = node.closest<HTMLElement>('[hidden]'); if (hiddenParent && scope.contains(hiddenParent)) continue;
        const label = node.dataset.outlineLabel || node.textContent?.trim() || ''; if (!label) continue;
        const documentKey = node.closest<HTMLElement>('[data-outline-document]')?.dataset.outlineDocument;
        const key = documentKey ? `document:${documentKey}:${label}` : node.dataset.outlineKey || `section:${label}`;
        const occurrence = occurrences.get(key) || 0; occurrences.set(key, occurrence + 1);
        const id = outlineId(paneId, label, occurrence, key);
        if (node.id && node.id !== id && !node.id.startsWith(`pane-${encodeURIComponent(paneId)}-`) && !node.dataset.workspaceOriginalId) node.dataset.workspaceOriginalId = node.id;
        if (node.closest('.markdown')) {
          const fragment = markdownHeadingAnchor(label); const fragmentKey = `${documentKey || ''}:${fragment}`;
          const repeated = fragments.get(fragmentKey) || 0; fragments.set(fragmentKey, repeated + 1);
          if (!node.dataset.workspaceOriginalId || repeated) node.dataset.outlineAnchor = markdownHeadingAnchor(label, repeated);
          else delete node.dataset.outlineAnchor;
        }
        node.id = id;
        const depth = Number(node.dataset.outlineLevel || node.tagName.slice(1));
        next.push({ id, label, level: Number.isFinite(depth) ? Math.max(1, Math.min(4, depth)) : 1 });
      }
      setEntries(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(collect); };
    const observe = new MutationObserver(schedule); observe.observe(scope, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['hidden', 'data-outline-label', 'data-outline-level', 'data-outline-key', 'data-outline-document'] });
    collect(); return () => { observe.disconnect(); cancelAnimationFrame(frame); };
  }, [paneId, scopeRef]);
  useEffect(() => {
    const scope = scopeRef.current; if (!scope || !entries.length) return;
    const scroller = scope.querySelector<HTMLElement>('[data-pane-scroll]') || scope;
    const update = () => {
      const viewport = scroller.getBoundingClientRect(); const visibleTop = viewport.top + paneOcclusion(scroller);
      const top = visibleTop + 20 * paneScrollScale(viewport.height, scroller.offsetHeight);
      const candidates = entries.map(entry => resolvePaneAnchor(scope, entry.id)).filter((item): item is HTMLElement => !!item && item.getClientRects().length > 0);
      const previous = candidates.filter(item => (item.closest<HTMLElement>('[data-outline-scroll-start]') || item).getBoundingClientRect().top <= top).at(-1);
      const atEnd = scroller.scrollTop >= scroller.scrollHeight - scroller.clientHeight - 1;
      const firstVisible = atEnd && (!previous || previous.getBoundingClientRect().bottom < visibleTop) ? candidates.find(item => { const rect = item.getBoundingClientRect(); return rect.top >= visibleTop && rect.bottom <= viewport.bottom; }) : undefined;
      setCurrentId((firstVisible || previous || candidates[0])?.id);
    };
    const resize = new ResizeObserver(update); resize.observe(scroller);
    scroller.addEventListener('scroll', update, { passive: true }); update(); return () => { resize.disconnect(); scroller.removeEventListener('scroll', update); };
  }, [entries, scopeRef]);
  const toggle = <button ref={toggleRef} className="pane-outline-toggle" type="button" aria-expanded={expanded} aria-controls={listId} onClick={() => setExpanded(!expanded)} title={expanded ? '收起目录' : '展开目录'}><ListTree size={15} /><span>目录</span>{expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}</button>;
  const list = expanded && <nav ref={listRef} id={listId} className="pane-outline-list" data-overlay={narrow || undefined} aria-label="当前页面目录">{entries.length ? entries.map(entry => <button type="button" key={entry.id} data-level={entry.level} className={currentId === entry.id ? 'is-current' : ''} aria-current={currentId === entry.id ? 'location' : undefined} title={entry.label} onClick={event => { const scope = scopeRef.current; if (scope) { const found = scrollToPaneAnchor(scope, entry.id, 'instant', event.detail === 0); setNavigationMessage(found ? '' : `“${entry.label}”当前不可定位，请重新打开对应内容。`); if (found) { setCurrentId(entry.id); if (narrow) setExpanded(false); } } }}><span>{entry.label}</span></button>) : <p className="pane-outline-empty">此页暂无目录</p>}<p className={navigationMessage ? 'pane-outline-message' : 'workspace-sr-only'} role="status">{navigationMessage}</p></nav>;
  return children ? children({ toggle, list, expanded, narrow }) : <div className={`pane-outline ${expanded ? 'is-expanded' : ''}`}>{toggle}{list}</div>;
}

export function PaneFrame({ paneId, children, toolbar, outline = true, outlineExpanded, onOutlineExpandedChange, initialScrollTop = 0, onScrollPosition }: { paneId: string; children: ReactNode; toolbar?: ReactNode; outline?: boolean; outlineExpanded?: boolean; onOutlineExpandedChange?: (expanded: boolean) => void; initialScrollTop?: number; onScrollPosition?: (top: number) => void }) {
  const scope = useRef<HTMLDivElement>(null); const scroller = useRef<HTMLDivElement>(null); const content = useRef<HTMLDivElement>(null);
  const restorePending = useRef(false); const onScroll = useRef(onScrollPosition); onScroll.current = onScrollPosition;
  useEffect(() => {
    const scroll = scroller.current; const body = content.current; const frameScope = scope.current;
    if (!scroll || !body || !frameScope) return;
    const desired = Number.isFinite(initialScrollTop) ? Math.max(0, initialScrollTop) : 0;
    restorePending.current = desired > 0; let frame = 0;
    const attempt = () => {
      if (!restorePending.current || scroll.clientHeight === 0) return;
      const max = Math.max(0, scroll.scrollHeight - scroll.clientHeight);
      // Async documents/images may not be tall enough yet. Keep the requested position until
      // content arrives, but stop immediately once the user starts interacting with this page.
      if (max + 1 >= desired) { restorePending.current = false; scroll.scrollTo({ top: desired, behavior: 'instant' }); onScroll.current?.(scroll.scrollTop); }
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(attempt); };
    const stop = () => { restorePending.current = false; };
    const wheel = (event: WheelEvent) => { if (!event.ctrlKey) stop(); };
    frameScope.addEventListener('pointerdown', stop, true); frameScope.addEventListener('keydown', stop, true); scroll.addEventListener('wheel', wheel, { passive: true }); scroll.addEventListener('touchstart', stop, { passive: true });
    const resize = new ResizeObserver(schedule); resize.observe(scroll); resize.observe(body);
    const mutation = new MutationObserver(schedule); mutation.observe(body, { childList: true, characterData: true, subtree: true });
    if (!desired) scroll.scrollTop = 0; else schedule();
    return () => { pendingPaneJumps.get(scroll)?.(); resize.disconnect(); mutation.disconnect(); cancelAnimationFrame(frame); frameScope.removeEventListener('pointerdown', stop, true); frameScope.removeEventListener('keydown', stop, true); scroll.removeEventListener('wheel', wheel); scroll.removeEventListener('touchstart', stop); restorePending.current = false; };
  }, [paneId]); // Initial restore only; ordinary rerenders must not overwrite user scrolling.
  const body = <div className="workspace-pane-scroll" data-pane-scroll ref={scroller} onScroll={event => { if (!restorePending.current && event.currentTarget.clientHeight > 0) onScrollPosition?.(event.currentTarget.scrollTop); }}><div className="workspace-pane-content" ref={content}>{children}</div></div>;
  return <div className={`workspace-pane-frame ${outline ? 'has-outline' : ''}`} ref={scope} data-workspace-scope={paneId}>
    {outline ? <PaneOutline paneId={paneId} scopeRef={scope} expanded={outlineExpanded} onExpandedChange={onOutlineExpandedChange}>{({ toggle, list }) => <>
      <header className="workspace-pane-header">{toggle}<div className="workspace-pane-toolbar">{toolbar}</div></header>{list}{body}
    </>}</PaneOutline> : <>{toolbar && <header className="workspace-pane-header"><div className="workspace-pane-toolbar">{toolbar}</div></header>}{body}</>}
  </div>;
}

/** Success is compact; detailed or destructive results remain available in task history. */
export function WorkspaceFeedback({ message, tone = 'success', onDismiss, onOpenHistory }: { message: string; tone?: 'success' | 'info' | 'error'; onDismiss?: () => void; onOpenHistory?: () => void }) {
  return <div className={`workspace-feedback ${tone}`} role={tone === 'error' ? 'alert' : 'status'}><span>{tone === 'error' ? <CircleAlert size={14} /> : <Check size={14} />}{message}</span>{onOpenHistory && <button type="button" onClick={onOpenHistory}>查看记录</button>}{onDismiss && <button className="icon-button" type="button" aria-label="关闭提示" onClick={onDismiss}><X size={13} /></button>}</div>;
}

export function WorkspaceShortcutHelp({ onClose }: { onClose: () => void }) {
  const close = useCallback((event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); }, [onClose]);
  useEffect(() => { document.addEventListener('keydown', close); return () => document.removeEventListener('keydown', close); }, [close]);
  return <div className="workspace-shortcut-help" role="region" aria-label="工作区快捷键"><div><PanelTop size={16} /><strong>工作区快捷键</strong><button className="icon-button" type="button" aria-label="关闭快捷键说明" onClick={onClose}><X size={15} /></button></div><dl><dt>Ctrl＋Tab</dt><dd>切换到下个标签</dd><dt>Ctrl＋Shift＋Tab</dt><dd>切换到上个标签</dd><dt>Ctrl＋W</dt><dd>关闭当前业务标签</dd><dt>Ctrl＋滚轮</dt><dd>调整界面缩放</dd><dt>Ctrl＋Z</dt><dd>撤销分类与收藏操作</dd></dl><p>左右分栏时工作台保留在左，业务标签切换右侧。拖动分隔条调整宽度，双击恢复 55%／45%；Esc 取消本次拖动。</p><p>拖动标签调整顺序，向外拖出可单独打开窗口。输入框中的撤销仍由输入框处理。</p></div>;
}
