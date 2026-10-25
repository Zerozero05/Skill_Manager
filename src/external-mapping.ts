export type MappingDraft = { id: string; source: 'local' | 'github'; path: string; repositoryPath: string; remoteCommit: string };
type MappingReference = {
  id: string; referenceKind?: string; effectiveSource?: string | null; readable: boolean; canonicalPath?: string | null;
  remote?: { repositoryPath?: string; commit?: string }; candidates?: { exact: boolean; repositoryPath: string; commit: string }[];
};

/** A deliberate GitHub button always opens that source, even when local bytes win export. */
export function buildExternalMappings(rows: readonly MappingReference[], source?: 'github'): MappingDraft[] {
  return rows.filter(item => !['command', 'skill_alias'].includes(item.referenceKind || '')).map(item => {
    const exact = item.candidates?.filter(candidate => candidate.exact) || [];
    return {
      id: item.id,
      source: source || (item.effectiveSource === 'github' || (!item.readable && exact.length === 1) ? 'github' : 'local'),
      path: item.canonicalPath || '',
      repositoryPath: item.remote?.repositoryPath || (exact.length === 1 ? exact[0].repositoryPath : ''),
      remoteCommit: item.remote?.commit || exact[0]?.commit || '',
    };
  });
}
