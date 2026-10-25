import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createElement, act } from 'react';
import { JSDOM } from 'jsdom';
import ts from 'typescript';
import type { LocalImportDraft } from '../src/local-sources.ts';

// Load the actual component and helper, without copying their implementation.
const modules = new Map<string, Promise<string>>();
async function moduleURL(url: URL): Promise<string> {
  const previous = modules.get(url.href); if (previous) return previous;
  const loading = (async () => {
    let code = ts.transpileModule(await readFile(url, 'utf8'), { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, jsx: ts.JsxEmit.ReactJSX } }).outputText.replace(/import\s+['"][^'"]+\.css['"];?/g, '');
    const replacements = new Map<string, string>();
    for (const match of code.matchAll(/from\s+(['"])([^'"]+)\1/g)) {
      const value = match[2]; if (replacements.has(value)) continue;
      replacements.set(value, value.startsWith('.') ? await moduleURL(new URL(/\.[cm]?[jt]sx?$/.test(value) ? value : value + '.ts', url)) : import.meta.resolve(value));
    }
    code = code.replace(/from\s+(['"])([^'"]+)\1/g, (_match,_quote,value) => `from ${JSON.stringify(replacements.get(value))}`);
    return `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`;
  })(); modules.set(url.href,loading); return loading;
}
const dom = new JSDOM('<!doctype html><html><body><div id="root"></div></body></html>', {url:'http://localhost'});
Object.assign(globalThis,{window:dom.window,document:dom.window.document,HTMLElement:dom.window.HTMLElement,Element:dom.window.Element,Node:dom.window.Node,IS_REACT_ACT_ENVIRONMENT:true});
const { createRoot } = await import('react-dom/client');
const { LocalSources } = await import(await moduleURL(new URL('../src/LocalSources.tsx',import.meta.url)));
const host=document.getElementById('root')!;
let cleanup=()=>{};
function createTestRoot(){const root=createRoot(host);cleanup=()=>root.unmount();return root;}
afterEach(async()=>{await act(async()=>cleanup());cleanup=()=>{};});
const buttons=(text:string) => Array.from(host.querySelectorAll<HTMLButtonElement>('button')).filter(button=>button.textContent?.includes(text));
const box=(name:string) => host.querySelector<HTMLInputElement>(`input[aria-label="选择 ${name}"]`)!;
const press=async(element:Element,options:MouseEventInit={})=>act(async()=>{element.dispatchEvent(new dom.window.MouseEvent('click',{bubbles:true,cancelable:true,...options}));});
const noop=()=>{};

test('multiple directory preview preserves managed, readonly and ignored choices and sends explicit selection/exclusions',async()=>{
  const calls:{command:string;args:any}[]=[];
  let draft:LocalImportDraft|undefined;
  Object.assign(dom.window,{__TAURI_INTERNALS__:{invoke:async(command:string,args:any)=>{assert.equal(command,'plugin:dialog|open');assert.equal(args.options.multiple,true);return ['D:/synthetic/skills','D:/synthetic/extra'];}}});
  const action=async(_label:string,command:string,args:any)=>{
    calls.push({command,args});
    if(command==='scan_local')return args.path.endsWith('/skills') ? {skills:[{name:'new',localPath:'D:/synthetic/skills/new'},{name:'known',localPath:'D:/synthetic/skills/known'},{name:'ignored',localPath:'D:/synthetic/skills/ignored'},{name:'system',readOnly:true,localPath:'D:/synthetic/skills/.system/system'}]} : {skills:[{name:'other',localPath:'D:/synthetic/extra/other'},{name:'known',localPath:'d:\\synthetic\\skills\\known'}]};
    return {skills:[]};
  };
  const root=createTestRoot();
  await act(async()=>root.render(createElement(LocalSources,{roots:[],ignoredPaths:['D:/synthetic/skills/ignored'],skills:[{name:'known',localPath:'D:/synthetic/skills/known'}],onDraft:(value:LocalImportDraft)=>{draft=value;},action,onClose:noop})));
  await press(buttons('选择目录')[0]);await press(buttons('预览')[0]);
  assert.equal(host.querySelectorAll('.local-discovery-row').length,5);
  assert.equal(box('known').disabled,true);assert.equal(box('known').checked,true);
  assert.equal(box('system').checked,false);assert.equal(box('ignored').checked,false);
  assert.deepEqual(draft?.selected,['D:/synthetic/skills/new','D:/synthetic/extra/other']);
  await press(box('new'));
  await press(box('system'));
  await press(buttons('保存目录并登记所选')[0]);
  const saved=calls.find(call=>call.command==='add_local_roots');assert(saved);
  assert.deepEqual(saved.args.paths,['D:/synthetic/skills','D:/synthetic/extra']);
  assert.deepEqual(saved.args.selectedPaths,['D:/synthetic/extra/other','D:/synthetic/skills/.system/system']);
  assert.deepEqual(saved.args.excludedPaths,['D:/synthetic/skills/new','D:/synthetic/skills/ignored']);
  assert(calls.every(call=>['scan_local','add_local_roots'].includes(call.command)));
});

test('actual checkbox Shift range and list Ctrl+A work without intercepting search text shortcuts',async()=>{
  let draft:LocalImportDraft|undefined;
  const candidates=['one','two','three'].map(name=>({name,localPath:`D:/synthetic/${name}`}));
  const initialDraft={paths:['D:/synthetic'],input:'',candidates,selected:[],query:'',automatic:true};
  const root=createTestRoot();
  await act(async()=>root.render(createElement(LocalSources,{roots:[],ignoredPaths:[],skills:[],initialDraft,onDraft:(value:LocalImportDraft)=>{draft=value;},action:async()=>({}),onClose:noop})));
  await press(box('one'));await press(box('three'),{shiftKey:true});
  assert.deepEqual(draft?.selected,candidates.map(item=>item.localPath));
  await press(buttons('取消选择')[0]);assert.deepEqual(draft?.selected,[]);
  const search=host.querySelector<HTMLInputElement>('[aria-label="搜索发现的 Skill"]')!;
  const textShortcut=new dom.window.KeyboardEvent('keydown',{key:'a',ctrlKey:true,bubbles:true,cancelable:true});
  await act(async()=>{search.dispatchEvent(textShortcut);});
  assert.equal(textShortcut.defaultPrevented,false);assert.deepEqual(draft?.selected,[]);
  const list=host.querySelector<HTMLElement>('[aria-label="发现的 Skill 列表"]')!;
  const listShortcut=new dom.window.KeyboardEvent('keydown',{key:'a',ctrlKey:true,bubbles:true,cancelable:true});
  await act(async()=>{list.dispatchEvent(listShortcut);});
  assert.equal(listShortcut.defaultPrevented,true);assert.deepEqual(draft?.selected,candidates.map(item=>item.localPath));
});

test('root pause/remove and ignored restore dispatch management-only commands with selected IDs',async()=>{
  const calls:{command:string;args:any}[]=[];
  const action=async(_label:string,command:string,args:any)=>{calls.push({command,args});return {skills:[]};};
  const roots=[{id:'a',name:'Alpha',path:'D:/synthetic/a',enabled:true},{id:'b',name:'Beta',path:'D:/synthetic/b',enabled:false}];
  const root=createTestRoot();
  await act(async()=>root.render(createElement(LocalSources,{roots,ignoredPaths:['D:/synthetic/a/ignored'],skills:[],onDraft:noop,action,onClose:noop})));
  await press(buttons('暂停')[0]);
  assert.deepEqual(calls[0],{command:'update_local_root',args:{rootId:'a',enabled:false}});
  await press(host.querySelector<HTMLInputElement>('[aria-label="选择目录 Alpha"]')!);
  await press(buttons('移除目录 (1)')[0]);
  assert.deepEqual(calls[1],{command:'remove_local_roots',args:{rootIds:['a']}});
  const ignored=host.querySelector<HTMLDetailsElement>('.local-ignored')!;
  await act(async()=>{ignored.open=true;});
  await press(ignored.querySelector<HTMLInputElement>('label input')!);
  await press(buttons('恢复所选 (1)')[0]);
  assert.deepEqual(calls[2],{command:'restore_local_ignores',args:{paths:['D:/synthetic/a/ignored']}});
  assert(calls.every(call=>!['forget','delete_skill','disconnect'].includes(call.command)));
});
