import test from 'node:test';
import assert from 'node:assert/strict';
import { queryLibrary, paginateLibrary, needsAttention, libraryCounts, githubFolderUrl, displayStatus, statusExplanation, comparisonScope, selectionForDrag, rangeSelection, libraryStatusOptions, statusFilterLabel, LIBRARY_STATUS_GROUPS, type LibraryItem } from '../src/library.ts';
import { libraryMode, cardNavigationIndex } from '../src/library.ts';

test('cards preference reads are compatible and actual grid columns govern keyboard navigation', () => {
  for (const mode of ['list','details','cards'] as const) assert.equal(libraryMode(mode), mode);
  for (const value of [undefined, null, '', 'future', 42, {}]) assert.equal(libraryMode(value),'list');
  assert.equal(cardNavigationIndex(1,8,3,'ArrowDown'),4);
  assert.equal(cardNavigationIndex(7,8,3,'ArrowUp'),4);
  assert.equal(cardNavigationIndex(4,8,1,'ArrowDown'),5);
  assert.equal(cardNavigationIndex(6,8,3,'ArrowDown'),7);
  assert.equal(cardNavigationIndex(0,8,3,'ArrowLeft'),0);
  assert.equal(cardNavigationIndex(7,8,3,'Home'),0);
  assert.equal(cardNavigationIndex(0,8,3,'End'),7);
  assert.equal(cardNavigationIndex(0,0,3,'ArrowDown'),null);
  assert.equal(cardNavigationIndex(-1,8,3,'ArrowDown'),null);
  assert.equal(cardNavigationIndex(1,8,3,'Enter'),null);
});

const collection: LibraryItem[] = Array.from({ length: 517 }, (_, n) => ({ id: String(n), name: `skill-${n + 1}`, description: n === 501 ? '中文翻译 PDF' : 'Synthetic local skill', localPath: `D:/fixture/${n + 1}`, status: 'local_only' }));
test('status labels and filters share error/pending precedence instead of advertising cached equality', () => {
  const items: LibraryItem[] = [
    {id:'ok',name:'ok',status:'synced',sourceId:'repo'},
    {id:'fail',name:'fail',status:'synced',sourceId:'repo',error:'cannot read'},
    {id:'wait',name:'wait',status:'synced',sourceId:'repo',remoteCheckRequired:true},
    {id:'gone',name:'gone',status:'synced',sourceId:'repo',remoteMissing:true},
    {id:'extra',name:'extra',status:'local_extra',untrackedFiles:['README.md']},
  ];
  assert.equal(displayStatus(items[0]).label,'内容一致');assert.equal(displayStatus(items[1]).key,'error');
  assert.equal(displayStatus(items[2]).key,'remote_check_required');assert.equal(displayStatus(items[3]).key,'remote_missing');
  assert.deepEqual(queryLibrary(items,{statusFilter:'synced'}).map(s=>s.id),['ok']);
  assert.deepEqual(queryLibrary(items,{statusFilter:'error'}).map(s=>s.id),['fail']);
  assert.deepEqual(queryLibrary(items,{statusFilter:'remote_missing'}).map(s=>s.id),['gone']);
  assert.deepEqual(queryLibrary(items,{statusFilter:'local_extra'}).map(s=>s.id),['extra']);
  assert(needsAttention(items[4]));assert.equal(queryLibrary(items,{statusFilter:'attention'}).length,4);
});
test('comparison explanation qualifies partial runtime, independent times, extras and baseline honestly', () => {
  const skill:LibraryItem={id:'a',name:'a',status:'synced',comparisonScope:['SKILL.md','scripts/run.py'],runtimeConfirmed:false,lastChecked:'2026-10-23T00:00:00Z',untrackedFiles:['notes.md']};
  const text=statusExplanation(skill);assert(text.includes('2 个文件'));assert(text.includes('待确认'));assert(text.includes('比较基准：未知'));assert(text.includes('最近远端检查：未知'));assert(text.includes('1 个本地文件'));
  assert.deepEqual(comparisonScope(skill),['SKILL.md','scripts/run.py']);
  assert.deepEqual(comparisonScope({id:'b',name:'b'}),[]);
  assert(statusExplanation({...skill,status:'local_extra'}).includes('尚不等于待上传修改'));
  assert(statusExplanation({...skill,status:'docs_updated'}).includes('运行比较内容一致'));
});
test('favorites are an independent view composed with category metadata, source and query', () => {
  const items:LibraryItem[]=[{id:'a',name:'paper',categoryId:'research',favorite:true,sourceId:'repo'},{id:'b',name:'tool',categoryId:'tools',favorite:true},{id:'c',name:'paper2',categoryId:'research'}];
  const before=structuredClone(items);
  assert.deepEqual(queryLibrary(items,{categoryFilter:'favorites'}).map(s=>s.id),['a','b']);
  assert.deepEqual(queryLibrary(items,{categoryFilter:'favorites',sourceFilter:'github',query:'paper'}).map(s=>s.id),['a']);
  assert.deepEqual(queryLibrary(items,{categoryFilter:'research'}).map(s=>s.id),['a','c']);assert.deepEqual(items,before);
});
test('drag and range selection are constrained to the current page and actual selected row', () => {
  const page=['a','b','c','d'];assert.deepEqual(selectionForDrag('a',['a','b','hidden'],page),['a','b']);
  assert.deepEqual(selectionForDrag('c',['a','b'],page),['c']);assert.deepEqual(selectionForDrag('hidden',['hidden'],page),[]);
  assert.deepEqual(rangeSelection(page,['a','hidden'],'b','d',true),['a','b','c','d']);
  assert.deepEqual(rangeSelection(page,['a','b','c','d'],'c','b',false),['a','d']);
  assert.deepEqual(rangeSelection(page,[],null,'c',true),['c']);
});
test('single categories compose with query, source and status before pagination without changing records', () => {
  const items: LibraryItem[] = [
    { id: 'a', name: 'paper-1', categoryId: 'research', localPath: 'D:/a', sourceId: 'repo', status: 'local_modified' },
    { id: 'b', name: 'paper-2', categoryId: 'tools', localPath: 'D:/b', status: 'local_modified' },
    { id: 'c', name: 'paper-3', categoryId: 'research', sourceId: 'repo', status: 'remote_only' },
    { id: 'd', name: 'paper-4' }, { id: 'e', name: 'paper-5', categoryId: null },
  ];
  const before = structuredClone(items);
  assert.deepEqual(queryLibrary(items, { categoryFilter: 'research' }).map(s => s.id), ['a', 'c']);
  assert.deepEqual(queryLibrary(items, { categoryFilter: 'research', query: 'paper', sourceFilter: 'linked', statusFilter: 'local_modified' }).map(s => s.id), ['a']);
  assert.deepEqual(queryLibrary(items, { categoryFilter: 'uncategorized' }).map(s => s.id), ['d', 'e']);
  assert.equal(queryLibrary(items, { categoryFilter: 'all' }).length, 5);
  assert.equal(queryLibrary(items, { categoryFilter: 'missing' }).length, 0);
  assert.equal(paginateLibrary(queryLibrary(items, { categoryFilter: 'research' }), 2, 1).items[0].id, 'c');
  assert.deepEqual(items, before);
});
test('searches the entire library using normalized cross-field AND terms before pagination', () => {
  const result = queryLibrary(collection, { query: 'ＰＤＦ 中文' });
  assert.deepEqual(result.map(s => s.id), ['501']);
  assert.equal(paginateLibrary(result, 1, 20).start, 1);
  assert.equal(queryLibrary(collection, { query: 'skill-502 missing' }).length, 0);
});
test('natural name sorting and pagination are stable and leave source records untouched', () => {
  const shuffled = [collection[9], collection[1], collection[0]];
  assert.deepEqual(queryLibrary(shuffled).map(s => s.name), ['skill-1', 'skill-2', 'skill-10']);
  assert.equal(shuffled[0].name, 'skill-10');
  const page = paginateLibrary(queryLibrary(collection), 26, 20);
  assert.equal(page.items.length, 17); assert.equal(page.start, 501); assert.equal(page.end, 517);
  assert.equal(page.items[0].name, 'skill-501');
});
test('source filters distinguish standalone, remote and associated entries', () => {
  const items: LibraryItem[] = [{ id: 'a', name: 'local', localPath: 'D:/a' }, { id: 'b', name: 'remote', sourceId: 'repo' }, { id: 'c', name: 'linked', sourceId: 'repo', localPath: 'D:/c', repo: 'owner/example' }];
  assert.deepEqual(queryLibrary(items, { sourceFilter: 'linked' }).map(s => s.id), ['c']);
  assert.equal(queryLibrary(items, { sourceFilter: 'github' }).length, 2);
  assert.deepEqual(queryLibrary(items, { query: 'OWNER/EXAMPLE', sourceFilter: 'local' }).map(s => s.id), ['c']);
});
test('cached synced labels do not hide errors, missing remotes or pending checks', () => {
  const items: LibraryItem[] = [{ id: 'a', name: 'a', status: 'synced', error: 'network unavailable' }, { id: 'b', name: 'b', status: 'synced', remoteCheckRequired: true }, { id: 'c', name: 'c', status: 'synced', remoteMissing: true }, { id: 'd', name: 'd', status: 'synced' }];
  assert.equal(queryLibrary(items, { statusFilter: 'attention' }).length, 3);
  assert.deepEqual(queryLibrary(items, { statusFilter: 'error' }).map(s => s.id), ['a']);
  assert.deepEqual(queryLibrary(items, { statusFilter: 'remote_missing' }).map(s => s.id), ['c']);
  assert(needsAttention({ id: 'e', name: 'e', untrackedFiles: ['custom.txt'] }));
});
test('status and checked sorting prioritize actionable entries and parse missing dates safely', () => {
  const items: LibraryItem[] = [{ id: 'a', name: 'a', status: 'synced', lastChecked: 'bad' }, { id: 'b', name: 'b', status: 'remote_updated', lastChecked: '2026-10-23T01:00:00Z' }, { id: 'c', name: 'c', status: 'both_modified', lastChecked: '2026-10-22T01:00:00Z' }];
  assert.deepEqual(queryLibrary(items, { sort: 'status' }).map(s => s.id), ['c', 'b', 'a']);
  assert.deepEqual(queryLibrary(items, { sort: 'checked' }).map(s => s.id), ['b', 'c', 'a']);
});
test('page bounds remain valid after filtering or removal and handle empty collections', () => {
  assert.equal(paginateLibrary(collection.slice(0, 3), 10, 20).page, 1);
  assert.equal(paginateLibrary(collection, -1, 20).page, 1);
  assert.deepEqual(paginateLibrary([], 99, 20), { items: [], total: 0, page: 1, pageCount: 1, start: 0, end: 0 });
  assert.equal(paginateLibrary(collection, NaN, NaN).items.length, 20);
});

test('summary separates GitHub Skill entries, repository sources and local links', () => {
  const remote: LibraryItem[] = Array.from({ length: 15 }, (_, n) => ({ id: `r${n}`, name: `remote-${n}`, sourceId: 'repo' }));
  const local: LibraryItem[] = Array.from({ length: 16 }, (_, n) => ({ id: `l${n}`, name: `local-${n}`, localPath: `D:/fixture/${n}` }));
  assert.deepEqual(libraryCounts([...local, ...remote], [{ id: 'repo' }]), { total: 31, local: 16, github: 15, linked: 0, repositories: 1 });
  const linked = local.map((s, n) => ({ ...s, sourceId: n < 15 ? 'repo' : undefined }));
  assert.deepEqual(libraryCounts(linked, [{ id: 'repo' }]), { total: 16, local: 16, github: 15, linked: 15, repositories: 1 });
  assert.equal(libraryCounts(linked, [{ id: 'main', repo: 'Example/Skills' }, { id: 'dev', repo: 'example/skills' }, { id: 'another', repo: 'example/another' }]).repositories, 2);
});

test('source folder URLs retain branch and exact encoded package path and reject unsafe origins', () => {
  assert.equal(githubFolderUrl({ repo: 'example/skills', branch: 'feature/runtime', remotePath: 'folder/中文 skill' }), 'https://github.com/example/skills/tree/feature%2Fruntime/folder/%E4%B8%AD%E6%96%87%20skill');
  assert.equal(githubFolderUrl({ repo: 'example/skills', commit: 'abc' }), 'https://github.com/example/skills/tree/abc');
  assert.equal(githubFolderUrl({ repo: 'example/skills' }), 'https://github.com/example/skills/tree/HEAD');
  assert.equal(githubFolderUrl({ repo: 'javascript:alert(1)' }), null);
  assert.equal(githubFolderUrl({ repo: 'example/skills', remotePath: '../secrets' }), null);
  assert.equal(githubFolderUrl({ repo: 'example/skills', remotePath: 'a\\b' }), null);
});

test('matching comparison evidence never invents historical installation evidence', () => {
  const text = statusExplanation({id:'evidence',name:'evidence',status:'synced',baselineCommit:'a'.repeat(40),baselineKind:'comparison',installedCommit:null});
  assert(text.includes('比较基准：已核实')); assert(text.includes('历史安装提交：未知')); assert(!text.includes('安装基准：已核实'));
});

test('historical and informational runtime records do not leak into the attention filter', () => {
  const skill: LibraryItem = { id: 'notes', name: 'notes', status: 'synced', runtimeWarnings: ['Old flat warning'], runtimeIssues: [{ category: 'history', severity: 'info', source: 'sources.md', target: 'old.md', reason: 'Historical record' }] };
  assert.equal(needsAttention(skill), false);
  assert.deepEqual(queryLibrary([skill], { statusFilter: 'attention' }), []);
  assert.equal(needsAttention({ ...skill, runtimeIssues: undefined }), true);
  assert.equal(needsAttention({ ...skill, runtimeIssues: [{ category: 'package_required', severity: 'error', source: 'SKILL.md', target: 'scripts/missing.py', reason: 'Missing' }] }), true);
});

test('six visible categories retain every raw status, direction and overlay precedence', () => {
  const expected: Record<string,string> = {local_only:'local_only',remote_only:'remote_only',synced:'synced',local_modified:'changed',remote_updated:'changed',both_modified:'changed',docs_updated:'changed',unknown_baseline:'review',local_extra:'review',error:'problem',remote_missing:'problem',remote_check_required:'review'};
  assert.equal(LIBRARY_STATUS_GROUPS.length, 6);
  for (const [status, group] of Object.entries(expected)) {
    const skill:LibraryItem = {id:status,name:status,status};
    const before = structuredClone(skill), visible = displayStatus(skill);
    assert.equal(visible.key,status); assert.equal(visible.group,group);
    assert.equal(visible.label,statusFilterLabel(group as Parameters<typeof statusFilterLabel>[0]));
    assert.deepEqual(skill,before);
  }
  const overlaid = displayStatus({id:'overlaid',name:'overlaid',status:'synced',remoteMissing:true,error:'offline',remoteCheckRequired:true});
  assert.equal(overlaid.key,'remote_missing'); assert.equal(overlaid.label,'检查异常');
  assert.equal(displayStatus({id:'local',name:'local',status:'local_modified'}).detail,'本地有修改');
  assert.equal(displayStatus({id:'remote',name:'remote',status:'remote_updated'}).detail,'远端有更新');
  assert.equal(displayStatus({id:'docs',name:'docs',status:'docs_updated'}).detail,'仅说明有更新');
});

test('group filters are additive while legacy subtype filters remain exact', () => {
  const items:LibraryItem[] = ['local_modified','remote_updated','both_modified','docs_updated','unknown_baseline','local_extra','synced'].map(status => ({id:status,name:status,status}));
  assert.equal(queryLibrary(items,{statusFilter:'changed'}).length,4);
  assert.deepEqual(queryLibrary(items,{statusFilter:'local_modified'}).map(item => item.id),['local_modified']);
  assert.deepEqual(queryLibrary(items,{statusFilter:'review'}).map(item => item.id),['local_extra','unknown_baseline']);
  const unknown:LibraryItem = {id:'unknown',name:'unknown',status:'future-status'};
  assert.equal(displayStatus(unknown).group,'review');
  assert.equal(displayStatus(unknown).detail,'状态未知');
  assert.equal(displayStatus(unknown).key,'future-status');
  assert.equal(needsAttention(unknown),false,'Unknown presentation must not silently broaden the existing attention rule');
  for (const status of ['constructor','toString','__proto__']) {
    const value = displayStatus({...unknown,status});
    assert.equal(value.key,status); assert.equal(value.group,'review'); assert.equal(value.detail,'状态未知');
  }
});

test('dynamic status counts use category, source and search before the selected status', () => {
  const items:LibraryItem[] = [
    {id:'changed',name:'paper one',categoryId:'research',sourceId:'repo',localPath:'D:/one',status:'local_modified'},
    {id:'same',name:'paper two',categoryId:'research',sourceId:'repo',localPath:'D:/two',status:'synced'},
    {id:'other-category',name:'paper three',categoryId:'tools',sourceId:'repo',localPath:'D:/three',status:'error'},
    {id:'other-source',name:'paper four',categoryId:'research',localPath:'D:/four',status:'local_only'},
    {id:'other-search',name:'tool five',categoryId:'research',sourceId:'repo',localPath:'D:/five',status:'remote_updated'},
  ];
  const before = structuredClone(items);
  const options = libraryStatusOptions(items,{categoryFilter:'research',sourceFilter:'linked',query:'ＰＡＰＥＲ',statusFilter:'review'});
  assert.deepEqual(options.map(option => [option.value,option.count]),[['all',2],['attention',1],['synced',1],['changed',1],['review',0]]);
  assert.equal(queryLibrary(items,{categoryFilter:'research',sourceFilter:'linked',query:'paper',statusFilter:'review'}).length,0);
  assert.deepEqual(items,before);
});

test('zero-result selections remain visible without permanently offering unavailable statuses', () => {
  assert.deepEqual(libraryStatusOptions([],{statusFilter:'problem'}).map(option => [option.value,option.count]),[['all',0],['problem',0]]);
  assert.deepEqual(libraryStatusOptions([],{statusFilter:'local_extra'}).map(option => [option.value,option.count]),[['all',0],['local_extra',0]]);
  const options = libraryStatusOptions([{id:'one',name:'one',status:'local_only'}]);
  assert.deepEqual(options.map(option => option.value),['all','local_only']);
});

test('unknown baselines with historical evidence never claim that installation records are absent', () => {
  const skill:LibraryItem = {id:'scope',name:'scope',status:'unknown_baseline',baselineCommit:'a'.repeat(40),installedCommit:'b'.repeat(40),changeDetails:[{path:'new.py',change:'different'}]};
  const explanation = statusExplanation(skill);
  assert(explanation.includes('已有比较基准'));
  assert(explanation.includes('历史安装提交：已记录'));
  assert(!explanation.includes('尚无安装基准'));
  assert(explanation.includes('不据此判断哪侧较新'));
});
