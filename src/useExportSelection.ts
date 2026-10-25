import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type ExportSelectionDraft = { id: string; skillId: string; revision: number; selectedIds: string[]; version: number };
type SelectionReply = { draft?: ExportSelectionDraft | null; leaseId?: string; cleared?: boolean };
type SelectionEvent = { skillId: string; version: number; draft: ExportSelectionDraft | null };
type Context = {
  skillId: string; paneId: string; alive: boolean; leaseId?: string; draft?: ExportSelectionDraft;
  version: number; ready: Promise<void>; tail: Promise<void>; failure?: unknown; operations: number;
};
const request = <T,>(action: string, args: Record<string, unknown>) => invoke<T>('manager_request', { request: { action, ...args } });
const message = (cause: unknown) => cause instanceof Error ? cause.message : String(cause);
const copy = (draft: ExportSelectionDraft): ExportSelectionDraft => ({ ...draft, selectedIds: [...draft.selectedIds] });

/** One memory-only draft per Skill, shared by its files/export panes and windows. */
export function useExportSelection({ skillId, paneId, enabled }: { skillId?: string | null; paneId: string; enabled: boolean }) {
  const [draft, setDraft] = useState<ExportSelectionDraft | null>(null);
  const [loading, setLoading] = useState(false), [pending, setPending] = useState(false), [error, setError] = useState('');
  const context = useRef<Context | null>(null);
  const accept = useCallback((target: Context, next: ExportSelectionDraft | null, version: number) => {
    if (!target.alive || context.current !== target || version < target.version) return;
    target.version = version;
    target.draft = next ? copy(next) : undefined;
    setDraft(next ? copy(next) : null);
  }, []);
  const load = useCallback(async (target: Context) => {
    const reply = await request<SelectionReply>('workspace_get_export_selection', { skillId: target.skillId, paneId: target.paneId });
    if (!reply.draft || !reply.leaseId) throw new Error('导出选择暂不可用，请重新打开页面。');
    target.leaseId = reply.leaseId;
    // Keep the lease even after teardown so its late cleanup can release only
    // that registration, never the new effect's registration for the same pane.
    if (!target.alive) { target.draft = copy(reply.draft); return; }
    accept(target, reply.draft, reply.draft.version);
  }, [accept]);
  useEffect(() => {
    setDraft(null); setError(''); setPending(false);
    if (!enabled || !skillId) { context.current = null; setLoading(false); return; }
    const target: Context = { skillId, paneId, alive: true, version: -1, ready: Promise.resolve(), tail: Promise.resolve(), operations: 0 };
    context.current = target; setLoading(true);
    let unlisten: (() => void) | undefined;
    target.ready = (async () => {
      const stop = await listen<SelectionEvent>('workspace-export-selection-changed', event => {
        if (event.payload.skillId === target.skillId) accept(target, event.payload.draft, event.payload.version);
      });
      if (!target.alive) { stop(); return; }
      unlisten = stop;
      await load(target);
    })().catch(cause => {
      target.failure = cause;
      if (target.alive && context.current === target) setError(message(cause));
    }).finally(() => {
      if (target.alive && context.current === target) setLoading(false);
    });
    return () => {
      target.alive = false; unlisten?.();
      if (context.current === target) context.current = null;
      void Promise.allSettled([target.ready, target.tail]).then(async () => {
        if (target.draft && target.leaseId) await request('workspace_release_export_selection', {
          skillId: target.skillId, paneId: target.paneId, draftId: target.draft.id, leaseId: target.leaseId,
        });
      }).catch(() => undefined);
    };
  }, [skillId, paneId, enabled, accept, load]);
  const enqueue = useCallback(<T,>(run: (target: Context) => Promise<T>): Promise<T> => {
    const target = context.current;
    if (!target?.alive) return Promise.reject(new Error('当前页面没有可用的导出选择。'));
    target.operations += 1; setPending(true);
    const work = target.tail.then(async () => {
      await target.ready;
      if (!target.alive || context.current !== target) throw new Error('页面已切换，本次选择未继续修改。');
      if (!target.draft || !target.leaseId) await load(target);
      const value = await run(target);
      target.failure = undefined;
      if (target.alive && context.current === target) setError('');
      return value;
    }).catch(cause => {
      target.failure = cause;
      if (target.alive && context.current === target) setError(message(cause));
      throw cause;
    }).finally(() => {
      target.operations -= 1;
      if (target.alive && context.current === target) setPending(target.operations > 0);
    });
    // Continue accepting later deliberate interactions after a failed one, while
    // flush still reports an unresolved failure to the caller before export.
    target.tail = work.then(() => undefined, () => undefined);
    return work;
  }, [load]);
  const update = useCallback((args: Record<string, unknown>) => enqueue(async target => {
    const reply = await request<SelectionReply>('workspace_update_export_selection', {
      skillId: target.skillId, paneId: target.paneId, draftId: target.draft!.id, leaseId: target.leaseId, ...args,
    });
    if (reply.draft) accept(target, reply.draft, reply.draft.version);
    return reply;
  }), [accept, enqueue]);
  const toggle = useCallback((resourceId: string, included: boolean) => update({ operation: 'toggle', resourceId, included }).then(() => undefined), [update]);
  const set = useCallback((selectedIds: string[]) => update({ operation: 'set', selectedIds }).then(() => undefined), [update]);
  const reset = useCallback(() => update({ operation: 'clear' }).then(() => undefined), [update]);
  const clearIfCurrent = useCallback((snapshot: ExportSelectionDraft) => update({ operation: 'clear', draftId: snapshot.id, expectedRevision: snapshot.revision }).then(reply => reply.cleared === true), [update]);
  const refresh = useCallback(() => enqueue(async target => { await load(target); return copy(target.draft!); }), [enqueue, load]);
  const flush = useCallback(async () => {
    const target = context.current;
    if (!target?.alive) throw new Error('当前页面没有可用的导出选择。');
    await target.ready;
    // A callback may enqueue another click while the first acknowledgement is
    // returning; drain through the latest tail instead of one old promise.
    for (;;) { const tail = target.tail; await tail; if (tail === target.tail) break; }
    if (!target.alive || context.current !== target) throw new Error('页面已切换，请返回当前导出页。');
    if (target.failure) throw target.failure;
    if (!target.draft || !target.leaseId) await load(target);
    return copy(target.draft!);
  }, [load]);
  return { draft, selectedIds: draft?.selectedIds || [], loading, pending, error, active: enabled && !!skillId, toggle, set, reset, clearIfCurrent, refresh, flush };
}
