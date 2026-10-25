import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createElement, act } from 'react';
import { JSDOM } from 'jsdom';
import ts from 'typescript';

const modules = new Map<string, Promise<string>>();
async function moduleURL(url: URL): Promise<string> {
  let cached = modules.get(url.href); if (cached) return cached;
  cached = (async () => {
    let code = ts.transpileModule(await readFile(url, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } }).outputText.replace(/import\s+['"][^'"]+\.css['"];?/g, '');
    const replacements = new Map<string, string>();
    for (const match of code.matchAll(/from\s+(['"])([^'"]+)\1/g)) {
      const value = match[2]; if (replacements.has(value)) continue;
      if (value.startsWith('.')) {
        const path = /\.[cm]?[jt]sx?$/.test(value) ? value : ['FloatingMenu','CheckControl','ChangesPanel'].some(name => value.endsWith(name)) ? value + '.tsx' : value + '.ts';
        replacements.set(value, await moduleURL(new URL(path, url)));
      } else replacements.set(value, import.meta.resolve(value));
    }
    code = code.replace(/from\s+(['"])([^'"]+)\1/g, (_m,_q,value) => `from ${JSON.stringify(replacements.get(value))}`);
    return `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`;
  })(); modules.set(url.href,cached); return cached;
}
const dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', { url: 'http://localhost' });
Object.assign(globalThis, { window:dom.window, document:dom.window.document, HTMLElement:dom.window.HTMLElement, Element:dom.window.Element, Node:dom.window.Node, getComputedStyle:dom.window.getComputedStyle.bind(dom.window), requestAnimationFrame:(fn:FrameRequestCallback)=>setTimeout(()=>fn(0),0), cancelAnimationFrame:clearTimeout, IS_REACT_ACT_ENVIRONMENT:true });
const { createRoot } = await import('react-dom/client');
const { CheckControl } = await import(await moduleURL(new URL('../src/CheckControl.tsx',import.meta.url)));
const { ChangesPanel } = await import(await moduleURL(new URL('../src/ChangesPanel.tsx',import.meta.url)));
const host=document.getElementById('root')!;
test('global mode is persisted through the actual control and manual check does not enable automatic mode', async () => {
  const calls: unknown[] = [], snapshots: unknown[]=[];
  Object.assign(dom.window,{ __TAURI_INTERNALS__:{ invoke: async (_command:string,args:any) => { calls.push(args.request); return {checkMode:args.request.mode}; } } });
  const tasks={tasks:[],run:async (...args:unknown[]) => { calls.push(args); return { checkMode:'manual' }; }};
  const root=createRoot(host);
  await act(async()=>root.render(createElement(CheckControl,{snapshot:{checkMode:'auto'},tasks,onSnapshot:(value:unknown)=>snapshots.push(value)})));
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="检查方式"]')!.click());
  await act(async()=>Array.from(document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).find(button=>button.textContent?.includes('手动检查'))!.click());
  assert.deepEqual(calls[0],{action:'set_check_mode',mode:'manual'});
  assert.deepEqual(snapshots[0],{checkMode:'manual'});
  await act(async()=>root.render(createElement(CheckControl,{snapshot:{checkMode:'manual'},tasks,onSnapshot:(value:unknown)=>snapshots.push(value)})));
  assert(host.textContent?.includes('手动'));
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="检查本地与 GitHub 变化"]')!.click());
  assert.equal((calls[1] as unknown[])[1],'check_now');
  assert.equal(calls.filter(item=>!Array.isArray(item)).length,1);
  await act(async()=>root.unmount());
});
test('Changes selection checks requested IDs only and successful discoveries stay in recap', async () => {
  const selected: string[][]=[];
  const root=createRoot(host), noop=()=>{};
  await act(async()=>root.render(createElement(ChangesPanel,{skills:[{id:'same',name:'same',localPath:'D:/same',status:'synced'},{id:'changed',name:'changed',localPath:'D:/changed',sourceId:'s',status:'remote_updated'}],checkSummary:{newLocal:1,associated:1},onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop,onCheckMany:(ids:string[])=>selected.push(ids)})));
  assert.equal(host.querySelectorAll('[data-change-skill]').length,1);
  await act(async()=>host.querySelector<HTMLInputElement>('[aria-label="选择变化 changed"]')!.click());
  await act(async()=>Array.from(host.querySelectorAll<HTMLButtonElement>('button')).find(button=>button.textContent?.includes('检查／重试所选'))!.click());
  assert.deepEqual(selected,[['changed']]);
  assert(host.querySelector('.check-recap')?.textContent?.includes('新关联'));
  await act(async()=>root.unmount());
});
