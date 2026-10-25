import { useCallback, useEffect, useRef } from 'react';

export type LayoutPreferences = { uiZoom?: number; sidebarWidth?: number; sidebarCollapsed?: boolean };

// Display interactions stay responsive while a network operation holds the manager.
// Merge repeated changes, then send only the latest values once that operation ends.
export function usePreferenceQueue<T>({ save, isBusy, onSaved, onFailed }: {
  save: (patch: LayoutPreferences) => Promise<T>;
  isBusy: () => boolean;
  onSaved: (patch: LayoutPreferences, result: T) => void;
  onFailed: (patch: LayoutPreferences, error: unknown) => void;
}) {
  const callbacks = useRef({ save, isBusy, onSaved, onFailed });
  callbacks.current = { save, isBusy, onSaved, onFailed };
  const pending = useRef<LayoutPreferences>({});
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const saving = useRef<Promise<boolean> | null>(null);
  const mounted = useRef(true);
  const drain = useRef<(wait: boolean) => Promise<boolean>>(async () => true);
  drain.current = async wait => {
    clearTimeout(timer.current);
    let success = true;
    while (mounted.current) {
      if (saving.current) {
        success = await saving.current && success;
        if (!wait) return success;
        continue;
      }
      if (!Object.keys(pending.current).length) return success;
      if (callbacks.current.isBusy()) {
        if (!wait) { timer.current = setTimeout(() => void drain.current(false), 200); return success; }
        await new Promise(resolve => setTimeout(resolve, 100));
        continue;
      }
      const patch = pending.current;
      pending.current = {};
      saving.current = (async () => {
        try {
          const result = await callbacks.current.save(patch);
          if (mounted.current) callbacks.current.onSaved(patch, result);
          return true;
        } catch (error) {
          // A failed older request must never roll back a more recent interaction.
          const untouched = Object.fromEntries(Object.entries(patch).filter(([key]) => !(key in pending.current))) as LayoutPreferences;
          if (mounted.current) callbacks.current.onFailed(untouched, error);
          return false;
        } finally {
          saving.current = null;
          if (mounted.current && Object.keys(pending.current).length) timer.current = setTimeout(() => void drain.current(false), 400);
        }
      })();
      success = await saving.current && success;
      if (!wait) return success;
    }
    return false;
  };
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; clearTimeout(timer.current); };
  }, []);
  const queue = useCallback((patch: LayoutPreferences) => {
    pending.current = { ...pending.current, ...patch };
    clearTimeout(timer.current);
    timer.current = setTimeout(() => void drain.current(false), 400);
  }, []);
  const flush = useCallback(() => drain.current(true), []);
  return { queue, flush };
}
