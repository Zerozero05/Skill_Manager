import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import SessionApp from './App';
import { useTasks, terminalTask } from './tasks';
import { WorkspaceShell } from './WorkspaceShell';
import { createHomePane, localHistoryPane, reorderWorkspacePanes, type WorkspaceNavigation, type WorkspacePane, type ScreenPoint, type PaneTaskBadge } from './workspace';
import { WorkspaceSessionContext, type PaneFocusNavigation, type SessionBridge, type SessionDraft } from './session';
import './session.css';
import { isStaleReadSnapshot } from './snapshot-clock';
import { acknowledgeCloseRound, bridgeMessage, pendingCloseRound, readCloseBounds, RecoverableSubscription, retryBridgeRead, shouldPollWorkspace } from './bridge-recovery';

type Transfer = { id: string; paneId: string; sourceWindowLabel: string; targetWindowLabel: string };
type CloseRequest = { closeId: string; round?: string; preserveRuntime?: boolean };
type WorkspaceReply = { windowLabel: string; workspace: WorkspaceNavigation; nativeCloseRequested?: boolean; closing?: boolean; ownedCloseId?: string | null; closeRequest?: CloseRequest | null };
const api = <T,>(action: string, args: Record<string, unknown> = {}) => invoke<T>('manager_request', { request: { action, ...args } });
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export default function WorkspaceApp() {
  const label = useMemo(() => isTauri() ? getCurrentWindow().label : 'main', []);
  const [workspace, setWorkspace] = useState<WorkspaceNavigation>({ revision: -1, windows: [{ label, panes: [createHomePane(label === 'main' ? 'home' : `home:${label}`)], activePaneId: label === 'main' ? 'home' : `home:${label}` }] });
  const current = useRef(workspace); current.current = workspace;
  const [managerSnapshot, setManagerSnapshot] = useState<unknown>();
  const managerRevision = useRef(-1);
  const readGeneration = useRef<number | null | undefined>(undefined);
  const readGenerationWatermark = useRef<number | undefined>(undefined);
  const tasks = useTasks();
  const [drafts, setDrafts] = useState<Record<string, SessionDraft>>({});
  const loaded = useRef(new Set<string>());
  const bridges = useRef(new Map<string, SessionBridge>());
  const [error, setError] = useState('');
  const [closing, setClosing] = useState(false);
  const closingRef = useRef(false);
  const updateClosePolling = useRef<() => void>(() => undefined);
  const [showHelp, setShowHelp] = useState(false);
  const ready = useRef(false);
  const mounted = useRef(true);
  const accepting = useRef(Promise.resolve());
  const pendingSaves = useRef(new Set<Promise<unknown>>());
  const acceptedTransfers = useRef(new Set<string>());
  const loadedTransfers = useRef(new Map<string, string>());
  const moving = useRef(new Set<string>());
  const [movingIds, setMovingIds] = useState<ReadonlySet<string>>(new Set());
  const openingHistory = useRef(false);
  const openingTaskResults = useRef(new Set<string>());
  const openingDetails = useRef(new Set<string>());
  useEffect(() => {
    if (!isTauri()) return;
    const root = document.documentElement, window = getCurrentWindow();
    let disposed = false, requested = '', pending = Promise.resolve();
    const apply = () => {
      if (!['light', 'dark', 'eye'].includes(root.dataset.theme || '')) return;
      const theme = root.dataset.theme === 'dark' ? 'dark' : 'light';
      if (theme === requested) return;
      requested = theme;
      pending = pending.then(async () => {
        if (disposed) return;
        try { await window.setTheme(theme); }
        catch (cause) { if (!disposed) setError(`窗口主题未应用：${errorText(cause)}`); }
      });
    };
    const observer = new MutationObserver(apply);
    observer.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
    apply();
    return () => { disposed = true; observer.disconnect(); };
  }, []);
  const attemptedClientRepairs = useRef(new Map<string, number>());
  const observedContextTasks = useRef(new Set<string>());

  const track = useCallback(<T,>(promise: Promise<T>) => {
    pendingSaves.current.add(promise);
    void promise.finally(() => pendingSaves.current.delete(promise)).catch(() => undefined);
    return promise;
  }, []);
  const acceptSnapshot = useCallback((value: unknown) => {
    const next = value as { stateRevision?: number; readGeneration?: number | null; skills?: unknown[]; sources?: unknown[]; connections?: unknown[] };
    if (!next || !Array.isArray(next.skills) || !Array.isArray(next.sources) || !Array.isArray(next.connections)) return;
    const revision = next.stateRevision ?? 0;
    if (isStaleReadSnapshot(next, managerRevision.current, readGenerationWatermark.current) || revision === managerRevision.current && next.readGeneration === readGeneration.current) return;
    if (typeof next.readGeneration === 'number') readGenerationWatermark.current = next.readGeneration;
    readGeneration.current = next.readGeneration;
    managerRevision.current = revision; setManagerSnapshot(next);
  }, []);
  useEffect(() => {
    const snapshot = managerSnapshot as { connections?: { id: string; path: string; target: string; autoRepairable?: boolean; checkedAt?: string }[] } | undefined;
    const candidates = snapshot?.connections?.filter(connection => connection.autoRepairable) || [];
    if (!candidates.length || closingRef.current) return;
    const key = JSON.stringify(candidates.map(connection => [connection.id, connection.path, connection.target]));
    const previous = attemptedClientRepairs.current.get(key);
    if (previous !== undefined && Date.now() - previous < 30 * 60_000) return;
    attemptedClientRepairs.current.set(key, Date.now());
    if (attemptedClientRepairs.current.size > 64) attemptedClientRepairs.current.delete(attemptedClientRepairs.current.keys().next().value!);
    // The backend treats repair as an exclusive write task and independently proves every entry.
    void tasks.run('正在安全修复客户端入口', 'repair_client_connections', { connectionIds: candidates.map(connection => connection.id) }, { view: 'clients' }, false)
      .then(acceptSnapshot).catch(cause => { if (mounted.current) setError(`客户端入口自动修复未完成：${errorText(cause)}。可在客户端页面重新检查。`); });
  }, [managerSnapshot, tasks.run, acceptSnapshot]);
  const acceptWorkspace = useCallback((next: WorkspaceNavigation) => {
    accepting.current = accepting.current.then(async () => {
      if (!mounted.current || next.revision < current.current.revision) return;
      const owner = next.windows.find(window => window.label === label);
      const incoming = (next.transfers || []) as Transfer[];
      const targets = incoming.filter(item => item.targetWindowLabel === label);
      const panes = [...(owner?.panes || []), ...targets.map(item => next.windows.find(window => window.label === item.sourceWindowLabel)?.panes.find(pane => pane.id === item.paneId)).filter((pane): pane is WorkspacePane => !!pane)];
      const values: Record<string, SessionDraft> = {};
      const localPaneIds = new Set(panes.map(pane => pane.id));
      for (const id of loaded.current) if (!localPaneIds.has(id)) { loaded.current.delete(id); loadedTransfers.current.delete(id); }
      for (const pane of panes) {
        const transfer = targets.find(item => item.paneId === pane.id);
        const refreshTransfer = transfer && loadedTransfers.current.get(pane.id) !== transfer.id;
        if (loaded.current.has(pane.id) && !refreshTransfer) continue;
        const result = await retryBridgeRead(() => api<{ runtime?: SessionDraft }>('workspace_get_pane_runtime', { paneId: pane.id }), { active: () => mounted.current });
        values[pane.id] = result.runtime || {}; loaded.current.add(pane.id);
        if (transfer) loadedTransfers.current.set(pane.id, transfer.id);
      }
      if (!mounted.current || next.revision < current.current.revision) return;
      setDrafts(previous => ({ ...Object.fromEntries(Object.entries(previous).filter(([id]) => localPaneIds.has(id))), ...values }));
      current.current = next; setWorkspace(next);
    }).catch(cause => { if (mounted.current) setError(errorText(cause)); });
    return accepting.current;
  }, [label]);

  const refreshState = useCallback(async () => acceptSnapshot(await retryBridgeRead(() => api('get_state'), { active: () => mounted.current })), [acceptSnapshot]);
  useEffect(() => {
    // Credential storage may succeed before a later notification fails. Re-read its opaque
    // generation even on failed tasks, rather than retaining an old display cache context.
    for (const task of tasks.tasks) {
      if (!terminalTask(task) || !['set_token', 'delete_token', 'set_network_settings'].includes(task.action) || observedContextTasks.current.has(task.id)) continue;
      observedContextTasks.current.add(task.id);
      void refreshState().catch(cause => { if (mounted.current) setError(errorText(cause)); });
    }
  }, [tasks.tasks, refreshState]);
  const applyReply = useCallback(async (promise: Promise<WorkspaceReply>) => {
    try { const reply = await track(promise); await acceptWorkspace(reply.workspace); }
    catch (cause) { setError(errorText(cause)); }
  }, [acceptWorkspace, track]);
  const register = useCallback((id: string, bridge: SessionBridge) => {
    bridges.current.set(id, bridge);
    return () => { if (bridges.current.get(id) === bridge) bridges.current.delete(id); };
  }, []);
  const updateNavigation = useCallback((id: string, patch: Record<string, unknown>) => {
    if (!ready.current || closingRef.current) return;
    void applyReply(api('workspace_update_pane', { paneId: id, state: patch }));
  }, [applyReply]);
  const closePane = useCallback((id: string) => {
    if (closingRef.current) return;
    void applyReply(api('workspace_close_pane', { paneId: id }));
  }, [applyReply]);
  const focusPane = useCallback((id: string, navigation?: PaneFocusNavigation) => { void applyReply(api('workspace_focus_pane', { paneId: id, ...navigation })); }, [applyReply]);
  const openHistory = useCallback(() => {
    if (closingRef.current) return;
    const existing = localHistoryPane(current.current, label);
    if (existing) { focusPane(existing.id); return; }
    if (openingHistory.current) return;
    openingHistory.current = true;
    const pane: WorkspacePane = { id: crypto.randomUUID(), kind: 'history', title: '操作记录', state: {} };
    void applyReply(api('workspace_open_pane', { pane })).finally(() => { openingHistory.current = false; });
  }, [applyReply, focusPane, label]);
  const openDetail = useCallback((skill: { id: string; name: string; displayName?: string }, section: 'docs' | 'files' | 'source') => {
    const existing = current.current.windows.flatMap(window => window.panes).find(pane => pane.kind === 'detail' && pane.skillId === skill.id);
    if (existing) {
      void applyReply(api('workspace_focus_pane', { paneId: existing.id, section })); return;
    }
    if (openingDetails.current.has(skill.id)) return;
    openingDetails.current.add(skill.id);
    const pane: WorkspacePane = { id: crypto.randomUUID(), kind: 'detail', title: skill.displayName || skill.name, skillId: skill.id, state: { section } };
    void applyReply(api('workspace_open_pane', { pane })).finally(() => { openingDetails.current.delete(skill.id); });
  }, [applyReply]);
  const openOperation = useCallback((modal: unknown, draft: SessionDraft, title: string, skillId?: string, activate = true, parentPaneId?: string) => {
    const kind = (modal as { kind?: string })?.kind || 'operation';
    const taskId = kind === 'task' ? (modal as { task?: { id?: string } }).task?.id : undefined;
    if (taskId) {
      const existing = current.current.windows.find(window => window.label === label)?.panes.find(pane => pane.kind === 'operation' && pane.state.operation === 'task' && pane.state.taskId === taskId);
      if (existing) { focusPane(existing.id); return; }
      if (openingTaskResults.current.has(taskId)) return;
      openingTaskResults.current.add(taskId);
    }
    const clientId = kind === 'client-connect' ? (modal as { client: string }).client : undefined;
    if (clientId) {
      const existing = current.current.windows.find(window => window.label === label)?.panes.find(pane => pane.kind === 'operation' && pane.state.operation === kind && pane.state.clientId === clientId);
      if (existing) { focusPane(existing.id); return; }
    }
    const pane: WorkspacePane = { id: typeof draft.outcomeTaskId === 'string' ? `outcome-${draft.outcomeTaskId}` : crypto.randomUUID(), kind: 'operation', title, skillId, state: { operation: kind, ...(clientId ? { clientId } : {}), ...(kind === 'task' ? { taskId: ((modal as { task?: { id?: string } }).task?.id) } : {}) } };
    void applyReply(api('workspace_open_pane', { pane, runtime: { ...draft, modal }, activate, parentPaneId })).finally(() => { if (taskId) openingTaskResults.current.delete(taskId); });
  }, [applyReply, focusPane, label]);
  const goHome = useCallback((view = 'skills', clientId?: string) => {
    const owner = current.current.windows.find(window => window.label === label);
    const home = owner?.panes.find(pane => pane.kind === 'home');
    if (!home) return;
    void api<WorkspaceReply>('workspace_update_pane', { paneId: home.id, state: { view, ...(clientId ? { clientId } : {}) } }).then(reply => acceptWorkspace(reply.workspace)).then(() => focusPane(home.id)).catch(cause => setError(errorText(cause)));
  }, [acceptWorkspace, focusPane, label]);
  const movePane = useCallback(async (id: string, point?: ScreenPoint, returnToMain = false) => {
    moving.current.add(id); setMovingIds(new Set(moving.current));
    try {
      setError('');
      const bridge = bridges.current.get(id);
      if (bridge && !await bridge.flush()) throw new Error('页面设置尚未保存，请重试');
      if (bridge) await api('workspace_set_pane_runtime', { paneId: id, runtime: bridge.capture() });
      await applyReply(api(returnToMain ? 'workspace_move' : 'workspace_detach', { paneId: id, ...(returnToMain ? { targetWindowLabel: 'main' } : point ? { screenX: point.screenX, screenY: point.screenY } : {}) }));
    } catch (cause) { setError(errorText(cause)); }
    finally { moving.current.delete(id); setMovingIds(new Set(moving.current)); }
  }, [applyReply]);
  const flush = useCallback(async (preserveRuntime = false) => {
    for (const bridge of bridges.current.values()) if (!await bridge.flush()) return false;
    await Promise.all([...pendingSaves.current]);
    // Keep drafts in the shared process so a failed storage migration can
    // recreate its WebViews without losing the pages that were just closed.
    if (preserveRuntime) for (const [paneId, bridge] of bridges.current) await api('workspace_set_pane_runtime', { paneId, runtime: bridge.capture() });
    if (isTauri()) {
      const window = getCurrentWindow();
      const bounds = await readCloseBounds(async () => {
        const scale = await window.scaleFactor(), position = await window.outerPosition(), size = await window.innerSize();
        return { x: position.x / scale, y: position.y / scale, width: size.width / scale, height: size.height / scale, maximized: await window.isMaximized() };
      }, { active: () => mounted.current });
      const owner = current.current.windows.find(item => item.label === label);
      // Omitting bounds preserves the backend's last known position; pane saves still must succeed.
      if (owner) await api('workspace_save', { panes: owner.panes.map(pane => { const draft = bridges.current.get(pane.id)?.capture(); return draft ? { ...pane, state: { ...pane.state, scrollTop: draft.scrollTop, outlineExpanded: draft.outlineExpanded, expandedSections: draft.expandedSections } } : pane; }), activePaneId: owner.activePaneId, ...(bounds ? { bounds } : {}) });
    }
    return true;
  }, [label]);
  const closeWindow = useCallback(async (all = false, storagePreviewId?: string) => {
    if (closingRef.current) return false;
    closingRef.current = true; setClosing(true);
    updateClosePolling.current();
    let ownedCloseId: string | undefined;
    try {
      if (!await flush(!!storagePreviewId)) throw new Error('界面设置未保存，窗口保持打开');
      const result = await api<{ final: boolean; closeId?: string; pendingWindowLabels?: string[] }>('workspace_close_intent', { all, ...(storagePreviewId ? { storageMigration: true } : {}) });
      ownedCloseId = result.closeId;
      if (!result.final) { await getCurrentWindow().destroy(); return true; }
      // Other windows acknowledge their local drafts before the backend closes the app.
      await api('workspace_finish_close', { closeId: result.closeId, ...(storagePreviewId ? { storagePreviewId } : {}) });
      return true;
    } catch (cause) {
      if (!ownedCloseId) {
        // The intent may have succeeded without its reply. Never cancel another window's close.
        try { ownedCloseId = (await retryBridgeRead(() => api<WorkspaceReply>('workspace_get'), { active: () => mounted.current })).ownedCloseId || undefined; } catch { /* Keep an unconfirmed close untouched. */ }
      }
      if (ownedCloseId) await api('workspace_cancel_close', { closeId: ownedCloseId }).catch(() => undefined);
      else await api('workspace_cancel_close', { nativeOnly: true }).catch(() => undefined);
      closingRef.current = false; setClosing(false); setError(`窗口暂未关闭：${errorText(cause)}`);
      updateClosePolling.current();
      return false;
    }
  }, [flush]);
  const requestStorageRestart = useCallback(async (previewId: string) => {
    if (!await closeWindow(true, previewId)) throw new Error('保存或存储迁移未完成，请查看窗口中的提示。');
  }, [closeWindow]);

  useEffect(() => {
    mounted.current = true; if (!isTauri()) { setError('请在 Skill Manager 桌面程序中打开'); return; }
    let disposed = false, ipcAvailable = true, repeatSnapshot = false;
    let observedBackendClose = false;
    const inFlightCloseRounds = new Set<string>();
    const flushedCloseRounds = new Map<string, { ok: boolean; error?: string }>();
    let snapshot: Promise<void> | undefined, poll: ReturnType<typeof setInterval> | undefined;
    const subscriptions: RecoverableSubscription[] = [];
    const closeSubscriptions = new Set<RecoverableSubscription>();
    const active = () => !disposed && mounted.current;
    const stopPolling = () => { if (poll) clearInterval(poll); poll = undefined; };
    const report = (cause: unknown) => { if (!active()) return; ipcAvailable = false; stopPolling(); setError(bridgeMessage(cause)); };
    const receiveCloseRequest = (payload: CloseRequest) => {
      const key = `${payload.closeId}:${payload.round || 'initial'}`;
      if (!active() || inFlightCloseRounds.has(key)) return;
      inFlightCloseRounds.add(key);
      observedBackendClose = true; closingRef.current = true; setClosing(true);
      updatePolling();
      void (async () => {
        const peek = () => api<WorkspaceReply>('workspace_get');
        const currentClose = await retryBridgeRead(peek, { active });
        if (!pendingCloseRound(payload, currentClose)) { acceptCloseState(currentClose); return; }
        let outcome = flushedCloseRounds.get(key);
        if (!outcome) {
          try { outcome = { ok: await flush(!!payload.preserveRuntime) }; }
          catch (cause) { outcome = { ok: false, error: errorText(cause) }; }
          flushedCloseRounds.set(key, outcome);
          while (flushedCloseRounds.size > 16) flushedCloseRounds.delete(flushedCloseRounds.keys().next().value!);
        }
        const acknowledgement = { closeId: payload.closeId, round: payload.round, ...outcome };
        // Only this idempotent control ACK may be repeated, after a read of the same pending round.
        await acknowledgeCloseRound(payload, () => api('workspace_close_ack', acknowledgement), peek, active);
      })().catch(report).finally(() => { inFlightCloseRounds.delete(key); });
    };
    const acceptCloseState = (reply: WorkspaceReply) => {
      if (reply.closing) { observedBackendClose = true; closingRef.current = true; setClosing(true); }
      if (reply.closeRequest) receiveCloseRequest(reply.closeRequest);
      else if (reply.nativeCloseRequested && ready.current && !closingRef.current) void closeWindow();
      else if (reply.closing === false && observedBackendClose) {
        observedBackendClose = false; closingRef.current = false; setClosing(false);
      }
      updatePolling();
    };
    const reconcile = (afterSubscription = false, controlOnly = false): Promise<void> => {
      if (!active()) return Promise.resolve();
      if (snapshot) { repeatSnapshot ||= afterSubscription; return snapshot; }
      snapshot = (async () => {
        do {
          repeatSnapshot = false;
          const lightweight = controlOnly || closingRef.current || document.visibilityState === 'hidden';
          if (!lightweight && !closingRef.current) await refreshState();
          const reply = await retryBridgeRead(() => api<WorkspaceReply>('workspace_get'), { active });
          if (!active()) return;
          if (!lightweight) await acceptWorkspace(reply.workspace);
          acceptCloseState(reply);
          ipcAvailable = true;
        } while (repeatSnapshot && active());
      })().finally(() => { snapshot = undefined; });
      return snapshot;
    };
    const updatePolling = () => {
      const canPoll = () => shouldPollWorkspace({ alive: active(), visible: document.visibilityState !== 'hidden', ipcAvailable, degraded: subscriptions.some(item => !item.connected), closeDegraded: [...closeSubscriptions].some(item => !item.connected), closing: closingRef.current });
      if (!canPoll()) { stopPolling(); return; }
      if (!poll) poll = setInterval(() => {
        if (!canPoll()) { stopPolling(); return; }
        void reconcile(false, closingRef.current || document.visibilityState === 'hidden').catch(report);
      }, 2000);
    };
    updateClosePolling.current = updatePolling;
    const add = (register: () => Promise<() => void>, closeControl = false) => {
      const subscription = new RecoverableSubscription(register, {
        active,
        onReady: () => { if (ipcAvailable) void reconcile(true).catch(report); updatePolling(); },
        onUnavailable: () => updatePolling(),
      });
      subscriptions.push(subscription); if (closeControl) closeSubscriptions.add(subscription); void subscription.recover();
    };
    add(() => listen<WorkspaceNavigation>('workspace-changed', event => { void acceptWorkspace(event.payload); }));
    add(() => listen<{ stateRevision: number }>('state-changed', event => { if (event.payload.stateRevision > managerRevision.current) void refreshState().catch(report); }));
    add(() => listen('read-context-changed', () => { void refreshState().catch(report); }));
    add(() => listen<{ paneId: string }>('workspace-activate', event => { void applyReply(api('workspace_activate', { paneId: event.payload.paneId })); }));
    add(() => listen<CloseRequest>('workspace-close-request', event => receiveCloseRequest(event.payload)), true);
    add(() => listen('workspace-close-cancelled', () => { observedBackendClose = false; closingRef.current = false; setClosing(false); updatePolling(); }), true);
    add(() => listen('workspace-native-close-request', () => { if (ready.current) void reconcile(true).catch(report); }), true);
    add(() => getCurrentWindow().onCloseRequested(event => { event.preventDefault(); if (ready.current) void reconcile(true).catch(report); }), true);
    void reconcile().then(async () => {
      if (!active()) return;
      ready.current = true;
      if (!((current.current.transfers || []) as Transfer[]).some(item => item.targetWindowLabel === label)) {
        // Registration is a write. A failed acknowledgement is not replayed.
        const result = await api<WorkspaceReply>('workspace_ready'); await acceptWorkspace(result.workspace);
      }
      await reconcile(true);
    }).catch(report);
    const focus = () => {
      if (!active() || document.visibilityState === 'hidden') return;
      ipcAvailable = true;
      for (const item of subscriptions) void item.recover();
      void reconcile(true).then(() => { if (ready.current && !closingRef.current) void api('workspace_focus').catch(() => undefined); updatePolling(); }).catch(report);
    };
    const visibilityChanged = () => { updatePolling(); if (document.visibilityState !== 'hidden') focus(); };
    window.addEventListener('focus', focus); document.addEventListener('visibilitychange', visibilityChanged);
    return () => { disposed = true; mounted.current = false; stopPolling(); if (updateClosePolling.current === updatePolling) updateClosePolling.current = () => undefined; for (const item of subscriptions) item.dispose(); window.removeEventListener('focus', focus); document.removeEventListener('visibilitychange', visibilityChanged); };
  }, [acceptWorkspace, applyReply, closeWindow, flush, refreshState, label]);
  useEffect(() => {
    const stopEditing = (event: Event) => { if (closingRef.current) { event.preventDefault(); event.stopImmediatePropagation(); } };
    window.addEventListener('wheel', stopEditing, { capture: true, passive: false }); window.addEventListener('keydown', stopEditing, true);
    return () => { window.removeEventListener('wheel', stopEditing, true); window.removeEventListener('keydown', stopEditing, true); };
  }, []);

  const owner = workspace.windows.find(window => window.label === label);
  const transfers = (workspace.transfers || []) as Transfer[];
  const incoming = transfers.filter(transfer => transfer.targetWindowLabel === label);
  const incomingPanes = incoming.map(transfer => workspace.windows.find(window => window.label === transfer.sourceWindowLabel)?.panes.find(pane => pane.id === transfer.paneId)).filter((pane): pane is WorkspacePane => !!pane);
  const ownerPanes = owner?.panes || [createHomePane(label === 'main' ? 'home' : `home:${label}`)];
  const panes = [...ownerPanes, ...incomingPanes.filter(pane => !ownerPanes.some(item => item.id === pane.id))];
  useEffect(() => {
    if (!incoming.length) return;
    let cancelled = false;
    const frame = requestAnimationFrame(() => {
      if (cancelled) return;
      for (const transfer of incoming) {
        if (acceptedTransfers.current.has(transfer.id) || !bridges.current.has(transfer.paneId)) continue;
        acceptedTransfers.current.add(transfer.id);
        void api<WorkspaceReply>('workspace_ready', { transferId: transfer.id }).then(reply => acceptWorkspace(reply.workspace)).catch(cause => { acceptedTransfers.current.delete(transfer.id); setError(`页面转移尚未完成，原页保留：${errorText(cause)}`); });
      }
    });
    return () => { cancelled = true; cancelAnimationFrame(frame); };
  }, [workspace, drafts, acceptWorkspace]);
  const pendingTransfers = new Set([...movingIds, ...transfers.filter(transfer => transfer.sourceWindowLabel === label).map(transfer => transfer.paneId)]);
  const badges = new Map<string, PaneTaskBadge>();
  for (const task of tasks.tasks) {
    if (!task.origin.paneId) continue;
    const existing = badges.get(task.origin.paneId);
    if (existing && (['queued', 'running'].includes(existing.status) || terminalTask(task))) continue;
    badges.set(task.origin.paneId, { status: task.status as PaneTaskBadge['status'], label: task.label, count: tasks.tasks.filter(item => item.origin.paneId === task.origin.paneId && !terminalTask(item)).length });
  }
  return <div className={`desktop-workspace ${closing ? 'workspace-is-closing' : ''}`}>
    {error && <div className="workspace-error" role="alert"><span>{error}</span><button onClick={() => setError('')}>关闭提示</button></div>}
    {closing && <div className="workspace-closing" role="status">正在保存并等待关键任务完成…</div>}
    <WorkspaceShell panes={panes} activePaneId={incomingPanes.at(-1)?.id || owner?.activePaneId || panes[0].id} splitView={owner?.splitView} onSplitChange={splitView => { void applyReply(api('workspace_set_split', { splitView: splitView || null })); }} pendingTransfers={pendingTransfers} taskBadges={badges} shortcutsBlocked={closing}
      onActivate={id => { void applyReply(api('workspace_activate', { paneId: id })); }} onClose={closePane}
      onReorder={(id, beforeId) => { const window = current.current.windows.find(item => item.label === label); if (window) void applyReply(api('workspace_save', { panes: reorderWorkspacePanes(window.panes, id, beforeId), activePaneId: window.activePaneId, expectedRevision: current.current.revision })); }}
      onDetach={(id, point) => { void movePane(id, point); }} onReturn={label === 'main' ? undefined : id => { void movePane(id, undefined, true); }}
      onResetLayout={() => { void applyReply(api('workspace_reset_layout')); }} onShowHelp={() => setShowHelp(value => !value)}
      renderPane={(pane, active, visible) => <WorkspaceSessionContext.Provider key={pane.id} value={{ pane, active, visible, owned: ownerPanes.some(item => item.id === pane.id), isTransferring: () => moving.current.has(pane.id) || ((current.current.transfers || []) as Transfer[]).some(item => item.paneId === pane.id), closing, initialDraft: drafts[pane.id] || {}, managerSnapshot, tasks, acceptSnapshot, register, openDetail, openHistory, openOperation: (modal, draft, title, skillId) => openOperation(modal, { ...draft, originPaneId: pane.id }, title, skillId, active, pane.id), updateNavigation, goHome, closePane, focusPane, requestStorageRestart }}>{Object.prototype.hasOwnProperty.call(drafts, pane.id) ? <SessionApp /> : <p className="muted small-text" role="status">正在恢复页面…</p>}</WorkspaceSessionContext.Provider>} />
    {showHelp && <aside className="workspace-help" aria-label="快捷键说明"><button className="icon-button" onClick={() => setShowHelp(false)} aria-label="关闭快捷键说明">×</button><strong>快捷操作</strong><p>Ctrl＋Tab／Ctrl＋Shift＋Tab：切换标签</p><p>Ctrl＋W：关闭当前业务标签</p><p>Ctrl＋滚轮：缩放界面；Ctrl＋0：恢复 100%</p><p>Ctrl＋Z：撤销分类与收藏；输入框内优先撤销文字</p><p>左右分栏：工作台在左，当前业务页在右；拖动中间边界调整宽度，双击复位。</p><p>拖动标签排序，拖出窗口可独立显示。</p><button className="button" onClick={() => void closeWindow(true)}>退出全部窗口</button></aside>}
  </div>;
}
