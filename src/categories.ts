export const CATEGORY_COLORS = [
  { value: 'blue', label: '蓝色' }, { value: 'teal', label: '青色' },
  { value: 'green', label: '绿色' }, { value: 'purple', label: '紫色' },
  { value: 'orange', label: '橙色' }, { value: 'rose', label: '玫红' },
  { value: 'indigo', label: '靛蓝' }, { value: 'slate', label: '灰色' },
] as const;
export type CategoryColor = typeof CATEGORY_COLORS[number]['value'];
export type Category = { id: string; name: string; color?: string };
export function isCategoryColor(value: unknown): value is CategoryColor {
  return CATEGORY_COLORS.some(color => color.value === value);
}
export function nextCategoryColor(used: Iterable<CategoryColor>): CategoryColor {
  const entries = [...used], values = new Set(entries);
  return CATEGORY_COLORS.find(color => !values.has(color.value))?.value || CATEGORY_COLORS[entries.length % CATEGORY_COLORS.length].value;
}
/** Legacy defaults are derived from identity, never labels/order, and cause no save. */
export function categoryColors(categories: readonly Category[]): Map<string, CategoryColor> {
  const result = new Map<string, CategoryColor>();
  for (const category of categories) if (isCategoryColor(category.color)) result.set(category.id, category.color);
  const used = new Set(result.values());
  let overflow = 0;
  for (const category of [...categories].sort((a, b) => a.id < b.id ? -1 : a.id > b.id ? 1 : 0)) {
    if (result.has(category.id)) continue;
    const unused = CATEGORY_COLORS.find(color => !used.has(color.value));
    const color = unused?.value || CATEGORY_COLORS[overflow++ % CATEGORY_COLORS.length].value;
    result.set(category.id, color); used.add(color);
  }
  return result;
}
export function categoryColor(category: Category | undefined, resolved: ReadonlyMap<string, CategoryColor>): CategoryColor | undefined {
  return category ? resolved.get(category.id) : undefined;
}
