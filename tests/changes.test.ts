import assert from 'node:assert/strict';
import test from 'node:test';
import { changePrimaryAction, changeReason, changesGroup, changesSummary, displayStatus, needsAttention, paginateLibrary, queryChanges, type LibraryItem } from '../src/library.ts';

const item = (id:string, extra:Partial<LibraryItem> = {}):LibraryItem => ({id,name:id,localPath:`D:/synthetic/${id}`,sourceId:'source',status:'synced',...extra});

test('Changes categories partition the existing attention membership with no missing or double-counted Skill', () => {
  const skills = [
    item('remote',{status:'remote_updated'}), item('docs',{status:'docs_updated'}),
    item('pending',{remoteCheckRequired:true}), item('extra',{status:'local_extra',untrackedFiles:['notes.md']}),
    item('error',{error:'offline'}), item('missing',{remoteMissing:true,error:'offline'}),
    item('runtime',{runtimeIssues:[{category:'package_required',severity:'error',source:'SKILL.md',target:'run.py',reason:'Missing dependency'}]}),
    item('notice',{status:'local_only',localNotice:'An extra local file needs a decision'}),
    item('same'), item('unknown',{status:'future-status'}),
    item('history',{runtimeIssues:[{category:'history',severity:'info',source:'sources.md',target:'old.md',reason:'Historical only'}]}),
  ];
  const before = structuredClone(skills);
  assert.deepEqual(changesSummary(skills),{total:8,changed:2,review:4,problem:2});
  const all = queryChanges(skills), partitions = ['changed','review','problem'].flatMap(group => queryChanges(skills,{group:group as 'changed'|'review'|'problem'}));
  assert.deepEqual(all.map(skill => skill.id).sort(),skills.filter(needsAttention).map(skill => skill.id).sort());
  assert.deepEqual(partitions.map(skill => skill.id).sort(),all.map(skill => skill.id).sort());
  assert.equal(new Set(partitions.map(skill => skill.id)).size,all.length);
  assert.equal(changesGroup(skills.find(skill => skill.id === 'runtime')!), 'review');
  assert.equal(displayStatus(skills.find(skill => skill.id === 'runtime')!).key,'synced','Runtime attention does not rewrite content comparison');
  assert.deepEqual(skills,before);
});

test('Changes search, category totals and pagination use the same matching members', () => {
  const skills = Array.from({length:47},(_,index) => item(`paper-${index+1}`,{status:index%2 ? 'local_extra' : 'remote_updated'}));
  const matching = queryChanges(skills,{query:'paper'}), counts = changesSummary(matching);
  assert.equal(counts.total,47); assert.equal(counts.changed+counts.review+counts.problem,47);
  const page = paginateLibrary(matching,3,20);
  assert.equal(page.items.length,7); assert.equal(page.start,41); assert.equal(page.end,47);
  const one = queryChanges(skills,{query:'paper-47'});
  assert.equal(changesSummary(one).total,1);
  assert.equal(paginateLibrary(one,3,20).page,1);
});

test('primary actions preserve docs-only, extra-file, comparison and retry distinctions', () => {
  assert.deepEqual(changePrimaryAction(item('docs',{status:'docs_updated'})),{kind:'docs',label:'查看说明'});
  assert.deepEqual(changePrimaryAction(item('extra',{status:'local_extra'})),{kind:'files',label:'查看文件'});
  assert.deepEqual(changePrimaryAction(item('unknown',{status:'unknown_baseline'})),{kind:'open',label:'核对依据'});
  assert.deepEqual(changePrimaryAction(item('remote',{status:'remote_updated'})),{kind:'compare',label:'预览更新'});
  assert.equal(changePrimaryAction(item('both',{status:'both_modified'})).kind,'compare');
  assert.equal(changePrimaryAction(item('error',{status:'docs_updated',error:'offline'})).kind,'check');
  assert.equal(changePrimaryAction(item('pending',{status:'local_modified',remoteCheckRequired:true})).kind,'check');
  assert.equal(changePrimaryAction(item('runtime',{runtimeIssues:[{category:'uncertain',severity:'warning',source:'SKILL.md',target:'file',reason:'Uncertain'}]})).kind,'files');
});

test('reasons use effective overlays and do not turn unknown comparison direction into missing installation history', () => {
  const historical = item('historical',{status:'unknown_baseline',baselineCommit:'a'.repeat(40),installedCommit:'b'.repeat(40)});
  assert(changeReason(historical).includes('已有比较基准'));
  assert(!changeReason(historical).includes('尚无安装'));
  assert(changeReason(historical).includes('不判断哪侧较新'));
  assert(changeReason(item('docs',{status:'docs_updated'})).includes('无需重装'));
  assert(changeReason(item('extra',{status:'local_extra'})).includes('尚不等于待上传修改'));
  assert.equal(changeReason(item('error',{status:'both_modified',error:'actual network failure'})),'actual network failure');
  assert(changeReason(item('missing',{status:'synced',remoteMissing:true})).includes('本地文件继续保留'));
});
