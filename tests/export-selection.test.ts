import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { clusterExportReferences, exportInclusionLabel, exportPlanReadKeys, exportPlanView, exportResourceState, filterExportResources, optionalExportIds, resourceForReference, resourcePathParts, type ExportPlan, type ExportResource } from '../src/export-selection.ts';

const resource = (id: string, change: Partial<ExportResource> = {}): ExportResource => ({ id, label: id, path: id, group: 'optional_external', included: false, selectable: true, source: 'local', reason: '普通环境文件，仅本次选择', referenceIds: [], ...change });

test('changing a ZIP choice retains the same resource plan while the new selection remains unverified', () => {
  const plan: ExportPlan = { id: 'before-choice', skillId: 'one', resources: [resource('README.md')], manifest: [], summary: { required: 1, optionalPackageSelected: 0, optionalPackageTotal: 1, optionalExternalSelected: 0, optionalExternalTotal: 0, generated: 0, total: 1, remoteCount: 0, bytes: 1 }, canExport: true, issues: [] };
  const prior = { key: 'none-selected', contextKey: 'one:source:epoch2:commitA:bytesA', plan, error: '', complete: true, fromCache: false };
  const pending = exportPlanView(prior, 'readme-selected', prior.contextKey);
  assert.equal(pending.plan, plan, 'the mounted resource groups keep their existing objects');
  assert.equal(pending.current, false, 'retained display cannot authorize the changed ZIP');
  assert.equal(exportPlanView({ ...prior, key: 'readme-selected', complete: false }, 'readme-selected', prior.contextKey).current, false);
  const verified = { ...plan, id: 'after-choice' };
  assert.deepEqual(exportPlanView({ ...prior, key: 'readme-selected', plan: verified }, 'readme-selected', prior.contextKey), { plan: verified, current: true });
});

test('retained export rows cannot cross Skill, source, credential, commit or local content contexts', () => {
  const plan = { skillId: 'one' } as ExportPlan;
  const context = ['one', 'sourceA:epoch2:bytesA', 'commitA', true];
  const prior = { key: 'old-selection', contextKey: JSON.stringify(context), plan, error: '', complete: true, fromCache: false };
  for (const changed of [['two', context[1], context[2], true], ['one', 'sourceB:epoch2:bytesA', context[2], true], ['one', 'sourceA:epoch3:bytesA', context[2], true], ['one', 'sourceA:epoch2:bytesB', context[2], true], ['one', context[1], 'commitB', true], ['one', context[1], context[2], false]]) {
    assert.deepEqual(exportPlanView(prior, 'new-selection', JSON.stringify(changed)), { plan: null, current: false });
  }
  assert.equal(exportPlanView(prior, 'new-selection', prior.contextKey, undefined, false).plan, null, 'unknown read context permits no previous-selection reuse');
  assert.equal(exportPlanView({ ...prior, plan: null, error: '401' }, prior.key, prior.contextKey).plan, null);
});

test('manual recheck immediately invalidates the current manifest while keeping optional groups mounted', () => {
  const plan = { skillId: 'one', canExport: true } as ExportPlan;
  const contextKey = 'one:source:epoch2:commitA:bytesA';
  const before = exportPlanReadKeys(contextKey, '["memory"]', 0), refresh = exportPlanReadKeys(contextKey, '["memory"]', 1);
  assert.equal(before.cacheKey, refresh.cacheKey); assert.notEqual(before.requestKey, refresh.requestKey);
  const prior = { key: before.requestKey, contextKey, plan, error: '', complete: true, fromCache: false };
  const displayed = exportPlanView(prior, refresh.requestKey, contextKey, plan);
  assert.equal(displayed.plan, plan); assert.equal(displayed.current, false, 'a refresh click cannot expose the old proof as current');
  const blocked = { ...plan, canExport: false };
  assert.deepEqual(exportPlanView({ ...prior, key: refresh.requestKey, plan: blocked }, refresh.requestKey, contextKey), { plan: blocked, current: true });
});

test('export select-all includes only available optional resources and uses resource IDs once', () => {
  assert.deepEqual(optionalExportIds([resource('one'), resource('one'), resource('package', { group: 'optional_package' }), resource('required', { group: 'required' }), resource('command', { group: 'environment', selectable: false }), resource('missing', { selectable: false }), resource('stale', { included: true, selectable: true, source: 'none' }), resource('generated', { source: 'generated', group: 'required' })]), ['one', 'package']);
});

test('resource search preserves selected versus unavailable rows without conflating source or filename', () => {
  const first = resource('one', { label: 'AGENTS.md', path: 'D:/rules/AGENTS.md', included: true });
  const other = resource('two', { label: 'AGENTS.md', path: 'D:/other/AGENTS.md' });
  assert.deepEqual(filterExportResources([first, other], 'agents rules'), [first]);
  assert.deepEqual(filterExportResources([first, other], '', true), [first]);
  assert.deepEqual(filterExportResources([first, other], 'agents'), [first, other]);
});

test('unknown and unavailable required resources cannot show automatic success', () => {
  assert.deepEqual(exportResourceState(), { automatic: false, available: false, selectable: false, problem: false, label: '导出范围待核对' });
  for (const change of [{ included: false }, { source: 'none' as const }, { materializable: false }]) {
    const state = exportResourceState(resource('required', { group: 'required', included: true, selectable: false, ...change }));
    assert.equal(state.available, false);
    assert.equal(state.problem, true);
    assert.equal(state.selectable, false);
    assert.equal(state.label, '必要文件待处理');
  }
});

test('a selected failed optional file remains removable and select-all excludes unusable sources', () => {
  const failed = resource('failed', { included: true, selectable: false, source: 'none', materializable: false, group: 'environment' });
  assert.equal(exportResourceState(failed).selectable, true);
  assert.equal(exportResourceState(failed).problem, true);
  assert.equal(exportResourceState({ ...failed, included: false }).selectable, false);
  assert.deepEqual(optionalExportIds([resource('blocked', { materializable: false }), resource('ready')]), ['ready']);
});

test('a failed refresh displays required evidence as pending without changing the resource or optional removal', () => {
  const required = resource('required', { group: 'required', included: true, selectable: false });
  const original = { ...required };
  assert.equal(exportResourceState(required, true).label, '导出范围待核对');
  assert.equal(exportResourceState(required, true).available, false);
  assert.deepEqual(required, original);
  const optional = resource('failed', { included: true, selectable: false, source: 'none', materializable: false });
  assert.equal(exportResourceState(optional, true).selectable, true);
});

test('resource paths show the filename and directory once without conflating same basenames', () => {
  assert.deepEqual(resourcePathParts('D:\\rules\\AGENTS.md'), { name: 'AGENTS.md', directory: 'D:/rules' });
  assert.deepEqual(resourcePathParts('shared/AGENTS.md'), { name: 'AGENTS.md', directory: 'shared' });
  assert.deepEqual(resourcePathParts('SKILL.md'), { name: 'SKILL.md', directory: '' });
});

test('one primary resource owns all its reference controls without duplicate export choices', () => {
  const value = resource('physical', { referenceIds: ['first-reference', 'second-reference'] });
  assert.equal(resourceForReference([value], 'first-reference'), value);
  assert.equal(resourceForReference([value], 'second-reference'), value);
  assert.equal(resourceForReference([value], 'missing'), undefined);
});

test('standalone references collapse only under a proven shared resource ID, never their filename', () => {
  const references = [{ id: 'first', target: 'same.md' }, { id: 'second', target: 'same.md' }, { id: 'other', target: 'same.md' }, { id: 'unknown', target: 'same.md' }];
  const resources = [resource('one', { referenceIds: ['first', 'second'] }), resource('other', { referenceIds: ['other'] })];
  assert.deepEqual(clusterExportReferences(references, resources).map(group => group.map(item => item.id)), [['first', 'second'], ['other'], ['unknown']]);
});

test('unavailable or environment resources still count once under an authoritative resource ID', () => {
  const references = [{ id: 'first' }, { id: 'second' }, { id: 'unknown' }];
  assert.deepEqual(clusterExportReferences(references, [resource('blocked', { group: 'environment', materializable: false, referenceIds: ['first', 'second'] })]).map(group => group.map(item => item.id)), [['first', 'second'], ['unknown']]);
});

test('inclusion wording distinguishes automatic, selected, unselected and non-file states', () => {
  assert.equal(exportInclusionLabel(resource('required', { group: 'required', included: true, selectable: false })), '自动包含');
  assert.equal(exportInclusionLabel(resource('missing', { group: 'required', included: true, selectable: false, source: 'none' })), '必要文件待处理');
  assert.equal(exportInclusionLabel(resource('protected', { group: 'required', included: true, selectable: false, materializable: false })), '必要文件待处理');
  assert.equal(exportInclusionLabel(resource('selected-missing', { group: 'environment', included: true, selectable: false, source: 'none', materializable: false })), '已选文件待处理，可取消选择');
  assert.equal(exportInclusionLabel(resource('selected', { included: true })), '本次包含');
  assert.equal(exportInclusionLabel(resource('unselected')), '未选择');
  assert.equal(exportInclusionLabel(resource('command', { group: 'environment', selectable: false })), '不加入 ZIP');
  assert.equal(exportInclusionLabel(resource('record', { group: 'required', source: 'generated', included: true })), '自动生成并包含');
});

test('the three theme foreground and control palettes meet the planned contrast on normal and selected surfaces', () => {
  const styles = readFileSync(new URL('../src/themes.css', import.meta.url), 'utf8');
  const colors = (selector: string) => Object.assign({}, ...[...styles.matchAll(/(:root(?:\[data-theme=(?:light|dark|eye)\])?)\s*\{([^}]+)\}/g)].filter(match => match[1] === selector).map(match => Object.fromEntries([...match[2].matchAll(/(--[\w-]+):\s*([^;]+)(?:;|$)/gi)].map(value => [value[1], value[2].trim()])))) as Record<string, string>;
  const luminance = (hex: string) => {
    const text = hex.length === 4 ? hex.slice(1).split('').map(value => value + value).join('') : hex.slice(1);
    const values = text.match(/../g)!.map(value => parseInt(value, 16) / 255).map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
    return values.reduce((total, value, index) => total + value * [.2126, .7152, .0722][index], 0);
  };
  const contrast = (first: string, second: string) => { const a = luminance(first), b = luminance(second); return (Math.max(a, b) + .05) / (Math.min(a, b) + .05); };
  for (const theme of ['light', 'dark', 'eye']) {
    const variables = { ...colors(':root'), ...colors(`:root[data-theme=${theme}]`) };
    const resolve = (key: string): string => { const value = variables[key]; assert(value, `Missing ${theme} ${key}`); const alias = /^var\((--[\w-]+)\)$/.exec(value); return alias ? resolve(alias[1]) : value; };
    const palette = Object.fromEntries(Object.keys(variables).map(key => [key, resolve(key)]));
    for (const background of ['--surface', '--surface-soft', '--selected']) {
      assert(contrast(palette['--text'], palette[background]) >= 7, `${theme} body ${background}`);
      assert(contrast(palette['--text-secondary'], palette[background]) >= 4.5, `${theme} secondary ${background}`);
      assert(contrast(palette['--muted'], palette[background]) >= 4.5, `${theme} auxiliary ${background}`);
      assert(contrast(palette['--control-line'], palette[background]) >= 3, `${theme} controls ${background}`);
    }
    for (const state of ['good', 'warn', 'bad', 'info']) assert(contrast(palette[`--${state}`], palette[`--${state}-bg`]) >= 4.5, `${theme} ${state} label`);
    assert(contrast(palette['--neutral-text'], palette['--neutral-bg']) >= 4.5, `${theme} neutral status label`);
    assert(contrast(palette['--accent'], palette['--on-accent']) >= 4.5, `${theme} primary action`);
  }
});
