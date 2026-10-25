import { isBridgeFailure } from './bridge-recovery.ts';

export type ReadFailure = 'auth' | 'source' | 'network' | 'bridge' | 'other';

export function readRequestKey(cacheKey: string, refreshRevision: number) {
  return JSON.stringify([cacheKey, refreshRevision]);
}

/** The first field is also the target-invalidation identity used by file refresh. */
export function inventoryReadKey(skillId: string, generation: number | null | undefined, contentStamp: string) {
  return JSON.stringify([skillId, generation ?? null, contentStamp]);
}

/** Bounded display data only. This cache never supplies mutation or export proofs. */
export class BoundedReadCache<T> {
  private entries = new Map<string, { value: T; bytes: number; checkedAt: number }>();
  private pending = new Map<string, { promise: Promise<T>; owners: Set<() => boolean> }>();
  private bytes = 0;
  private maxEntries: number;
  private maxBytes: number;
  private maxEntryBytes: number;
  private clock: () => number;
  constructor(maxEntries = 32, maxBytes = 8 * 1024 * 1024, maxEntryBytes = 1024 * 1024, clock: () => number = () => performance.now()) {
    this.maxEntries = maxEntries; this.maxBytes = maxBytes; this.maxEntryBytes = maxEntryBytes; this.clock = clock;
  }
  peek(key: string): T | undefined { return this.entries.get(key)?.value; }
  keys(): IterableIterator<string> { return this.entries.keys(); }
  get(key: string): T | undefined {
    const entry = this.entries.get(key);
    if (entry) { this.entries.delete(key); this.entries.set(key, entry); }
    return entry?.value;
  }
  delete(key: string) {
    const entry = this.entries.get(key);
    if (entry) this.bytes -= entry.bytes;
    this.entries.delete(key);
  }
  clear() { this.entries.clear(); this.bytes = 0; }
  fresh(key: string, staleAfterMs: number): boolean {
    const entry = this.entries.get(key);
    return !!entry && this.clock() - entry.checkedAt < staleAfterMs;
  }
  set(key: string, value: T): boolean {
    this.delete(key);
    let bytes: number;
    try { bytes = new TextEncoder().encode(JSON.stringify(value)).byteLength; }
    catch { return false; }
    if (bytes > this.maxEntryBytes || bytes > this.maxBytes) return false;
    this.entries.set(key, { value, bytes, checkedAt: this.clock() }); this.bytes += bytes;
    while (this.entries.size > this.maxEntries || this.bytes > this.maxBytes) this.delete(this.entries.keys().next().value!);
    return true;
  }
  load(key: string, read: (active: () => boolean) => Promise<T>, force = false, owner: () => boolean = () => true): Promise<T> {
    const current = this.pending.get(key);
    if (current && !force) { current.owners.add(owner); return current.promise; }
    const owners = new Set([owner]);
    const active = () => [...owners].some(owns => owns());
    const request = Promise.resolve().then(() => read(active)).then(value => {
      if (this.pending.get(key)?.promise === request) this.set(key, value);
      return value;
    });
    this.pending.set(key, { promise: request, owners });
    // Waiters keep their promise; only the shared coalescing index is bounded.
    while (this.pending.size > this.maxEntries) this.pending.delete(this.pending.keys().next().value!);
    void request.finally(() => { if (this.pending.get(key)?.promise === request) this.pending.delete(key); }).catch(() => undefined);
    return request;
  }
  get statistics() { return { entries: this.entries.size, bytes: this.bytes, pending: this.pending.size }; }
}

export type DocumentCacheIdentity = {
  readGeneration?: number | null; skillId: string; sourceId?: string | null;
  repo?: string | null; branch?: string | null; remotePath?: string | null;
  commit?: string | null; version?: string; path?: string; origin?: string;
  localPath?: string | null; localFingerprint?: unknown;
};
/** A credential generation is an opaque number, never a credential or its hash. */
export function documentCacheKey(identity: DocumentCacheIdentity): string {
  return JSON.stringify([identity.readGeneration ?? null, identity.skillId, identity.sourceId ?? null,
    identity.repo ?? null, identity.branch ?? null, identity.remotePath ?? null, identity.commit ?? null,
    identity.version ?? 'latest', identity.path ?? null, identity.origin ?? null,
    identity.localPath ?? null, identity.localFingerprint ?? null]);
}

export function classifyReadFailure(cause: unknown): ReadFailure {
  if (isBridgeFailure(cause)) return 'bridge';
  const text = cause instanceof Error ? cause.message : String(cause);
  if (/\b(?:401|403)\b|authentication|unauthori[sz]ed|forbidden|凭据|身份验证|权限不足|无权访问/i.test(text)) return 'auth';
  if (/\b404\b|not found|no longer exists|来源已变化|不存在|已移除|不是.*文档|not UTF-8|invalid.*(?:path|source|document)/i.test(text)) return 'source';
  if (/timeout|timed out|network|connect|offline|rate limit|\b429\b|网络|连接|超时|离线|请求额度/i.test(text)) return 'network';
  return 'other';
}
