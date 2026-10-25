export type ShortcutHelpMode = 'list' | 'cards';
export type ShortcutHelpRow = { keys: readonly string[]; action: string };
export type ShortcutHelpRect = { left: number; top: number; right: number; bottom: number };

const sharedRows: readonly ShortcutHelpRow[] = [
  { keys: ['空格'], action: '勾选或取消当前项' },
  { keys: ['Shift＋空格', 'Shift＋点击'], action: '扩展勾选范围' },
  { keys: ['Ctrl＋A'], action: '选择本页全部' },
];

/** Mirrors tableKeys and LibraryCards; keyboard navigation does not open a Skill. */
export function shortcutHelpRows(mode: ShortcutHelpMode): readonly ShortcutHelpRow[] {
  return [
    { keys: mode === 'cards' ? ['↑', '↓', '←', '→'] : ['↑', '↓'], action: mode === 'cards' ? '移动卡片焦点' : '移动行焦点' },
    { keys: ['Home', 'End'], action: '移到本页首项或末项' },
    ...sharedRows,
    { keys: mode === 'cards' ? ['Shift＋方向键', 'Shift＋Home / End'] : ['Shift＋↑ / ↓', 'Shift＋Home / End'], action: '扩选到目标项' },
  ];
}

/** Inputs and output use the same logical pixels, including application zoom. */
export function shortcutHelpBounds(...rects: readonly ShortcutHelpRect[]): ShortcutHelpRect {
  return {
    left: Math.max(...rects.map(rect => rect.left)), top: Math.max(...rects.map(rect => rect.top)),
    right: Math.min(...rects.map(rect => rect.right)), bottom: Math.min(...rects.map(rect => rect.bottom)),
  };
}

export function shortcutHelpPlacement(anchor: ShortcutHelpRect, bounds: ShortcutHelpRect, preferredWidth: number, naturalHeight: number) {
  const margin = 8, gap = 6;
  const availableWidth = bounds.right - bounds.left - margin * 2;
  const availableHeight = bounds.bottom - bounds.top - margin * 2;
  if (availableWidth <= 0 || availableHeight <= 0 || anchor.right <= bounds.left || anchor.left >= bounds.right || anchor.bottom <= bounds.top || anchor.top >= bounds.bottom) return null;
  const width = Math.min(preferredWidth, availableWidth);
  const left = Math.max(bounds.left + margin, Math.min(anchor.right - width, bounds.right - width - margin));
  const below = Math.max(0, bounds.bottom - margin - anchor.bottom - gap);
  const above = Math.max(0, anchor.top - gap - bounds.top - margin);
  const side = naturalHeight <= below || below >= above ? 'below' : 'above';
  const maxHeight = Math.max(0, side === 'below' ? below : above);
  if (!maxHeight) return null;
  const height = Math.min(naturalHeight, maxHeight);
  const top = Math.max(bounds.top + margin, Math.min(side === 'below' ? anchor.bottom + gap : anchor.top - gap - height, bounds.bottom - height - margin));
  return { left, top, width, maxHeight, side };
}

/** Dismissal must never scroll or focus a hidden/detached page. */
export function restoreShortcutHelpFocus(trigger: Pick<HTMLElement, 'isConnected' | 'getClientRects' | 'focus'> | null) {
  if (trigger?.isConnected && trigger.getClientRects().length) trigger.focus({ preventScroll: true });
}
