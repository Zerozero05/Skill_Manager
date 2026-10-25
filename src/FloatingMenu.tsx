import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

export type MenuItem = { id: string; label: ReactNode; title?: string; disabled?: boolean; danger?: boolean; action: () => void };
export type OpenMenu = { id: string; anchor: HTMLElement; label: string; items: MenuItem[]; side?: 'right' | 'bottom'; last?: boolean };

/** A single portalled menu avoids clipping and never changes the sidebar layout. */
export function FloatingMenu({ menu, onClose }: { menu: OpenMenu; onClose: (restoreFocus?: boolean) => void }) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0, maxHeight: 400, ready: false });
  useLayoutEffect(() => {
    const node = panel.current;
    if (!node) return;
    const zoom = Number(getComputedStyle(document.documentElement).getPropertyValue('--ui-zoom')) || 1;
    const anchor = menu.anchor.getBoundingClientRect();
    const width = node.getBoundingClientRect().width / zoom;
    const height = node.getBoundingClientRect().height / zoom;
    const viewportWidth = window.innerWidth / zoom, viewportHeight = window.innerHeight / zoom;
    const gap = 6, margin = 8;
    let left = menu.side === 'right' ? anchor.right / zoom + gap : anchor.left / zoom;
    let top = menu.side === 'right' ? anchor.top / zoom : anchor.bottom / zoom + gap;
    if (left + width > viewportWidth - margin && menu.side === 'right') left = anchor.left / zoom - width - gap;
    left = Math.max(margin, Math.min(left, viewportWidth - width - margin));
    if (top + height > viewportHeight - margin) top = menu.side === 'right' ? viewportHeight - height - margin : anchor.top / zoom - height - gap;
    top = Math.max(margin, top);
    setPosition({ left, top, maxHeight: Math.max(80, viewportHeight - top - margin), ready: true });
    const outside = (event: PointerEvent) => {
      if (!node.contains(event.target as Node) && !menu.anchor.contains(event.target as Node)) onClose(false);
    };
    const dismiss = (event: Event) => { if (node.contains(event.target as Node) && (event.type === 'scroll' || event.type === 'wheel' && !(event as WheelEvent).ctrlKey)) return; onClose(false); };
    document.addEventListener('pointerdown', outside, true);
    window.addEventListener('resize', dismiss);
    window.addEventListener('wheel', dismiss, true);
    // Let the trigger's pending reveal-scroll finish before scroll can dismiss the
    // newly positioned menu. Pointer, wheel and resize dismissal stay immediate.
    let scrollFrame = 0;
    const openingFrame = requestAnimationFrame(() => { scrollFrame = requestAnimationFrame(() => window.addEventListener('scroll', dismiss, true)); });
    return () => {
      cancelAnimationFrame(openingFrame); cancelAnimationFrame(scrollFrame);
      document.removeEventListener('pointerdown', outside, true);
      window.removeEventListener('resize', dismiss);
      window.removeEventListener('scroll', dismiss, true);
      window.removeEventListener('wheel', dismiss, true);
    };
  }, [menu, onClose]);
  useLayoutEffect(() => {
    if (!position.ready) return;
    const items = Array.from(panel.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') || []);
    (menu.last ? items.at(-1) : items[0])?.focus({ preventScroll: true });
    if (menu.last && panel.current) panel.current.scrollTop = panel.current.scrollHeight;
  }, [menu, position.ready]);
  return createPortal(<div ref={panel} className="floating-menu" role="menu" aria-label={menu.label} style={{ position: 'fixed', left: position.left, top: position.top, maxHeight: position.maxHeight, visibility: position.ready ? 'visible' : 'hidden' }} onKeyDown={event => {
    const items = Array.from(panel.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') || []);
    const index = items.indexOf(document.activeElement as HTMLButtonElement);
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); event.stopPropagation();
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    } else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(true); }
    else if (event.key === 'Tab') { onClose(false); }
  }}>{menu.items.map(item => <button className={`floating-menu-item ${item.danger ? 'danger' : ''}`} role="menuitem" key={item.id} title={item.title} disabled={item.disabled} onClick={() => { onClose(true); item.action(); }}>{item.label}</button>)}</div>, document.body);
}
