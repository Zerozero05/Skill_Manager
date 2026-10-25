import { uniqueRuntimeIssues, type RuntimeIssue } from './runtime.ts';

export type RuntimeReference = { source: string; target: string };
const location = (value: string) => value.replaceAll('\\', '/');

/** A scan record belongs to a reference only when both complete locations agree. */
export function sameRuntimeReference(issue: RuntimeReference, reference: RuntimeReference) {
  return location(issue.source) === location(reference.source) && location(issue.target) === location(reference.target);
}

export function runtimeIssuesForReference(issues: readonly RuntimeIssue[], reference: RuntimeReference) {
  return uniqueRuntimeIssues([...issues]).filter(issue => sameRuntimeReference(issue, reference));
}

export function unattachedRuntimeIssues(issues: readonly RuntimeIssue[], references: readonly RuntimeReference[]) {
  return uniqueRuntimeIssues([...issues]).filter(issue => !references.some(reference => sameRuntimeReference(issue, reference)));
}
