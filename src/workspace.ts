/** Workspace persistence deliberately contains navigation only. Operation drafts stay in memory. */
export const PANE_KINDS = ['home', 'detail', 'operation', 'runtime', 'export', 'preview', 'receipts', 'audit', 'history', 'task', 'local', 'github', 'associate'] as const;
export type PaneKind = typeof PANE_KINDS[number];
export type PaneNavigationState = {
  section?: string; documentPath?: string; documentVersion?: 'installed' | 'latest';
  view?: string; clientId?: string; taskId?: string; operation?: string;
  scrollTop?: number; outlineExpanded?: boolean; expandedSections?: string[]; reviewRequired?: boolean;
};
export type WorkspacePane = { id: string; kind: PaneKind; title?: string; skillId?: string; state: PaneNavigationState };
export type WindowBounds = { x?: number; y?: number; width: number; height: number; maximized?: boolean };
export type SplitView = { rightPaneId: string; ratio: number };
export type WindowWorkspace = { label: string; panes: WorkspacePane[]; activePaneId: string; bounds?: WindowBounds; splitView?: SplitView };
export type WorkspaceNavigation = { revision: number; windows: WindowWorkspace[]; transfers?: unknown[] };
export type ScreenPoint = { screenX: number; screenY: number };
export type OutlineEntry = { id: string; label: string; level: number };
export type PaneTaskBadge = { status: 'running' | 'queued' | 'succeeded' | 'partial' | 'failed' | 'cancelled' | 'interrupted'; label?: string; count?: number };

const text = (value: unknown, max = 200): string | undefined => typeof value === 'string' && value.trim() ? value.trim().slice(0, max) : undefined;
const record = (value: unknown): Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
const transientKinds = new Set<PaneKind>(['operation', 'runtime', 'export', 'preview', 'receipts', 'audit', 'associate', 'local', 'github']);
export const DEFAULT_SPLIT_RATIO = 0.55;
export const MIN_SPLIT_PANE_WIDTH = 320;
export const SPLIT_DIVIDER_WIDTH = 6;

export function createHomePane(id = 'home'): WorkspacePane { return { id, kind: 'home', title: '工作台', state: {} }; }

export function navigationState(value: unknown): PaneNavigationState {
  const raw = record(value); const state: PaneNavigationState = {};
  for (const key of ['section', 'documentPath', 'view', 'clientId', 'taskId', 'operation'] as const) { const item = text(raw[key], key === 'documentPath' ? 1024 : 200); if (item) state[key] = item; }
  if (raw.documentVersion === 'installed' || raw.documentVersion === 'latest') state.documentVersion = raw.documentVersion;
  if (typeof raw.scrollTop === 'number' && Number.isFinite(raw.scrollTop)) state.scrollTop = Math.max(0, raw.scrollTop);
  if (typeof raw.outlineExpanded === 'boolean') state.outlineExpanded = raw.outlineExpanded;
  if (typeof raw.reviewRequired === 'boolean') state.reviewRequired = raw.reviewRequired;
  if (Array.isArray(raw.expandedSections)) state.expandedSections = [...new Set(raw.expandedSections.filter((item): item is string => typeof item === 'string').map(item => item.trim()).filter(item => !!item && item.length <= 4096))].slice(0, 100);
  return state;
}

/** Restoring an operation restores its location, never an executable preview or selected attachments. */
export function restorePane(value: unknown): WorkspacePane | undefined {
  const raw = record(value); const id = text(raw.id); const kind = raw.kind as PaneKind;
  if (!id || !PANE_KINDS.includes(kind)) return undefined;
  const pane: WorkspacePane = { id, kind, state: navigationState(raw.state) };
  const title = text(raw.title); if (title) pane.title = title;
  const skillId = text(raw.skillId); if (skillId) pane.skillId = skillId;
  if (transientKinds.has(kind)) pane.state.reviewRequired = true;
  if (kind === 'history') delete pane.state.reviewRequired;
  return pane;
}

/** Opening history from the sidebar reuses this window's view; explicit pane transfers stay generic. */
export function localHistoryPane(workspace: Pick<WorkspaceNavigation, 'windows'>, windowLabel: string): WorkspacePane | undefined {
  return workspace.windows.find(window => window.label === windowLabel)?.panes.find(pane => pane.kind === 'history');
}

export function normalizePanes(panes: readonly WorkspacePane[], homeId = 'home'): WorkspacePane[] {
  const home = panes.find(pane => pane.kind === 'home') || createHomePane(homeId);
  const seen = new Set([home.id]);
  return [home, ...panes.filter(pane => { if (pane.kind === 'home' || seen.has(pane.id)) return false; seen.add(pane.id); return true; })];
}

export function splitViewForPanes(value: unknown, panes: readonly WorkspacePane[]): SplitView | undefined {
  const raw = record(value); const rightPaneId = text(raw.rightPaneId);
  if (!rightPaneId || !panes.some(pane => pane.id === rightPaneId && pane.kind !== 'home') || !panes.some(pane => pane.kind === 'home')) return undefined;
  const ratio = typeof raw.ratio === 'number' && Number.isFinite(raw.ratio) ? Math.max(0.1, Math.min(0.9, raw.ratio)) : DEFAULT_SPLIT_RATIO;
  return { rightPaneId, ratio };
}

/** Logical CSS pixels: narrow windows keep both panes usable and scroll horizontally. */
export function splitRatioForPosition(left: number, availableWidth: number): number {
  const width = Math.max(MIN_SPLIT_PANE_WIDTH * 2, Number.isFinite(availableWidth) ? availableWidth : 0);
  const position = Number.isFinite(left) ? left : width * DEFAULT_SPLIT_RATIO;
  return Math.max(0.1, Math.min(0.9, Math.max(MIN_SPLIT_PANE_WIDTH, Math.min(width - MIN_SPLIT_PANE_WIDTH, position)) / width));
}

export function visibleWorkspacePaneIds(panes: readonly WorkspacePane[], activePaneId: string, splitView?: SplitView): string[] {
  const split = splitViewForPanes(splitView, panes);
  if (split) return [panes.find(pane => pane.kind === 'home')!.id, split.rightPaneId];
  const active = panes.find(pane => pane.id === activePaneId) || panes[0];
  return active ? [active.id] : [];
}

export function restoreWindow(value: unknown, fallbackLabel = 'main'): WindowWorkspace {
  const raw = record(value); const label = text(raw.label) || fallbackLabel;
  const panes = normalizePanes((Array.isArray(raw.panes) ? raw.panes : []).map(restorePane).filter((pane): pane is WorkspacePane => !!pane), label === 'main' ? 'home' : `home:${label}`);
  const selected = text(raw.activePaneId); const window: WindowWorkspace = { label, panes, activePaneId: panes.some(pane => pane.id === selected) ? selected! : panes[0].id };
  const splitView = splitViewForPanes(raw.splitView, panes); if (splitView) window.splitView = splitView;
  const bounds = record(raw.bounds);
  if (typeof bounds.width === 'number' && Number.isFinite(bounds.width) && typeof bounds.height === 'number' && Number.isFinite(bounds.height)) {
    window.bounds = { width: Math.max(640, Math.round(bounds.width)), height: Math.max(480, Math.round(bounds.height)) };
    for (const key of ['x', 'y'] as const) if (typeof bounds[key] === 'number' && Number.isFinite(bounds[key])) window.bounds[key] = Math.round(bounds[key]);
    if (typeof bounds.maximized === 'boolean') window.bounds.maximized = bounds.maximized;
  }
  return window;
}

export function workspaceForStorage(value: unknown): WorkspaceNavigation {
  const raw = record(value); const labels = new Set<string>();
  const windows = (Array.isArray(raw.windows) ? raw.windows : []).map((window, index) => restoreWindow(window, index ? `window-${index}` : 'main')).filter(window => { if (labels.has(window.label)) return false; labels.add(window.label); return true; });
  return { revision: typeof raw.revision === 'number' && Number.isSafeInteger(raw.revision) && raw.revision >= 0 ? raw.revision : 0, windows: windows.length ? windows : [restoreWindow({})] };
}

/** Reopening a Skill reuses its detail pane; distinct operation sessions keep distinct IDs. */
export function openWorkspacePane(window: WindowWorkspace, pane: WorkspacePane): WindowWorkspace {
  const existing = window.panes.find(item => item.id === pane.id || pane.kind === 'detail' && item.kind === 'detail' && !!pane.skillId && item.skillId === pane.skillId);
  return activateWorkspacePane(existing ? window : { ...window, panes: normalizePanes([...window.panes, pane]) }, existing?.id || pane.id);
}

export function activateWorkspacePane(window: WindowWorkspace, paneId: string): WindowWorkspace {
  const pane = window.panes.find(item => item.id === paneId); if (!pane) return window;
  return { ...window, activePaneId: paneId, ...(window.splitView && pane.kind !== 'home' ? { splitView: { ...window.splitView, rightPaneId: paneId } } : {}) };
}

export function closeWorkspacePane(window: WindowWorkspace, paneId: string): WindowWorkspace {
  const index = window.panes.findIndex(pane => pane.id === paneId);
  if (index < 0 || window.panes[index].kind === 'home') return window;
  const panes = window.panes.filter(pane => pane.id !== paneId);
  const activePaneId = window.activePaneId === paneId ? panes[Math.max(0, index - 1)].id : window.activePaneId;
  const next = { ...window, panes, activePaneId };
  if (window.splitView?.rightPaneId === paneId) {
    const replacement = panes.find(pane => pane.id === activePaneId && pane.kind !== 'home') || panes.find(pane => pane.kind !== 'home');
    if (replacement) next.splitView = { ...window.splitView, rightPaneId: replacement.id }; else delete next.splitView;
  }
  return next;
}

/** beforeId omitted means end. Home stays first, including attempts to drop before it. */
export function reorderWorkspacePanes(panes: readonly WorkspacePane[], paneId: string, beforeId?: string): WorkspacePane[] {
  const normalized = normalizePanes(panes); const moving = normalized.find(pane => pane.id === paneId);
  if (!moving || moving.kind === 'home' || paneId === beforeId) return normalized;
  const rest = normalized.filter(pane => pane.id !== paneId); const before = rest.findIndex(pane => pane.id === beforeId);
  rest.splice(before < 0 ? rest.length : Math.max(1, before), 0, moving); return rest;
}

export function nextPaneId(panes: readonly WorkspacePane[], activePaneId: string, reverse = false): string {
  const ordered = normalizePanes(panes); const index = Math.max(0, ordered.findIndex(pane => pane.id === activePaneId));
  return ordered[(index + (reverse ? ordered.length - 1 : 1)) % ordered.length].id;
}

export type WorkspaceShortcut = 'next' | 'previous' | 'close' | undefined;
export function workspaceShortcut(event: { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean; isComposing?: boolean; defaultPrevented?: boolean }, blockingDialog = false): WorkspaceShortcut {
  if (event.defaultPrevented || event.isComposing || blockingDialog || !event.ctrlKey || event.altKey || event.metaKey) return undefined;
  if (event.key === 'Tab') return event.shiftKey ? 'previous' : 'next';
  if (event.key.toLowerCase() === 'w' && !event.shiftKey) return 'close';
  return undefined;
}

/** Occurrence is counted within the semantic key, never within the whole directory. */
export function outlineId(paneId: string, label: string, occurrence: number, context = 'section'): string {
  const safePane = encodeURIComponent(paneId);
  const slug = label.trim().toLocaleLowerCase().replace(/[^\p{L}\p{N}_-]+/gu, '-').replace(/^-|-$/g, '').slice(0, 60) || 'section';
  return `pane-${safePane}-${encodeURIComponent(context)}-${occurrence}-${slug}`;
}

/** A document link keeps its readable fragment even after assigning pane-scoped DOM IDs. */
export function markdownHeadingAnchor(label: string, occurrence = 0): string {
  const slug = label.trim().toLocaleLowerCase().replace(/[^\p{L}\p{N}\p{M}\s_-]/gu, '').replace(/\s/g, '-');
  return occurrence ? `${slug}-${occurrence}` : slug;
}

/** Pane IDs may themselves contain percent escapes. Resolve their literal value first. */
export function paneAnchorIndex(nodes: readonly { id: string; originalId?: string; outlineAnchor?: string }[], anchor: string): number {
  const raw = anchor.replace(/^#/, ''); let decoded = raw;
  try { decoded = decodeURIComponent(raw); } catch { /* Malformed document fragments stay literal. */ }
  for (const value of raw === decoded ? [raw] : [raw, decoded]) {
    for (const key of ['id', 'originalId', 'outlineAnchor'] as const) {
      const index = nodes.findIndex(node => node[key] === value); if (index >= 0) return index;
    }
  }
  return -1;
}

/** DOM rectangles include CSS zoom; scrollTop and offsetHeight use logical pixels. */
export function paneScrollScale(rectHeight: number, offsetHeight: number): number {
  const scale = rectHeight / offsetHeight;
  return Number.isFinite(scale) && scale > 0 ? scale : 1;
}

export function paneAnchorScrollTop(current: number, targetTop: number, viewportTop: number, scale: number, occlusion: number, max: number): number {
  return Math.max(0, Math.min(max, current + (targetTop - viewportTop - occlusion) / scale - 16));
}

export function paneTitle(pane: WorkspacePane): string {
  return pane.title || ({ home: '工作台', detail: 'Skill 详情', operation: '操作', runtime: '运行文件', export: '导出', preview: '变化预览', receipts: '恢复附件', audit: '检查结果', history: '操作记录', task: '任务结果', local: '添加本地 Skill', github: '添加 GitHub 来源', associate: '关联来源' } satisfies Record<PaneKind, string>)[pane.kind];
}
