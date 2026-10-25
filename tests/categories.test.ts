import test from 'node:test';
import assert from 'node:assert/strict';
import { CATEGORY_COLORS, categoryColors, categoryColor, isCategoryColor, nextCategoryColor } from '../src/categories.ts';

test('legacy category colors are distinct and read-only, independent of displayed order or names', () => {
  const categories = ['e','c','a','b','d'].map((id, index) => ({ id, name: `分类 ${index}`, future: { keep: true } }));
  const before = structuredClone(categories), colors = categoryColors(categories);
  assert.equal(new Set(colors.values()).size, 5);
  assert.deepEqual(categories, before);
  assert.deepEqual([...categoryColors([...categories].reverse())].sort(), [...colors].sort());
  assert.deepEqual([...categoryColors(categories.map(item => ({ ...item, name: `改名 ${item.name}` })))].sort(), [...colors].sort());
  assert.equal(categoryColor(categories[0], colors), 'orange');
  assert.equal(categoryColor(undefined, colors), undefined);
});

test('saved colors are retained, defaults avoid them, unknown values stay untouched and overflow is deterministic', () => {
  const categories = [{id:'a',name:'A',color:'green'},{id:'b',name:'B'},{id:'c',name:'C',color:'future-color'},...Array.from({length:9},(_, n) => ({id:`d${n}`,name:`D${n}`}))];
  const before = structuredClone(categories), colors = categoryColors(categories);
  assert.equal(colors.get('a'),'green'); assert.equal(colors.get('b'),'blue'); assert.equal(colors.get('c'),'teal');
  assert([...colors.values()].every(isCategoryColor));
  assert.deepEqual(categories,before);
  assert.deepEqual([...categoryColors(categories)], [...colors]);
  assert.equal(nextCategoryColor([]),'blue');
  assert.equal(nextCategoryColor(['blue','green']),'teal');
  assert.equal(nextCategoryColor([...CATEGORY_COLORS.map(color => color.value),'blue']),'teal');
  for (const value of [undefined, null, 'future-color', '__proto__', 1]) assert.equal(isCategoryColor(value),false);
});
