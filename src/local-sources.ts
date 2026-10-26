export type LocalRoot = { id: string; name: string; path: string; resolvedPath?: string; enabled: boolean; error?: string | null };
export type LocalCandidate = { id?: string; name: string; displayName?: string; description?: string; localPath?: string | null; path?: string; readOnly?: boolean };
export type LocalImportDraft = { paths: string[]; input: string; candidates?: LocalCandidate[]; selected: string[]; query: string; automatic: boolean; warnings?: string[] };
export const localPathKey = (path: string) => path.replace(/^\\\\\?\\/, '').replaceAll('\\', '/').replace(/\/+$/, '').toLocaleLowerCase('en-US');
export const candidatePath = (item: LocalCandidate) => item.localPath || item.path || '';
export function uniqueCandidates(items: readonly LocalCandidate[]) { const result = new Map<string, LocalCandidate>(); for (const item of items) { const path = candidatePath(item); if (path) result.set(localPathKey(path), item); } return [...result.values()]; }
export function localCandidates(items: readonly LocalCandidate[], query: string) { const term = query.trim().toLocaleLowerCase(); return items.filter(item => !term || [item.name, item.displayName, item.description, candidatePath(item)].some(value => value?.toLocaleLowerCase().includes(term))); }
export function rangePaths(paths: readonly string[], selected: readonly string[], path: string, anchor: string | null, checked: boolean, shift: boolean) {
  const at = paths.indexOf(path), start = anchor ? paths.indexOf(anchor) : -1;
  const affected = shift && at >= 0 && start >= 0 ? paths.slice(Math.min(start, at), Math.max(start, at) + 1) : [path];
  const next = new Set(selected); for (const value of affected) checked ? next.add(value) : next.delete(value); return [...next];
}
