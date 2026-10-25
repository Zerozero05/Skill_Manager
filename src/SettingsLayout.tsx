import { useEffect, useLayoutEffect, useRef, type ReactNode, type RefObject } from 'react';
import { PaneOutline } from './WorkspaceShell';
import './settings.css';

/** Reuses the home page scroller; settings never introduce a second content viewport. */
export function SettingsLayout({ paneId, scopeRef, expanded, onExpandedChange, children }: {
  paneId: string; scopeRef: RefObject<HTMLElement | null>; expanded: boolean;
  onExpandedChange: (expanded: boolean) => void; children: ReactNode;
}) {
  const shell = useRef<HTMLElement>(null), navigation = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    const root = shell.current, header = navigation.current;
    const scroll = scopeRef.current?.querySelector<HTMLElement>('[data-pane-scroll]') || scopeRef.current;
    if (!root || !header || !scroll) return;
    let frame = 0;
    const measure = () => {
      const padding = Number.parseFloat(getComputedStyle(scroll).paddingTop) || 0;
      root.style.setProperty('--settings-scroll-padding', `${padding}px`);
      root.style.setProperty('--settings-navigation-height', `${header.offsetHeight}px`);
      root.style.setProperty('--settings-viewport-height', `${scroll.clientHeight}px`);
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const resize = new ResizeObserver(schedule); resize.observe(scroll); resize.observe(header);
    const appearance = new MutationObserver(schedule);
    appearance.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'data-theme'] });
    measure();
    return () => { cancelAnimationFrame(frame); resize.disconnect(); appearance.disconnect(); };
  }, [scopeRef]);
  return <PaneOutline paneId={`${paneId}:settings`} scopeRef={scopeRef} compact expanded={expanded} onExpandedChange={onExpandedChange}>
    {({ toggle, list, narrow }) => <section ref={shell} className="settings-shell" aria-label="偏好设置">
      <header ref={navigation} className="settings-navigation" data-pane-sticky>{toggle}{narrow && list}</header>
      <div className="settings-sections">{!narrow && list}<div className="settings-content">{children}</div></div>
    </section>}
  </PaneOutline>;
}

export function SettingsSection({ sectionKey, title, children }: { sectionKey: string; title: string; children: ReactNode }) {
  const section = useRef<HTMLElement>(null);
  useEffect(() => {
    const decorate = () => {
      const heading = section.current?.querySelector('h2');
      if (!heading) return;
      heading.dataset.outlineKey = `settings:${sectionKey}`;
      heading.dataset.outlineLabel = title;
      heading.dataset.outlineLevel = '1';
      for (const card of section.current?.querySelectorAll<HTMLElement>('.card,.panel') || []) card.dataset.outlineScrollStart = '';
    };
    decorate(); const observer = new MutationObserver(decorate);
    if (section.current) observer.observe(section.current, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [sectionKey, title]);
  return <section ref={section} className="settings-section" aria-label={title}>{children}</section>;
}
