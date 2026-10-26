import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { exportPlanReadKeys, exportPlanView, type ExportPlan, type ExportPlanRead } from './export-selection';
import { BoundedReadCache, classifyReadFailure } from './read-cache';
import { bridgeMessage, retryBridgeRead } from './bridge-recovery';

const plans = new BoundedReadCache<ExportPlan>(32, 8 * 1024 * 1024, 2 * 1024 * 1024);
function load(key: string, args: Record<string, unknown>, cacheable: boolean, force: boolean, owner: () => boolean) {
  const read = (active: () => boolean) => retryBridgeRead(() => invoke<ExportPlan>('manager_request', { request: { action: 'get_export_plan', ...args } }), { active });
  return cacheable ? plans.load(key, read, force, owner) : read(owner);
}

/** Cached manifests are display data; the existing export action revalidates every source. */
export function useExportPlan({ skillId, selectedIds, enabled, contentKey, remoteCommit, cacheable = true }: { skillId?: string; selectedIds: string[]; enabled: boolean; contentKey: string; remoteCommit?: string; cacheable?: boolean }) {
  const [result, setResult] = useState<ExportPlanRead>({ key: '', contextKey: '', plan: null, error: '', complete: false, fromCache: false });
  const [refreshIndex, setRefreshIndex] = useState(0);
  const generation = useRef(0), lastRefresh = useRef(refreshIndex);
  const selection = JSON.stringify([...selectedIds].sort());
  const contextKey = JSON.stringify([skillId, contentKey, remoteCommit, cacheable]);
  const { cacheKey, requestKey: key } = exportPlanReadKeys(contextKey, selection, refreshIndex);
  const current = result.key === key ? result : undefined;
  const cached = cacheable ? plans.peek(cacheKey) : undefined;
  const view = exportPlanView(result, key, contextKey, cached, cacheable);
  const plan = view.plan;
  useEffect(() => {
    const sequence = ++generation.current;
    if (!enabled || !skillId) return;
    const previous = cacheable ? plans.get(cacheKey) : undefined;
    const manual = lastRefresh.current !== refreshIndex; lastRefresh.current = refreshIndex;
    if (!manual && previous && plans.fresh(cacheKey, 30_000)) {
      setResult({ key, contextKey, plan: previous, error: '', complete: true, fromCache: true });
      return;
    }
    setResult(state => {
      const retained = exportPlanView(state, key, contextKey, previous, cacheable).plan;
      return { key, contextKey, plan: retained, error: '', complete: false, fromCache: !!retained };
    });
    void load(cacheKey, { skillId, selectedIds: JSON.parse(selection), ...(remoteCommit ? { remoteCommit } : {}) }, cacheable, manual, () => generation.current === sequence).then(value => {
      if (generation.current === sequence) setResult({ key, contextKey, plan: value, error: '', complete: true, fromCache: false });
    }).catch(cause => {
      if (generation.current !== sequence) return;
      const keep = ['network', 'bridge'].includes(classifyReadFailure(cause));
      if (!keep) plans.delete(cacheKey);
      setResult(state => ({ key, contextKey, plan: keep && state.contextKey === contextKey ? state.plan : null, error: bridgeMessage(cause), complete: true, fromCache: keep }));
    });
    return () => { if (generation.current === sequence) generation.current++; };
  }, [key, cacheKey, contextKey, skillId, enabled, selection, remoteCommit, refreshIndex, cacheable]);
  const refresh = useCallback(() => setRefreshIndex(value => value + 1), []);
  const pending = enabled && (!current || !current.complete);
  return { plan: plan?.skillId === skillId ? plan : null, error: current?.error || '', loading: pending && !plan, refreshing: pending && !!plan, fromCache: current?.fromCache ?? !!cached, current: enabled && view.current && plan?.skillId === skillId, refresh };
}
