import { useCallback, useEffect, useRef, useState } from 'react';
import { BoundedReadCache, classifyReadFailure, readRequestKey, type ReadFailure } from './read-cache';

const documents = new BoundedReadCache<unknown>();
type Result<T> = { key: string; cacheKey: string; value: T | null; complete: boolean; error: string; fromCache: boolean };
export type DocumentCacheOptions<T> = {
  cacheKey: string; enabled: boolean; load: () => Promise<T>; initialValue?: T;
  cacheable?: boolean; staleAfterMs?: number; classifyError?: (cause: unknown) => ReadFailure;
};

/** Show an identity-matched document immediately, then re-read without blanking it. */
export function useDocumentCache<T>({ cacheKey, enabled, load, initialValue, cacheable = true, staleAfterMs = 30_000, classifyError = classifyReadFailure }: DocumentCacheOptions<T>) {
  const loadRef = useRef(load); loadRef.current = load;
  const classifyRef = useRef(classifyError); classifyRef.current = classifyError;
  const initial = useRef({ key: cacheKey, value: initialValue });
  const [refreshIndex, setRefreshIndex] = useState(0);
  const [result, setResult] = useState<Result<T>>({ key: '', cacheKey: '', value: null, complete: false, error: '', fromCache: false });
  const generation = useRef(0);
  const lastRefresh = useRef(refreshIndex);
  const cached = cacheable ? documents.peek(cacheKey) as T | undefined : undefined;
  const seeded = cacheable && initial.current.key === cacheKey ? initial.current.value : undefined;
  const requestKey = readRequestKey(cacheKey, refreshIndex);
  const active = result.key === requestKey ? result : undefined;
  const retained = cacheable && result.cacheKey === cacheKey ? result.value : null;
  const value = active?.value ?? cached ?? retained ?? seeded ?? null;
  useEffect(() => {
    const sequence = ++generation.current;
    if (!enabled) return;
    const previous = cacheable ? documents.get(cacheKey) as T | undefined : undefined;
    const seed = cacheable && initial.current.key === cacheKey ? initial.current.value : undefined;
    const manual = lastRefresh.current !== refreshIndex; lastRefresh.current = refreshIndex;
    if (!manual && previous !== undefined && documents.fresh(cacheKey, staleAfterMs)) {
      setResult({ key: requestKey, cacheKey, value: previous, complete: true, error: '', fromCache: true });
      return;
    }
    setResult(current => {
      const retained = previous ?? (cacheable && current.cacheKey === cacheKey ? current.value : null) ?? seed ?? null;
      return { key: requestKey, cacheKey, value: retained, complete: false, error: '', fromCache: retained !== null };
    });
    const read = loadRef.current;
    const request = cacheable ? documents.load(cacheKey, read, manual) as Promise<T> : Promise.resolve().then(read);
    void request.then(next => {
      if (generation.current === sequence) setResult({ key: requestKey, cacheKey, value: next, complete: true, error: '', fromCache: false });
    }).catch(cause => {
      if (generation.current !== sequence) return;
      const keep = classifyRef.current(cause) === 'network';
      if (!keep) { documents.delete(cacheKey); if (initial.current.key === cacheKey) initial.current.value = undefined; }
      setResult(current => ({ key: requestKey, cacheKey, value: keep && current.cacheKey === cacheKey ? current.value : null, complete: true, error: cause instanceof Error ? cause.message : String(cause), fromCache: keep && current.cacheKey === cacheKey && current.value !== null }));
    });
    return () => { if (generation.current === sequence) generation.current++; };
  }, [requestKey, cacheKey, enabled, cacheable, staleAfterMs, refreshIndex]);
  const refresh = useCallback(() => setRefreshIndex(index => index + 1), []);
  const pending = enabled && (!active || !active.complete);
  return { value, loading: pending && value === null, refreshing: pending && value !== null, error: active?.error || '', fromCache: active?.fromCache ?? cached !== undefined, refresh };
}
