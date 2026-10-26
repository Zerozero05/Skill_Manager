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
test('one pending task has a direct recheck without replacing its comparison action or adding batch controls', async () => {
  const selected: string[][]=[], compared:string[]=[];
  const root=createRoot(host), noop=()=>{};
  const props={checkSummary:{newLocal:1,associated:1},onOpen:noop,onCompare:(skill:{id:string})=>{compared.push(skill.id);},onUpload:noop,onDocs:noop,onFiles:noop,onCheck:(skill:{id:string})=>{selected.push([skill.id]);},onCheckMany:(ids:string[])=>selected.push(ids)};
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[{id:'same',name:'same',localPath:'D:/same',status:'synced'},{id:'changed',name:'changed',localPath:'D:/changed',sourceId:'s',status:'remote_updated'}]})));
  assert.equal(host.querySelectorAll('[data-change-skill]').length,1);
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  assert.equal(host.querySelector('.changes-selection'),null);
  assert.equal(host.querySelector('[aria-label="批量选择 Skill"]'),null);
  const compare=host.querySelector<HTMLButtonElement>('[data-change-skill="changed"] .changes-actions .primary')!;
  assert.equal(compare.textContent,'预览更新');
  await act(async()=>compare.click());
  assert.deepEqual(compared,['changed']);
  await act(async()=>host.querySelector<HTMLButtonElement>('.changes-list-action')!.click());
  assert.deepEqual(selected,[['changed']]);
  assert(host.querySelector('.check-recap')?.textContent?.includes('新关联'));
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[{id:'changed',name:'changed',status:'error',error:'offline'}]})));
  assert.equal(Array.from(host.querySelectorAll('button')).filter(button=>button.textContent==='重新检查').length,1,'An existing retry primary action must not be duplicated');
  await act(async()=>root.unmount());
});

test('processed documentation is reported only inside the existing recap and old summaries add no row', async () => {
  const root=createRoot(host), noop=()=>{};
  const props={skills:[{id:'docs',name:'documentation',localPath:'D:/docs',status:'docs_updated'}],onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop};
  const processed=()=>Array.from(host.querySelectorAll('dt')).find(node=>node.textContent==='已处理说明');
  await act(async()=>root.render(createElement(ChangesPanel,{...props,checkSummary:{newLocal:0,docsUpdated:2,needsAttention:0}})));
  assert.equal(host.querySelectorAll('[data-change-skill]').length,0);
  const recap=host.querySelector<HTMLDetailsElement>('.check-recap')!;
  assert.equal(recap.open,false);
  assert.equal(processed()?.nextElementSibling?.textContent,'2');
  assert.equal(processed()?.closest('details'),recap);
  assert.equal(host.querySelectorAll('.check-recap').length,1);
  await act(async()=>root.render(createElement(ChangesPanel,{...props,checkSummary:{docsUpdated:0,needsAttention:0}})));
  assert.equal(processed()?.nextElementSibling?.textContent,'0');
  await act(async()=>root.render(createElement(ChangesPanel,{...props,checkSummary:{newLocal:0,needsAttention:0}})));
  assert.equal(processed(),undefined);
  await act(async()=>root.unmount());
});

test('resolved documentation leaves batch selection while remaining tasks retain selection and shortcuts', async () => {
  const selected:string[][]=[], root=createRoot(host), noop=()=>{};
  const props={onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop,onCheckMany:(ids:string[])=>selected.push(ids)};
  const pending={id:'docs',name:'docs',localPath:'D:/docs',status:'docs_updated',localNotice:'Local file needs review'};
  const remaining={id:'remaining',name:'remaining',localPath:'D:/remaining',sourceId:'s',status:'remote_updated'};
  const batch=()=>host.querySelector<HTMLButtonElement>('[aria-label="检查所选 Skill"]')!;
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[pending,remaining]})));
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="批量选择 Skill"]')!.click());
  await act(async()=>host.querySelector<HTMLInputElement>('[aria-label="选择变化 docs"]')!.click());
  assert.equal(batch().disabled,false);
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[{...pending,localNotice:null},remaining],checkSummary:{docsUpdated:1,needsAttention:1}})));
  assert.equal(host.querySelector('[data-change-skill="docs"]'),null);
  assert.equal(batch().disabled,true,'A task that left the list must not stay in requested IDs');
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[pending,remaining]})));
  assert.equal(host.querySelector<HTMLInputElement>('[aria-label="选择变化 docs"]')!.checked,false,'A later problem must not restore a completed selection');
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[{...pending,localNotice:null},remaining]})));
  const rows=host.querySelector<HTMLElement>('[aria-label="待处理 Skill 列表"]')!;
  await act(async()=>rows.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'a',ctrlKey:true,bubbles:true})));
  assert.equal(host.querySelector<HTMLInputElement>('[aria-label="选择变化 remaining"]')!.checked,true);
  await act(async()=>batch().click());
  assert.deepEqual(selected,[['remaining']]);
  await act(async()=>rows.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
  assert.equal(host.querySelector('.changes-selection'),null);
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  await act(async()=>root.unmount());
});

test('documentation with an unavailable required external file opens files and source errors remain visible', async () => {
  const calls:string[]=[], root=createRoot(host), noop=()=>{};
  await act(async()=>root.render(createElement(ChangesPanel,{
    skills:[{id:'needs-file',name:'needs-file',localPath:'D:/needs-file',status:'docs_updated',runtimeAudit:{status:'needs_review',reason:'Required external file is missing'},runtimeIssues:[{category:'external',severity:'warning',source:'SKILL.md',target:'shared/rules.md',reason:'External rules need a binding'}]}],
    sourceErrors:[{id:'offline',repo:'synthetic/unavailable',error:'Source could not be checked'}],checkSummary:{docsUpdated:1,needsAttention:1,errors:1},
    onOpen:noop,onCompare:noop,onUpload:noop,onDocs:()=>calls.push('docs'),onFiles:()=>calls.push('files'),onCheck:noop,
  })));
  const row=host.querySelector<HTMLElement>('[data-change-skill="needs-file"]')!;
  assert(row.textContent?.includes('Required external file is missing'));
  const primary=row.querySelector<HTMLButtonElement>('.changes-actions .primary')!;
  assert.equal(primary.textContent,'查看文件');
  await act(async()=>primary.click());
  assert.deepEqual(calls,['files']);
  assert(host.querySelector('[data-change-source="offline"]')?.textContent?.includes('Source could not be checked'));
  await act(async()=>root.unmount());
});

test('a remaining task keeps its batch selection when pagination hides its row', async () => {
  const selected:string[][]=[], root=createRoot(host), noop=()=>{};
  const skills=Array.from({length:21},(_,index)=>({id:`pending-${index}`,name:`pending-${index}`,localPath:`D:/pending-${index}`,sourceId:'s',status:'remote_updated'}));
  await act(async()=>root.render(createElement(ChangesPanel,{skills,onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop,onCheckMany:(ids:string[])=>selected.push(ids)})));
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="批量选择 Skill"]')!.click());
  await act(async()=>host.querySelector<HTMLInputElement>('[aria-label="选择变化 pending-0"]')!.click());
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="下一页变化"]')!.click());
  assert.equal(host.querySelector('[data-change-skill="pending-0"]'),null);
  const batch=host.querySelector<HTMLButtonElement>('[aria-label="检查所选 Skill"]')!;
  assert.equal(batch.disabled,false);
  await act(async()=>batch.click());
  assert.deepEqual(selected,[['pending-0']]);
  await act(async()=>root.unmount());
});

test('batch controls are opt-in, grouped and can clear or exit without invoking a task', async () => {
  const requested:string[][]=[], root=createRoot(host), noop=()=>{};
  const skills=['one','two'].map(id=>({id,name:id,localPath:`D:/${id}`,sourceId:'s',status:'remote_updated'}));
  await act(async()=>root.render(createElement(ChangesPanel,{skills,onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop,onCheckMany:(ids:string[])=>requested.push(ids)})));
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  assert.equal(host.querySelector('.changes-selection'),null);
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="批量选择 Skill"]')!.click());
  const toolbar=host.querySelector<HTMLElement>('[aria-label="变化批量选择"]')!;
  assert.equal(host.querySelectorAll('input[type="checkbox"]').length,2);
  assert.equal(toolbar.querySelectorAll('.changes-selection-controls').length,1);
  assert.equal(toolbar.querySelectorAll('.changes-selection-actions').length,1);
  assert.equal(toolbar.querySelector('.changes-selection-count')?.textContent,'已选 0 项');
  const selectAll=Array.from(toolbar.querySelectorAll<HTMLButtonElement>('button')).find(button=>button.textContent?.startsWith('选择当前结果'))!;
  await act(async()=>selectAll.click());
  assert.equal(toolbar.querySelector('.changes-selection-count')?.textContent,'已选 2 项');
  const batch=toolbar.querySelector<HTMLButtonElement>('[aria-label="检查所选 Skill"]')!;
  await act(async()=>batch.click());
  assert.deepEqual(requested,[['one','two']]);
  await act(async()=>Array.from(toolbar.querySelectorAll<HTMLButtonElement>('button')).find(button=>button.textContent==='清空')!.click());
  assert.equal(batch.disabled,true);
  assert.equal(host.querySelectorAll('input[type="checkbox"]').length,2,'Clear keeps the explicit batch mode open');
  await act(async()=>Array.from(toolbar.querySelectorAll<HTMLButtonElement>('button')).find(button=>button.textContent==='退出选择')!.click());
  assert.equal(host.querySelector('.changes-selection'),null);
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  assert.equal(requested.length,1,'Selection-only operations do not submit new checks');
  await act(async()=>host.querySelector<HTMLElement>('[aria-label="待处理 Skill 列表"]')!.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'a',ctrlKey:true,bubbles:true})));
  const activeToolbar=host.querySelector<HTMLElement>('[aria-label="变化批量选择"]')!;
  await act(async()=>activeToolbar.querySelector('button')!.dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'Escape',bubbles:true})));
  assert.equal(host.querySelector('.changes-selection'),null,'Escape also exits when a toolbar control owns focus');
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  await act(async()=>root.unmount());
});

test('Ctrl+A enters batch mode, busy blocks selection changes, and zero pending tasks reset the mode', async () => {
  const requested:string[][]=[], root=createRoot(host), noop=()=>{};
  const skills=['one','two'].map(id=>({id,name:id,localPath:`D:/${id}`,sourceId:'s',status:'remote_updated'}));
  const props={skills,onOpen:noop,onCompare:noop,onUpload:noop,onDocs:noop,onFiles:noop,onCheck:noop,onCheckMany:(ids:string[])=>requested.push(ids)};
  const rows=()=>host.querySelector<HTMLElement>('[aria-label="待处理 Skill 列表"]')!;
  const key=(value:string)=>rows().dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:value,ctrlKey:value==='a',bubbles:true}));
  await act(async()=>root.render(createElement(ChangesPanel,{...props,busy:true})));
  assert.equal(host.querySelector<HTMLButtonElement>('[aria-label="批量选择 Skill"]')!.disabled,true);
  await act(async()=>key('a'));
  assert.equal(host.querySelector('.changes-selection'),null);
  await act(async()=>root.render(createElement(ChangesPanel,props)));
  await act(async()=>key('a'));
  assert.equal(host.querySelectorAll('input[type="checkbox"]:checked').length,2);
  await act(async()=>root.render(createElement(ChangesPanel,{...props,busy:true})));
  assert(Array.from(host.querySelectorAll<HTMLButtonElement>('.changes-selection button')).every(button=>button.disabled));
  assert(Array.from(host.querySelectorAll<HTMLInputElement>('input[type="checkbox"]')).every(input=>input.disabled));
  await act(async()=>key('Escape'));
  assert.equal(host.querySelectorAll('input[type="checkbox"]:checked').length,2);
  assert.deepEqual(requested,[]);
  await act(async()=>root.render(createElement(ChangesPanel,props)));
  await act(async()=>key('Escape'));
  assert.equal(host.querySelector('.changes-selection'),null);
  await act(async()=>key('a'));
  await act(async()=>root.render(createElement(ChangesPanel,{...props,skills:[]})));
  assert.equal(host.querySelector('.changes-selection'),null);
  assert.equal(host.querySelector('input[type="checkbox"]'),null);
  await act(async()=>root.render(createElement(ChangesPanel,props)));
  assert.equal(host.querySelector('.changes-selection'),null);
  await act(async()=>host.querySelector<HTMLButtonElement>('[aria-label="批量选择 Skill"]')!.click());
  assert.equal(host.querySelectorAll('input[type="checkbox"]:checked').length,0,'Resolved selections do not revive when problems return');
  await act(async()=>root.unmount());
});
