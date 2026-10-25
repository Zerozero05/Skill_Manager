export type RuntimeIssue = {
  category: 'package_required' | 'external' | 'history' | 'uncertain';
  severity: 'error' | 'warning' | 'info';
  source: string;
  target: string;
  reason: string;
};

export function uniqueRuntimeIssues(issues: RuntimeIssue[]) {
  const seen = new Set<string>();
  return issues.filter(issue => {
    const key = JSON.stringify([issue.category, issue.severity, issue.source, issue.target, issue.reason]);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

export function runtimeAttentionCount(issues?: RuntimeIssue[]) {
  return uniqueRuntimeIssues(issues || []).filter(issue =>
    issue.severity !== 'info' && (issue.category === 'package_required' || issue.category === 'uncertain')
  ).length;
}

export function requiredExportPaths(result: { exportRequiredFiles?: string[]; requiredFiles?: string[]; files: { path: string }[] }) {
  const available = new Set(result.files.map(file => file.path));
  return [...new Set(result.exportRequiredFiles ?? result.requiredFiles ?? [])].filter(path => available.has(path));
}

/** The ZIP writer deduplicates resource paths; previews count files, not references. */
export function exportResourceCounts(required: string[], extras: string[], files: { path: string; localAvailable: boolean }[], references: { id: string; packagePath?: string | null; required: boolean; effectiveSource?: string | null }[], selectedIds: string[]) {
  const regular = new Set([...required, ...extras]);
  const sources = new Map<string, boolean>();
  for (const path of regular) sources.set(path, !files.some(file => file.path === path && file.localAvailable));
  const externalRequired = new Set<string>();
  for (const item of references) {
    if (!item.required && !selectedIds.includes(item.id)) continue;
    const path = item.packagePath || `reference:${item.id}`;
    if (item.required && !regular.has(path)) externalRequired.add(path);
    if (!regular.has(path)) sources.set(path, (sources.get(path) ?? true) && item.effectiveSource === 'github');
  }
  return { externalRequired: externalRequired.size, remoteCount: [...sources.values()].filter(Boolean).length };
}
