export const BRIDGE_RETRY_DELAYS = [250, 1000] as const;

export function isBridgeFailure(cause: unknown): boolean {
  const text = cause instanceof Error ? cause.message : String(cause);
  return /failed to (?:send|receive) message (?:to|from) (?:the )?webview|webview (?:is |was )?(?:closed|unavailable)|ipc (?:channel |bridge )?(?:closed|unavailable|disconnected)/i.test(text);
}

export function bridgeMessage(cause: unknown): string {
  return isBridgeFailure(cause) ? '界面通信暂时中断，请重新核对当前状态。' : cause instanceof Error ? cause.message : String(cause);
}

type ReadOptions = { active?: () => boolean; wait?: (milliseconds: number) => Promise<void> };
const wait = (milliseconds: number) => new Promise<void>(resolve => setTimeout(resolve, milliseconds));

/** This helper is for subscriptions and reads only. Never wrap a file or selection write. */
export async function retryBridgeRead<T>(read: () => Promise<T>, options: ReadOptions = {}): Promise<T> {
  const active = options.active || (() => true);
  for (let attempt = 0; ; attempt++) {
    if (!active()) throw new Error('页面已离开，本次读取已停止。');
    try { return await read(); }
    catch (cause) {
      if (!isBridgeFailure(cause) || attempt >= BRIDGE_RETRY_DELAYS.length || !active()) throw cause;
      await (options.wait || wait)(BRIDGE_RETRY_DELAYS[attempt]);
    }
  }
}

function release(stop: () => void) {
  try { void Promise.resolve(stop()).catch(() => undefined); } catch { /* The old WebView may already be gone. */ }
}

/** One bounded registration attempt, restarted only by focus or an explicit recovery. */
export class RecoverableSubscription {
  private disposed = false;
  private stop?: () => void;
  private pending?: Promise<void>;
  private register: () => Promise<() => void>;
  private options: ReadOptions & { onReady?: () => void; onUnavailable?: (cause: unknown) => void };
  constructor(register: () => Promise<() => void>, options: ReadOptions & { onReady?: () => void; onUnavailable?: (cause: unknown) => void } = {}) { this.register = register; this.options = options; }
  get connected() { return !!this.stop; }
  recover(): Promise<void> {
    if (this.disposed || this.stop) return Promise.resolve();
    if (this.pending) return this.pending;
    const active = () => !this.disposed && (this.options.active?.() ?? true);
    this.pending = retryBridgeRead(this.register, { ...this.options, active }).then(stop => {
      if (!active()) { release(stop); return; }
      this.stop = stop;
      this.options.onReady?.();
    }).catch(cause => { if (active()) this.options.onUnavailable?.(cause); }).finally(() => { this.pending = undefined; });
    return this.pending;
  }
  dispose() { this.disposed = true; if (this.stop) release(this.stop); this.stop = undefined; }
}

export function shouldPeekSelection(state: { alive: boolean; visible: boolean; owned: boolean; ipcAvailable: boolean; connected: boolean }) {
  return state.alive && state.visible && state.owned && state.ipcAvailable && !state.connected;
}

export type CloseRound = { closeId: string; round?: string };
export type CloseControlSnapshot = { closeRequest?: CloseRound | null };
export function pendingCloseRound(expected: CloseRound, snapshot: CloseControlSnapshot): boolean {
  return snapshot.closeRequest?.closeId === expected.closeId && (snapshot.closeRequest.round || 'initial') === (expected.round || 'initial');
}

/** Control ACKs are idempotent. Re-send once only after a read proves this exact round is pending. */
export async function acknowledgeCloseRound(round: CloseRound, send: () => Promise<unknown>, peek: () => Promise<CloseControlSnapshot>, active: () => boolean): Promise<void> {
  if (!active()) return;
  try { await send(); }
  catch (cause) {
    if (!active()) throw cause;
    const snapshot = await retryBridgeRead(peek, { active });
    if (!pendingCloseRound(round, snapshot) || !active()) return;
    await send();
  }
}

export function shouldPollWorkspace(state: { alive: boolean; visible: boolean; ipcAvailable: boolean; degraded: boolean; closeDegraded: boolean }): boolean {
  return state.alive && state.ipcAvailable && (state.visible ? state.degraded : state.closeDegraded);
}

/** A lost acknowledgement is reconciled from state, never by replaying its write. */
export function selectionWriteConfirmed(before: { id: string; revision: number; selectedIds: string[] }, after: typeof before, operation: Record<string, unknown>): boolean {
  if (before.id !== after.id || operation.draftId && operation.draftId !== before.id || after.revision < before.revision) return false;
  if (operation.operation === 'toggle') return after.selectedIds.includes(String(operation.resourceId)) === operation.included;
  if (operation.operation === 'set') return JSON.stringify([...after.selectedIds].sort()) === JSON.stringify([...new Set(operation.selectedIds as string[])].sort());
  if (operation.operation === 'clear') {
    if (typeof operation.expectedRevision === 'number' && (operation.expectedRevision !== before.revision || after.revision > before.revision + 1)) return false;
    return !after.selectedIds.length;
  }
  return false;
}
