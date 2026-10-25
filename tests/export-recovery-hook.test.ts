import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createElement, act } from 'react';
import { JSDOM } from 'jsdom';
import ts from 'typescript';

type Draft = { id: string; skillId: string; revision: number; selectedIds: string[]; version: number };
type Selection = ReturnType<typeof import('../src/useExportSelection.ts').useExportSelection>;
type PlanState = ReturnType<typeof import('../src/useExportPlan.ts').useExportPlan>;
type Result = { selection: Selection; plan: PlanState };
const bridgeFailure = () => new Error('runtime error: failed to send message to the webview');
const clone = <T,>(value: T): T => structuredClone(value);
const delay = (milliseconds: number) => new Promise<void>(resolve => setTimeout(resolve, milliseconds));

// Compile the actual hooks and their local dependencies. No implementation is copied into this test.
const moduleURLs = new Map<string, Promise<string>>();
async function actualModuleURL(url: URL): Promise<string> {
  const existing = moduleURLs.get(url.href); if (existing) return existing;
  const loading = (async () => {
    const source = await readFile(url, 'utf8');
    let output = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } }).outputText;
    const imports = [...output.matchAll(/from\s+(['"])([^'"]+)\1/g)];
    const replacements = new Map<string, string>();
    for (const match of imports) {
      const specifier = match[2];
      if (replacements.has(specifier)) continue;
      replacements.set(specifier, specifier.startsWith('.')
        ? await actualModuleURL(new URL(/\.[cm]?[jt]sx?$/.test(specifier) ? specifier : `${specifier}.ts`, url))
        : import.meta.resolve(specifier));
    }
    output = output.replace(/from\s+(['"])([^'"]+)\1/g, (_match, _quote, specifier: string) => `from ${JSON.stringify(replacements.get(specifier))}`);
    return `data:text/javascript;base64,${Buffer.from(output).toString('base64')}`;
  })();
  moduleURLs.set(url.href, loading); return loading;
}

class MemoryBridge {
  draft: Draft;
  calls = new Map<string, number>();
  leases = new Map<string, string>();
  leaseRegistrations = 0;
  writes = 0;
  releases = 0;
  failedGets = 0;
  failedPlans = 0;
  loseWriteReply = false;
  listenMode: 'success' | 'fail' | 'deferred' = 'success';
  callbacks = new Map<number, (event: unknown) => void>();
  listeners = new Map<number, { event: string; callback: number }>();
  nextCallback = 0;
  nextListener = 0;
  pendingListen: (() => void)[] = [];
  pendingGet?: { promise: Promise<unknown>; resolve: (value: unknown) => void };
  constructor(skillId: string) { this.draft = { id: `draft-${skillId}`, skillId, revision: 0, selectedIds: [], version: 0 }; }
  count(action: string) { return this.calls.get(action) || 0; }
  install(window: Window) {
    Object.assign(window, {
      __TAURI_INTERNALS__: {
        transformCallback: (callback: (event: unknown) => void) => { const id = ++this.nextCallback; this.callbacks.set(id, callback); return id; },
        unregisterCallback: (id: number) => this.callbacks.delete(id),
        invoke: (command: string, args: Record<string, any>) => this.invoke(command, args),
      },
      __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: (_event: string, id: number) => this.listeners.delete(id) },
    });
  }
  selectionReply(paneId: string) {
    const leaseId = `lease-${++this.leaseRegistrations}`; this.leases.set(paneId, leaseId);
    return { draft: clone(this.draft), leaseId };
  }
  deferInitialGet() {
    let resolve!: (value: unknown) => void;
    const promise = new Promise<unknown>(done => { resolve = done; });
    this.pendingGet = { promise, resolve };
  }
  resolveInitialGet(paneId: string) {
    const pending = this.pendingGet; assert(pending); this.pendingGet = undefined;
    pending.resolve(this.selectionReply(paneId));
  }
  externalSelection(ids: string[], version = this.draft.version + 1) {
    this.draft = { ...this.draft, selectedIds: [...ids], revision: this.draft.revision + 1, version };
  }
  emit(draft = this.draft, version = draft.version) {
    const payload = { skillId: this.draft.skillId, version, draft: clone(draft) };
    for (const [id, listener] of this.listeners) this.callbacks.get(listener.callback)?.({ event: listener.event, id, payload });
  }
  completeListen() { for (const ready of this.pendingListen.splice(0)) ready(); }
  async invoke(command: string, args: Record<string, any>) {
    if (command === 'plugin:event|listen') {
      this.calls.set(command, this.count(command) + 1);
      if (this.listenMode === 'fail') throw bridgeFailure();
      const register = () => { const id = ++this.nextListener; this.listeners.set(id, { event: args.event, callback: args.handler }); return id; };
      if (this.listenMode === 'deferred') return new Promise<number>(resolve => this.pendingListen.push(() => resolve(register())));
      return register();
    }
    if (command === 'plugin:event|unlisten') { this.listeners.delete(args.eventId); return; }
    assert.equal(command, 'manager_request');
    const request = args.request;
    const action = request.action as string;
    this.calls.set(action, this.count(action) + 1);
    assert.equal(request.skillId, this.draft.skillId);
    if (action === 'workspace_get_export_selection') {
      if (this.failedGets > 0) { this.failedGets--; throw new Error('Synthetic initial selection read failed'); }
      if (this.pendingGet) return this.pendingGet.promise;
      return this.selectionReply(request.paneId);
    }
    if (action === 'workspace_read_export_selection') return {
      draft: clone(this.draft), version: this.draft.version,
      leaseValid: request.draftId === this.draft.id && this.leases.get(request.paneId) === request.leaseId,
    };
    if (action === 'workspace_release_export_selection') {
      if (this.leases.get(request.paneId) === request.leaseId) { this.leases.delete(request.paneId); this.releases++; }
      return { released: true };
    }
    if (action === 'workspace_update_export_selection') {
      assert.equal(request.draftId, this.draft.id); assert.equal(request.leaseId, this.leases.get(request.paneId));
      this.writes++;
      const selected = new Set(this.draft.selectedIds);
      if (request.operation === 'toggle') { if (request.included) selected.add(request.resourceId); else selected.delete(request.resourceId); }
      else if (request.operation === 'set') { selected.clear(); for (const id of request.selectedIds) selected.add(id); }
      else if (request.operation === 'clear') {
        if (request.expectedRevision !== undefined && request.expectedRevision !== this.draft.revision) return { draft: clone(this.draft), cleared: false };
        selected.clear();
      } else assert.fail(`Unexpected selection operation ${request.operation}`);
      this.externalSelection([...selected].sort());
      if (this.loseWriteReply) { this.loseWriteReply = false; throw bridgeFailure(); }
      this.emit(); return { draft: clone(this.draft), cleared: request.operation === 'clear' };
    }
    if (action === 'get_export_plan') {
      if (this.failedPlans > 0) { this.failedPlans--; throw bridgeFailure(); }
      const ids = new Set<string>(request.selectedIds);
      const resources = [
        { id: 'file:SKILL.md', label: 'SKILL.md', path: 'SKILL.md', group: 'required', included: true, selectable: false, source: 'local', size: 16, reason: 'entry', referenceIds: [] },
        { id: 'file:README.md', label: 'README.md', path: 'README.md', group: 'optional_package', included: ids.has('file:README.md'), selectable: true, source: 'github', size: 24, reason: 'optional documentation', referenceIds: [] },
        { id: 'external:rules', label: 'rules.md', path: 'shared/rules.md', group: 'optional_external', included: ids.has('external:rules'), selectable: true, source: 'local', size: 12, reason: 'optional rules', referenceIds: [] },
      ];
      const included = resources.filter(resource => resource.included);
      return { id: `plan-${this.draft.skillId}-${[...ids].sort().join(',')}`, skillId: this.draft.skillId, resources,
        manifest: included.map(resource => ({ path: resource.path, resourceId: resource.id, source: resource.source, size: resource.size })),
        summary: { required: 1, optionalPackageSelected: Number(ids.has('file:README.md')), optionalPackageTotal: 1, optionalExternalSelected: Number(ids.has('external:rules')), optionalExternalTotal: 1, generated: 0, total: included.length, remoteCount: included.filter(r => r.source === 'github').length, bytes: included.reduce((n, r) => n + r.size, 0) }, canExport: true, issues: [] };
    }
    assert.fail(`Unexpected manager action ${action}`);
  }
}

async function mount(bridge: MemoryBridge) {
  const dom = new JSDOM('<!doctype html><div id="app"></div>', { url: 'http://tauri.localhost', pretendToBeVisual: true });
  const saved = new Map<string, PropertyDescriptor | undefined>();
  for (const [key, value] of Object.entries({ window: dom.window, document: dom.window.document, HTMLElement: dom.window.HTMLElement, Node: dom.window.Node, Event: dom.window.Event, IS_REACT_ACT_ENVIRONMENT: true })) {
    saved.set(key, Object.getOwnPropertyDescriptor(globalThis, key)); Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  }
  bridge.install(dom.window as unknown as Window);
  const { createRoot } = await import('react-dom/client');
  const { useExportSelection } = await import(await actualModuleURL(new URL('../src/useExportSelection.ts', import.meta.url)));
  const { useExportPlan } = await import(await actualModuleURL(new URL('../src/useExportPlan.ts', import.meta.url)));
  const root = createRoot(dom.window.document.getElementById('app')!);
  let current!: Result, renderCount = 0, mounted = true;
  const paneId = `pane-${bridge.draft.skillId}`;
  function Probe({ visible }: { visible: boolean }) {
    const selection = useExportSelection({ skillId: bridge.draft.skillId, paneId, enabled: true, visible });
    const plan = useExportPlan({ skillId: bridge.draft.skillId, selectedIds: selection.selectedIds, enabled: !!selection.draft, contentKey: bridge.draft.skillId, cacheable: true });
    current = { selection, plan }; renderCount++;
    return createElement('output', null, JSON.stringify({ selected: selection.selectedIds, selectable: plan.plan?.resources.filter((resource: any) => resource.selectable).length, ready: plan.current }));
  }
  await act(async () => { root.render(createElement(Probe, { visible: true })); });
  return {
    get current() { return current; }, get renders() { return renderCount; }, paneId,
    async visible(value: boolean) { await act(async () => { root.render(createElement(Probe, { visible: value })); }); },
    async unmount() { if (mounted) { mounted = false; await act(async () => { root.unmount(); await delay(10); }); } },
    async dispose() {
      if (mounted) { mounted = false; await act(async () => { root.unmount(); await delay(10); }); }
      dom.window.close();
      for (const [key, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); }
    },
  };
}

async function until(check: () => unknown, label: string, milliseconds = 3000) {
  const end = performance.now() + milliseconds;
  while (!check() && performance.now() < end) await act(async () => { await delay(10); });
  assert(check(), label);
}

test('real hooks load draft and selectable resources even when event registration continually fails', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('listen-failure'); backend.listenMode = 'fail';
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current, 'A failed listener must not gate the authoritative first draft/plan');
    assert.equal(app.current.selection.error, '');
    assert.equal(app.current.plan.plan!.resources.filter(resource => resource.selectable).length, 2);
    await act(async () => { await app.current.selection.toggle('file:README.md', true); });
    await until(() => app.current.plan.current && app.current.plan.plan!.summary.optionalPackageSelected === 1, 'A GitHub-only optional resource can be selected');
    assert.deepEqual(app.current.selection.selectedIds, ['file:README.md']); assert.equal(backend.writes, 1);
  } finally { await app.dispose(); }
});

test('real selection and plan hooks recover together after an initial draft read failure', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('selection-recovery'); backend.listenMode = 'fail'; backend.failedGets = 1;
  const app = await mount(backend);
  try {
    await until(() => !!app.current.selection.error && !app.current.selection.loading, 'The initial selection read failure is visible');
    assert.equal(app.current.selection.draft, null); assert.equal(backend.count('get_export_plan'), 0);
    await act(async () => { await app.current.selection.refresh(); app.current.plan.refresh(); });
    await until(() => app.current.plan.current, 'Explicit recovery loads a draft and a fresh plan');
    assert.equal(app.current.selection.error, ''); assert.equal(backend.leaseRegistrations, 1);
    await act(async () => { await app.current.selection.toggle('external:rules', true); });
    await until(() => app.current.plan.current && app.current.plan.plan!.summary.optionalExternalSelected === 1, 'Recovered selection remains writable');
  } finally { await app.dispose(); }
});

test('a successful late subscription reconciles changes missed before listener registration', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('late-subscription'); backend.listenMode = 'deferred';
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current, 'The initial snapshot loads before the listener is ready');
    backend.externalSelection(['file:README.md']);
    await act(async () => { backend.completeListen(); });
    await until(() => app.current.selection.selectedIds.includes('file:README.md'), 'Subscription readiness must reconcile the missed snapshot');
    assert.equal(backend.leaseRegistrations, 1, 'Read-only reconciliation does not replace the pane lease');
    assert(backend.count('workspace_read_export_selection') >= 1);
  } finally { await app.dispose(); }
});

test('degraded event synchronization peeks without renewing leases and stops for hidden panes', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('fallback-peek'); backend.listenMode = 'fail';
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current, 'Initial content is ready');
    const registrations = backend.leaseRegistrations;
    backend.externalSelection(['external:rules']);
    await until(() => app.current.selection.selectedIds.includes('external:rules'), 'A visible degraded pane reads changes without events', 3500);
    assert.equal(backend.leaseRegistrations, registrations);
    await app.visible(false); const reads = backend.count('workspace_read_export_selection');
    await act(async () => { await delay(2100); });
    assert.equal(backend.count('workspace_read_export_selection'), reads, 'A hidden pane performs no fallback polling');
  } finally { await app.dispose(); }
});

test('out-of-order and post-unmount events cannot replace the newer real hook state', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('event-order');
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current && backend.listeners.size === 1, 'Draft and event listener are ready');
    backend.externalSelection(['file:README.md'], 5);
    await act(async () => { backend.emit(); });
    await until(() => app.current.selection.draft?.version === 5, 'The latest event is accepted');
    const callback = backend.callbacks.get([...backend.listeners.values()][0].callback)!;
    await act(async () => { backend.emit({ ...backend.draft, selectedIds: [], version: 4 }, 4); });
    assert.deepEqual(app.current.selection.selectedIds, ['file:README.md']);
    await app.unmount(); const renders = app.renders;
    await act(async () => { callback({ payload: { skillId: backend.draft.skillId, version: 6, draft: { ...backend.draft, selectedIds: [], version: 6 } } }); });
    assert.equal(app.renders, renders, 'Late delivery to an old callback does not update an unmounted pane');
  } finally { await app.dispose(); }
});

test('a lost write acknowledgement is confirmed by reading state and never duplicates the write', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('lost-write-reply');
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current, 'Initial content is ready');
    const registrations = backend.leaseRegistrations, reads = backend.count('workspace_read_export_selection');
    backend.loseWriteReply = true;
    await act(async () => { await app.current.selection.toggle('file:README.md', true); });
    await until(() => app.current.plan.current && app.current.selection.selectedIds.includes('file:README.md'), 'The acknowledged state is visible');
    assert.equal(backend.writes, 1); assert.equal(backend.leaseRegistrations, registrations);
    assert(backend.count('workspace_read_export_selection') >= reads + 2, 'Read before write and readback after acknowledgement loss both ran');
    assert.equal(app.current.selection.error, '');
  } finally { await app.dispose(); }
});

test('failed plan revalidation retains choices for display but cannot authorize export until refreshed', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('plan-recovery');
  const app = await mount(backend);
  try {
    await until(() => app.current.plan.current, 'Initial plan is ready');
    await act(async () => { await app.current.selection.toggle('file:README.md', true); });
    await until(() => app.current.plan.current && app.current.plan.plan!.summary.optionalPackageSelected === 1, 'Optional selection is ready');
    backend.failedPlans = 3;
    await act(async () => { app.current.plan.refresh(); });
    await until(() => !!app.current.plan.error, 'Bounded revalidation failure remains visible');
    assert.equal(app.current.plan.current, false, 'Retained evidence is never current export authorization');
    assert(app.current.plan.plan!.resources.some(resource => resource.id === 'file:README.md' && resource.selectable));
    assert.deepEqual(app.current.selection.selectedIds, ['file:README.md']);
    await act(async () => { await app.current.selection.refresh(); app.current.plan.refresh(); });
    await until(() => app.current.plan.current, 'The shared recovery sequence produces fresh evidence');
    assert.equal(app.current.plan.error, ''); assert.equal(backend.writes, 1);
  } finally { await app.dispose(); }
});

test('a late initial registration is released after unmount without performing a selection write', { timeout: 10000 }, async () => {
  const backend = new MemoryBridge('late-initial-read'); backend.listenMode = 'fail'; backend.deferInitialGet();
  const app = await mount(backend);
  try {
    assert.equal(app.current.selection.draft, null);
    await app.unmount(); const renders = app.renders;
    await act(async () => { backend.resolveInitialGet(app.paneId); await delay(20); });
    assert.equal(backend.releases, 1); assert.equal(backend.leases.size, 0);
    assert.equal(backend.writes, 0); assert.equal(app.renders, renders);
  } finally { await app.dispose(); }
});