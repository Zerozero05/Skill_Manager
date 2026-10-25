export type InventoryFile = { path: string; localAvailable?: boolean; remoteAvailable?: boolean; missing?: boolean };
export type InventoryIdentity = { localPath?: string | null; repo?: string | null; remotePath?: string | null; remoteCommit?: string | null };
type ReferenceIdentity = {
  id: string; canonicalPath?: string | null; kind?: string | null; referenceKind?: string;
  required: boolean; bindingStale: boolean; status: string; readable: boolean;
  effectiveSource?: string | null;
  local?: { status: string }; remote?: { status: string; repo?: string; commit?: string; repositoryPath?: string };
};

function relativePath(value: string) {
  const path = value.replaceAll('\\', '/');
  return !path.startsWith('/') && !/^[a-z]:/i.test(path) && path.split('/').every(part => part && part !== '.' && part !== '..') ? path : undefined;
}
function localPath(value?: string | null) {
  if (!value) return undefined;
  const path = value.replaceAll('\\', '/').replace(/^\/\/\?\/UNC\//i, '//').replace(/^\/\/\?\//, '').replace(/\/+$/, '');
  if (!/^(?:[a-z]:\/|\/\/)/i.test(path) || path.split('/').some(part => part === '.' || part === '..')) return undefined;
  return path.toLocaleLowerCase('en-US');
}
function repository(value?: string | null) { return value?.trim().replace(/\.git$/i, '').toLowerCase(); }
function fixedCommit(value?: string | null) { return value && /^[a-f0-9]{40}$/i.test(value) ? value.toLowerCase() : undefined; }
function remotePath(identity: InventoryIdentity, path: string) { return [identity.remotePath?.replace(/^\/+|\/+$/g, ''), path].filter(Boolean).join('/'); }

/** Derive presentation only. Reference identities, roles and export selections remain independent. */
export function groupExternalReferences<T extends ReferenceIdentity>(items: readonly T[], files: readonly InventoryFile[], identity: InventoryIdentity) {
  const byFile = new Map<string, T[]>(), standalone: T[] = [], fileByReference = new Map<string, string>();
  const root = localPath(identity.localPath), repo = repository(identity.repo), commit = fixedCommit(identity.remoteCommit);
  for (const item of items) {
    if (['command', 'skill_alias', 'directory'].includes(item.referenceKind || '') || item.kind === 'directory') { standalone.push(item); continue; }
    const canonical = localPath(item.canonicalPath), remoteCommit = fixedCommit(item.remote?.commit);
    const matches = files.filter(file => {
      const path = relativePath(file.path); if (!path) return false;
      const local = !!root && !!canonical && file.localAvailable === true && !file.missing && (item.kind === 'file' || item.referenceKind === 'package_file') && canonical === `${root}/${path.toLocaleLowerCase('en-US')}`;
      const remote = !!repo && !!commit && file.remoteAvailable === true && item.remote?.status === 'available' && repository(item.remote.repo) === repo && remoteCommit === commit && item.remote.repositoryPath === remotePath(identity, path);
      // A remote counterpart cannot establish the identity of different local bytes selected for export.
      return local || remote && !(item.effectiveSource === 'local' && canonical && (!root || canonical !== `${root}/${path.toLocaleLowerCase('en-US')}`));
    });
    // Conflicting local/remote identities and ambiguous inventory paths stay explicit.
    if (matches.length !== 1) { standalone.push(item); continue; }
    const path = matches[0].path;
    byFile.set(path, [...(byFile.get(path) || []), item]); fileByReference.set(item.id, path);
  }
  return { byFile, standalone, fileByReference };
}

export function referencePresentationIssues(item: ReferenceIdentity, file?: InventoryFile, identity?: InventoryIdentity) {
  const issues: string[] = [];
  if (item.bindingStale) issues.push('引用或绑定已变化');
  if (['missing', 'inaccessible', 'needs_selection', 'needs_review', 'unsupported'].includes(item.status)) issues.push('需要处理来源');
  if (item.remote?.status === 'error') issues.push('GitHub 读取失败');
  if (['needs_selection', 'needs_review'].includes(item.remote?.status || '')) issues.push('GitHub 来源需要核对');
  if (file && identity && item.canonicalPath && item.effectiveSource === 'local') {
    const root = localPath(identity.localPath), canonical = localPath(item.canonicalPath);
    if (!root || canonical !== `${root}/${file.path.toLocaleLowerCase('en-US')}`) issues.push('本地来源与清单不同');
  }
  if (file && identity && item.remote?.repositoryPath) {
    if (repository(item.remote.repo) !== repository(identity.repo) || item.remote.repositoryPath !== remotePath(identity, file.path) || fixedCommit(item.remote.commit) !== fixedCommit(identity.remoteCommit)) issues.push('远端来源与清单不同');
  }
  return [...new Set(issues)];
}
