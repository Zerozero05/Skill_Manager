import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { bridgeMessage, isBridgeFailure, RecoverableSubscription, retryBridgeRead, selectionWriteConfirmed, shouldPeekSelection } from './bridge-recovery';

export type ExportSelectionDraft = { id: string; skillId: string; revision: number; selectedIds: string[]; version: number };
type SelectionReply = { draft?: ExportSelectionDraft | null; leaseId?: string; cleared?: boolean; version?: number; leaseValid?: boolean };
type SelectionEvent = { skillId: string; version: number; draft: ExportSelectionDraft | null };
type Context = {
  skillId: string; paneId: string; alive: boolean; leaseId?: string; draft?: ExportSelectionDraft;
  version: number; ready: Promise<void>; tail: Promise<void>; failure?: unknown; operations: number;
  ipcAvailable: boolean; subscription?: RecoverableSubscription; updatePolling?: () => void;
};
const request = <T,>(action: string, args: Record<string, unknown>) => invoke<T>('manager_request', { request: { action, ...args } });
const copy = (draft: ExportSelectionDraft): ExportSelectionDraft => ({ ...draft, selectedIds: [...draft.selectedIds] });

/** One memory-only draft per Skill, shared by its files/export panes and windows. */
export function useExportSelection({ skillId, paneId, enabled, visible = true }: { skillId?: string | null; paneId: string; enabled: boolean; visible?: boolean }) {
  const [draft, setDraft] = useState<ExportSelectionDraft | null>(null);
  const [loading, setLoading] = useState(false), [pending, setPending] = useState(false), [error, setError] = useState('');
  const context = useRef<Context | null>(null), visibility = useRef(visible); visibility.current = visible;
  const active = useCallback((target: Context) => target.alive && context.current === target, []);
  const accept = useCallback((target: Context, next: ExportSelectionDraft | null, version: number) => {
    if (!active(target) || version < target.version) return;
    target.version = version; target.draft = next ? copy(next) : undefined;
    setDraft(next ? copy(next) : null);
  }, [active]);
  const load = useCallback(async (target: Context) => {
    const reply = await retryBridgeRead(() => request<SelectionReply>('workspace_get_export_selection', { skillId: target.skillId, paneId: target.paneId }), { active: () => active(target) });
    if (!reply.draft || !reply.leaseId) throw new Error('导出选择暂不可用，请重新核对。');
    target.leaseId = reply.leaseId;
    // A late registration releases only its own lease, never its replacement.
    if (!active(target)) { target.draft = copy(reply.draft); return; }
    target.ipcAvailable = true;
    accept(target, reply.draft, reply.draft.version);
  }, [accept, active]);
  const read = useCallback(async (target: Context, registerMissing: boolean) => {
    const reply = await retryBridgeRead(() => request<SelectionReply>('workspace_read_export_selection', { skillId: target.skillId, paneId: target.paneId, draftId: target.draft?.id, leaseId: target.leaseId }), { active: () => active(target) });
    if (!active(target)) return;
    target.ipcAvailable = true;
    if (!reply.leaseValid || !reply.draft) {
      if (!registerMissing) throw new Error('本次导出选择已更新，请重新核对。');
      await load(target);
    } else accept(target, reply.draft, reply.version ?? reply.draft.version);
  }, [accept, active, load]);
  const enqueue = useCallback(<T,>(run: (target: Context) => Promise<T>, interactive = true): Promise<T> => {
    const target = context.current;
    if (!target?.alive) return Promise.reject(new Error('当前页面没有可用的导出选择。'));
    if (interactive) { target.operations++; setPending(true); }
    const work = target.tail.then(async () => {
      await target.ready;
      if (!active(target)) throw new Error('页面已切换，本次选择未继续修改。');
      const value = await run(target);
      target.failure = undefined;
      if (active(target)) setError('');
      return value;
    }).catch(cause => {
      target.failure = cause;
      if (isBridgeFailure(cause)) { target.ipcAvailable = false; target.updatePolling?.(); }
      if (active(target)) setError(bridgeMessage(cause));
      throw cause;
    }).finally(() => {
      if (interactive) { target.operations--; if (active(target)) setPending(target.operations > 0); }
    });
    target.tail = work.then(() => undefined, () => undefined);
    return work;
  }, [active]);
  const refresh = useCallback(() => enqueue(async target => {
    target.ipcAvailable = true;
    void target.subscription?.recover();
    await read(target, true);
    target.updatePolling?.();
    return copy(target.draft!);
  }), [enqueue, read]);
  const update = useCallback((args: Record<string, unknown>) => enqueue(async target => {
    await read(target, true);
    const before = copy(target.draft!);
    let reply: SelectionReply;
    try {
      // This write is deliberately not wrapped in retryBridgeRead.
      reply = await request<SelectionReply>('workspace_update_export_selection', { skillId: target.skillId, paneId: target.paneId, draftId: before.id, leaseId: target.leaseId, ...args });
    } catch (cause) {
      if (!isBridgeFailure(cause)) throw cause;
      // Acknowledgement loss does not imply write failure. Read its actual result.
      await read(target, false);
      if (!target.draft || !selectionWriteConfirmed(before, target.draft, args)) throw new Error('选择结果已重新读取，请核对当前勾选后再操作；本次操作未重复提交。');
      reply = { draft: copy(target.draft), cleared: args.operation === 'clear' };
    }
    if (reply.draft) accept(target, reply.draft, reply.draft.version);
    return reply;
  }), [accept, enqueue, read]);
  const toggle = useCallback((resourceId: string, included: boolean) => update({ operation: 'toggle', resourceId, included }).then(() => undefined), [update]);
  const set = useCallback((selectedIds: string[]) => update({ operation: 'set', selectedIds }).then(() => undefined), [update]);
  const reset = useCallback(() => update({ operation: 'clear' }).then(() => undefined), [update]);
  const clearIfCurrent = useCallback((snapshot: ExportSelectionDraft) => update({ operation: 'clear', draftId: snapshot.id, expectedRevision: snapshot.revision }).then(reply => reply.cleared === true), [update]);
  const flush = useCallback(async () => {
    const target = context.current;
    if (!target?.alive) throw new Error('当前页面没有可用的导出选择。');
    await target.ready;
    for (;;) { const tail = target.tail; await tail; if (tail === target.tail) break; }
    if (!active(target)) throw new Error('页面已切换，请返回当前导出页。');
    // Events are an acceleration path; export always checks the authoritative snapshot.
    return enqueue(async current => { await read(current, true); return copy(current.draft!); });
  }, [active, enqueue, read]);
  useEffect(() => {
    setDraft(null); setError(''); setPending(false);
    if (!enabled || !skillId) { context.current = null; setLoading(false); return; }
    const target: Context = { skillId, paneId, alive: true, version: -1, ready: Promise.resolve(), tail: Promise.resolve(), operations: 0, ipcAvailable: true };
    context.current = target; setLoading(true);
    let timer: ReturnType<typeof setInterval> | undefined, polling = false;
    const stopPolling = () => { if (timer) clearInterval(timer); timer = undefined; };
    const canPeek = () => shouldPeekSelection({ alive: active(target), visible: visibility.current && document.visibilityState !== 'hidden', owned: enabled, ipcAvailable: target.ipcAvailable, connected: target.subscription?.connected ?? false });
    target.updatePolling = () => {
      if (!canPeek()) { stopPolling(); return; }
      if (!timer) timer = setInterval(() => {
        if (!canPeek()) { stopPolling(); return; }
        if (polling) return;
        polling = true;
        void enqueue(current => read(current, false), false).catch(() => { target.ipcAvailable = false; stopPolling(); }).finally(() => { polling = false; });
      }, 2000);
    };
    // Never gate the first snapshot on a WebView event subscription.
    target.ready = load(target).catch(cause => {
      target.failure = cause; target.ipcAvailable = false;
      if (active(target)) setError(bridgeMessage(cause));
    }).finally(() => { if (active(target)) setLoading(false); target.updatePolling?.(); });
    target.subscription = new RecoverableSubscription(() => listen<SelectionEvent>('workspace-export-selection-changed', event => {
      if (event.payload.skillId === target.skillId) accept(target, event.payload.draft, event.payload.version);
    }), {
      active: () => active(target),
      onReady: () => {
        stopPolling();
        // Queue after the initial snapshot, catching changes before listen became ready.
        void target.ready.then(() => {
          if (active(target) && target.ipcAvailable) return enqueue(current => read(current, true), false);
        }).catch(() => undefined);
      },
      onUnavailable: () => target.updatePolling?.(),
    });
    void target.subscription.recover();
    const focus = () => { if (active(target) && visibility.current && document.visibilityState !== 'hidden') void refresh().catch(() => undefined); };
    const visibilityChanged = () => { target.updatePolling?.(); if (document.visibilityState !== 'hidden') focus(); };
    window.addEventListener('focus', focus); document.addEventListener('visibilitychange', visibilityChanged);
    return () => {
      target.alive = false; stopPolling(); target.subscription?.dispose();
      window.removeEventListener('focus', focus); document.removeEventListener('visibilitychange', visibilityChanged);
      if (context.current === target) context.current = null;
      void Promise.allSettled([target.ready, target.tail]).then(async () => {
        if (target.draft && target.leaseId) await request('workspace_release_export_selection', { skillId: target.skillId, paneId: target.paneId, draftId: target.draft.id, leaseId: target.leaseId });
      }).catch(() => undefined);
    };
  }, [skillId, paneId, enabled, accept, active, enqueue, load, read, refresh]);
  useEffect(() => { context.current?.updatePolling?.(); }, [visible]);
  return { draft, selectedIds: draft?.selectedIds || [], loading, pending, error, active: enabled && !!skillId, toggle, set, reset, clearIfCurrent, refresh, flush };
}
