type SnapshotClock = { stateRevision?: number; readGeneration?: number | null };

/** Transport generations advance independently of persisted state revisions. */
export function isStaleReadSnapshot(next: SnapshotClock, revision: number, generation?: number) {
  return next.stateRevision !== undefined && next.stateRevision < revision
    || typeof next.readGeneration === 'number' && generation !== undefined && next.readGeneration < generation;
}
