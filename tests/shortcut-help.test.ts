import test from 'node:test';
import assert from 'node:assert/strict';
import { restoreShortcutHelpFocus, shortcutHelpBounds, shortcutHelpPlacement, shortcutHelpRows } from '../src/shortcut-help.ts';

test('help is constrained to the intersection of the current main area, pane and viewport', () => {
  const bounds = shortcutHelpBounds({ left: 224, top: 40, right: 1280, bottom: 900 }, { left: 0, top: 50, right: 700, bottom: 920 }, { left: 0, top: 0, right: 1280, bottom: 800 });
  assert.deepEqual(bounds, { left: 224, top: 50, right: 700, bottom: 800 });
  const value = shortcutHelpPlacement({ left: 620, top: 100, right: 680, bottom: 130 }, bounds, 352, 300)!;
  assert.deepEqual(value, { left: 328, top: 136, width: 352, maxHeight: 656, side: 'below' });
});

test('right alignment clamps at both edges without borrowing sidebar or adjacent pane space', () => {
  const bounds = { left: 320, top: 40, right: 560, bottom: 700 };
  const left = shortcutHelpPlacement({ left: 330, top: 80, right: 390, bottom: 110 }, bounds, 484, 280)!;
  const right = shortcutHelpPlacement({ left: 500, top: 80, right: 570, bottom: 110 }, bounds, 484, 280)!;
  assert.equal(left.width, 224); assert.equal(left.left, 328);
  assert.equal(right.width, 224); assert.equal(right.left, 328);
  assert(left.left + left.width <= bounds.right - 8);
});

test('help flips above near the bottom and uses a scrollable height when neither side fits', () => {
  const bounds = { left: 224, top: 40, right: 1000, bottom: 700 };
  const above = shortcutHelpPlacement({ left: 800, top: 630, right: 900, bottom: 660 }, bounds, 352, 300)!;
  assert.equal(above.side, 'above'); assert.equal(above.top, 324); assert.equal(above.maxHeight, 576);
  const short = shortcutHelpPlacement({ left: 800, top: 330, right: 900, bottom: 360 }, bounds, 352, 900)!;
  assert.equal(short.side, 'below'); assert.equal(short.top, 366); assert.equal(short.maxHeight, 326);
  assert(short.top + short.maxHeight <= bounds.bottom - 8);
});

test('offscreen anchors and collapsed visible intersections never produce an orphaned popup', () => {
  const bounds = { left: 224, top: 40, right: 700, bottom: 700 };
  for (const anchor of [{ left: 20, top: 50, right: 100, bottom: 90 }, { left: 700, top: 50, right: 760, bottom: 90 }, { left: 400, top: 0, right: 460, bottom: 40 }, { left: 400, top: 700, right: 460, bottom: 730 }]) assert.equal(shortcutHelpPlacement(anchor, bounds, 352, 300), null);
  assert.equal(shortcutHelpPlacement({ left: 400, top: 50, right: 460, bottom: 90 }, { ...bounds, right: 230 }, 352, 300), null);
});

test('logical geometry scales with fonts while respecting a very narrow pane', () => {
  const value = shortcutHelpPlacement({ left: 50, top: 80, right: 110, bottom: 110 }, { left: 40, top: 30, right: 140, bottom: 310 }, 22 * 22, 500)!;
  assert.equal(value.width, 84); assert.equal(value.left, 48); assert.equal(value.maxHeight, 186);
});

test('Escape restores only a connected visible trigger and always prevents reveal scrolling', () => {
  const focused: unknown[] = [];
  const trigger = { isConnected: true, getClientRects: () => [{ left: 20 }], focus: (options: unknown) => focused.push(options) };
  restoreShortcutHelpFocus(trigger as unknown as HTMLElement);
  assert.deepEqual(focused, [{ preventScroll: true }]);
  restoreShortcutHelpFocus({ ...trigger, isConnected: false } as unknown as HTMLElement);
  restoreShortcutHelpFocus({ ...trigger, getClientRects: () => [] } as unknown as HTMLElement);
  restoreShortcutHelpFocus(null);
  assert.equal(focused.length, 1);
});

test('list and card help describe their actual navigation keys without promising Enter to open a row', () => {
  const list = shortcutHelpRows('list'), cards = shortcutHelpRows('cards');
  assert.deepEqual(list[0].keys, ['↑', '↓']); assert.deepEqual(cards[0].keys, ['↑', '↓', '←', '→']);
  for (const rows of [list, cards]) {
    assert(rows.some(row => row.keys.includes('Ctrl＋A') && row.action === '选择本页全部'));
    assert(rows.some(row => row.keys.includes('空格')));
    assert(!rows.some(row => row.keys.includes('Enter')));
  }
});
