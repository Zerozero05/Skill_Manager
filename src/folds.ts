/** Fold state is navigation only. Stable keys survive heading text and count changes. */
const groupLabels: Record<string, readonly string[]> = {
  'files-required': ['必要文件', '必要运行文件'],
  'files-optional-package': ['可选包内文件', '可选说明与附加文件'],
  'files-optional-external': ['可选外部文件'],
  environment: ['环境引用', '环境要求', '环境与参考信息', '环境与参考'],
};

type SectionFold = { node: HTMLDetailsElement; key?: string; label: string };

function sections(root: HTMLElement): SectionFold[] {
  return Array.from(root.querySelectorAll<HTMLDetailsElement>('details')).map(node => {
    const summary = node.querySelector<HTMLElement>(':scope > summary');
    const key = node.dataset.foldKey || summary?.dataset.foldKey || node.dataset.outlineKey || summary?.dataset.outlineKey;
    const label = summary?.dataset.outlineLabel || summary?.textContent?.trim() || '';
    return { node, key, label };
  });
}

function uniqueSections(folds: readonly SectionFold[]): Map<string, number> {
  const counts = new Map<string, number>();
  for (const fold of folds) {
    const identity = fold.key ? `key:${fold.key}` : fold.label ? `label:${fold.label}` : '';
    if (identity) counts.set(identity, (counts.get(identity) || 0) + 1);
  }
  return counts;
}

/** Old repeated reference captions never identify a particular reference or file. */
function ambiguousReferenceLabel(label: string): boolean {
  return /^(引用\s*\d+\s*处|管理此处引用|管理来源与用途|来源与核验详情)(?:\s|\d|项|文件|必要|可选|$)/.test(label);
}

export function captureSectionFolds(root: HTMLElement): string[] {
  const folds = sections(root); const counts = uniqueSections(folds);
  return folds.flatMap(({ node, key, label }) => {
    const identity = key ? `key:${key}` : `label:${label}`;
    if (!label && !key || counts.get(identity) !== 1 || !key && ambiguousReferenceLabel(label)) return [];
    return [`${node.open ? 'open' : 'closed'}:${key ? `key:${key}` : label}`];
  });
}

/** Restore each identity once, so late content can restore without undoing user toggles. */
export function restoreSectionFolds(root: HTMLElement, initial: readonly string[], restoredSet: Set<string>): void {
  const folds = sections(root); const identities = uniqueSections(folds);
  const labels = new Map<string, number>();
  for (const { label } of folds) if (label) labels.set(label, (labels.get(label) || 0) + 1);
  const persisted = new Set(initial);
  const stateFor = (name: string): boolean | undefined => {
    const opened = persisted.has(`open:${name}`), closed = persisted.has(`closed:${name}`);
    return opened === closed ? undefined : opened;
  };
  for (const { node, key, label } of folds) {
    const identity = key ? `key:${key}` : `label:${label}`;
    if (restoredSet.has(identity) || identities.get(identity) !== 1) continue;
    let state = key ? stateFor(`key:${key}`) : undefined;
    const hasKeyState = !!key && (persisted.has(`open:key:${key}`) || persisted.has(`closed:key:${key}`));
    if (!hasKeyState) {
      const aliases = key && groupLabels[key] ? groupLabels[key] : [label];
      // All aliases must point at this one section; aliases shared by sibling sections
      // cannot safely map a legacy label to either of them.
      if (!aliases.some(alias => ambiguousReferenceLabel(alias)) && aliases.reduce((count, alias) => count + (labels.get(alias) || 0), 0) === 1) {
        const legacy = aliases.map(alias => stateFor(alias)).filter((value): value is boolean => value !== undefined);
        if (legacy.length && legacy.every(value => value === legacy[0])) state = legacy[0];
      }
    }
    if (state !== undefined) { node.open = state; restoredSet.add(identity); }
  }
}
