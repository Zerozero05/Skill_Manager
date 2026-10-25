import test from 'node:test';
import assert from 'node:assert/strict';
import { destinationAfterPicker, destinationDraft, destinationReady } from '../src/destination.ts';

test('each new operation starts undecided and transferred destination drafts remain independent', () => {
  assert.deepEqual(destinationDraft(), {mode:null,path:''});
  const first = destinationDraft({mode:'custom',path:'D:/chosen/skill.zip'}), second = destinationDraft();
  assert(destinationReady(first,'')); assert(!destinationReady(second,'D:/defaults/skill.zip'));
  assert.deepEqual(destinationDraft(structuredClone(first)),first);
  assert.deepEqual(destinationDraft({mode:'other',path:'D:/not-authorized'}),{mode:null,path:'D:/not-authorized'});
});
test('cancelling a picker preserves a prior choice and cannot silently choose a default', () => {
  const previous = destinationDraft();
  assert.equal(destinationAfterPicker(previous,null),previous);
  assert(!destinationReady(destinationAfterPicker(previous,null),'D:/defaults/skill.zip'));
  const chosen = destinationAfterPicker(previous,'D:/chosen/skill.zip');
  assert.deepEqual(chosen,{mode:'custom',path:'D:/chosen/skill.zip'});
  assert.equal(destinationAfterPicker(chosen,undefined),chosen);
});
test('default path changes require a fresh explicit selection while custom paths are stable', () => {
  assert(destinationReady({mode:'default',path:'D:/one/skill.zip'},'D:/one/skill.zip'));
  assert(!destinationReady({mode:'default',path:'D:/one/skill.zip'},'D:/two/skill.zip'));
  assert(!destinationReady({mode:'default',path:'D:/one/skill.zip'},''));
  assert(destinationReady({mode:'custom',path:'D:/chosen/skill.zip'},'D:/two/skill.zip'));
  assert(!destinationReady({mode:'custom',path:'  '},'D:/two/skill.zip'));
});
