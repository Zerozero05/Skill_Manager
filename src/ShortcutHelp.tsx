import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { CircleHelp, X } from 'lucide-react';
import { restoreShortcutHelpFocus, shortcutHelpBounds, shortcutHelpPlacement, shortcutHelpRows, type ShortcutHelpMode } from './shortcut-help';
import './shortcut-help.css';

/** Local help only: no task, inventory, network request or persistent state. */
export function ShortcutHelp({ mode, visible }: { mode: ShortcutHelpMode; visible: boolean }) {
  const trigger = useRef<HTMLButtonElement>(null), panel = useRef<HTMLElement>(null);
  const heading = useRef<HTMLElement>(null), content = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState({ left: 0, top: 0, width: 352, maxHeight: 400, ready: false });
  const panelId = useId(), headingId = useId();
  const close = useCallback((restoreFocus = false) => {
    setOpen(false);
    if (restoreFocus) restoreShortcutHelpFocus(trigger.current);
  }, []);
  useEffect(() => { close(); }, [visible, mode, close]);
  useLayoutEffect(() => {
    const button = trigger.current, popup = panel.current, header = heading.current, body = content.current;
    if (!open || !visible || !button || !popup || !header || !body) return;
    const main = button.closest<HTMLElement>('.main');
    const pane = button.closest<HTMLElement>('.workspace-page');
    if (!main || !pane) { close(); return; }
    let frame = 0;
    const measure = () => {
      if (!button.isConnected || !button.getClientRects().length || !main.getClientRects().length || !pane.getClientRects().length) { close(); return; }
      const rootStyle = getComputedStyle(document.documentElement);
      const zoom = Number(rootStyle.getPropertyValue('--ui-zoom')) || 1;
      const logical = (node: HTMLElement, client = false) => {
        const rect = node.getBoundingClientRect();
        const left = rect.left / zoom + (client ? node.clientLeft : 0), top = rect.top / zoom + (client ? node.clientTop : 0);
        return { left, top, right: client ? left + node.clientWidth : rect.right / zoom, bottom: client ? top + node.clientHeight : rect.bottom / zoom };
      };
      const viewport = window.visualViewport;
      const bounds = shortcutHelpBounds(logical(main, true), logical(pane, true), {
        left: (viewport?.offsetLeft || 0) / zoom, top: (viewport?.offsetTop || 0) / zoom,
        right: ((viewport?.offsetLeft || 0) + (viewport?.width || innerWidth)) / zoom,
        bottom: ((viewport?.offsetTop || 0) + (viewport?.height || innerHeight)) / zoom,
      });
      const value = shortcutHelpPlacement(logical(button), bounds, 22 * Number.parseFloat(rootStyle.fontSize), header.offsetHeight + body.scrollHeight + 2);
      if (!value) { close(); return; }
      setPosition(previous => {
        const next = { left: value.left, top: value.top, width: value.width, maxHeight: value.maxHeight, ready: true };
        return Object.keys(next).every(key => previous[key as keyof typeof previous] === next[key as keyof typeof next]) ? previous : next;
      });
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const outside = (event: Event) => {
      if (!popup.contains(event.target as Node) && !button.contains(event.target as Node)) close();
    };
    const scroll = (event: Event) => { if (!popup.contains(event.target as Node)) close(); };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault(); event.stopPropagation(); close(true);
    };
    const resize = new ResizeObserver(schedule);
    for (const node of [button, popup, main, pane]) resize.observe(node);
    const appearance = new MutationObserver(schedule);
    appearance.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'data-theme'] });
    const paneState = new MutationObserver(schedule);
    paneState.observe(pane, { attributes: true, attributeFilter: ['hidden', 'style', 'class'] });
    measure();
    document.addEventListener('pointerdown', outside, true); document.addEventListener('focusin', outside);
    document.addEventListener('keydown', escape); window.addEventListener('scroll', scroll, true);
    window.addEventListener('resize', schedule); window.visualViewport?.addEventListener('resize', schedule);
    return () => {
      cancelAnimationFrame(frame); resize.disconnect(); appearance.disconnect(); paneState.disconnect();
      document.removeEventListener('pointerdown', outside, true); document.removeEventListener('focusin', outside);
      document.removeEventListener('keydown', escape); window.removeEventListener('scroll', scroll, true);
      window.removeEventListener('resize', schedule); window.visualViewport?.removeEventListener('resize', schedule);
    };
  }, [open, visible, close]);
  return <>
    <button ref={trigger} type="button" className="shortcut-help-trigger" aria-label={mode === 'cards' ? '卡片快捷键说明' : '列表快捷键说明'} aria-expanded={open && visible} aria-controls={panelId} onKeyDown={event => {
      if (open && ['ArrowDown', 'PageDown'].includes(event.key)) { event.preventDefault(); event.stopPropagation(); content.current?.focus({ preventScroll: true }); }
    }} onClick={() => { if (open) close(); else { setPosition(previous => ({ ...previous, ready: false })); setOpen(true); } }}><CircleHelp size={14} /><span>快捷键</span></button>
    {open && visible && createPortal(<section ref={panel} id={panelId} className="shortcut-help" role="region" aria-labelledby={headingId} style={{ position: 'fixed', left: position.left, top: position.top, width: position.width, maxHeight: position.maxHeight, visibility: position.ready ? 'visible' : 'hidden' }}>
      <header ref={heading}><h2 id={headingId}>{mode === 'cards' ? '卡片快捷键' : '列表快捷键'}</h2><button type="button" className="shortcut-help-close" aria-label="关闭快捷键说明" onClick={() => close(true)}><X size={16} /></button></header>
      <div ref={content} className="shortcut-help-content" role="group" aria-label="快捷键操作说明" tabIndex={0}><p>先将焦点移入{mode === 'cards' ? '卡片' : '列表行'}；输入框保留原有快捷键。</p><dl>{shortcutHelpRows(mode).map(row => <div key={row.action}><dt>{row.keys.map(key => <kbd key={key}>{key}</kbd>)}</dt><dd>{row.action}</dd></div>)}</dl><p>选中后拖入左侧分类。点击名称打开详情，点击星标收藏。</p></div>
    </section>, document.body)}
  </>;
}
