import type { ManagerTask } from './tasks';

export const HISTORY_STATUS_FILTERS = ['all', 'active', 'ended', 'failed'] as const;
export type HistoryStatusFilter = typeof HISTORY_STATUS_FILTERS[number];
export type OperationHistoryDraft = { search: string; status: HistoryStatusFilter };

export function operationHistoryDraft(value: unknown): OperationHistoryDraft {
  const raw = value && typeof value === 'object' ? value as Record<string, unknown> : {};
  return { search: typeof raw.search === 'string' ? raw.search : '', status: HISTORY_STATUS_FILTERS.includes(raw.status as HistoryStatusFilter) ? raw.status as HistoryStatusFilter : 'all' };
}

const activeTask = (task: ManagerTask) => ['queued', 'running'].includes(task.status);
const statusText: Record<string, string> = { queued: '排队中', running: '进行中', succeeded: '完成', partial: '部分完成', failed: '失败', cancelled: '已取消', interrupted: '已中断' };

/** Views filter the shared records without sorting or mutating the task service's data. */
export function filterOperationHistory(tasks: readonly ManagerTask[], draft: OperationHistoryDraft): ManagerTask[] {
  const query = draft.search.trim().toLocaleLowerCase();
  return tasks.filter(task => {
    if (draft.status === 'active' && !activeTask(task)) return false;
    if (draft.status === 'ended' && activeTask(task)) return false;
    if (draft.status === 'failed' && !['failed', 'interrupted', 'partial'].includes(task.status)) return false;
    return !query || [task.label, task.stage, statusText[task.status] || task.status, task.error, task.createdAt].some(value => value?.toLocaleLowerCase().includes(query));
  });
}

/** The small sidebar view shares records, but never changes their service order. */
export function operationSummaryTasks(tasks: readonly ManagerTask[], limit = 5): ManagerTask[] {
  return tasks.filter(task => !task.hidden).sort((left, right) =>
    Number(activeTask(right)) - Number(activeTask(left)) || right.createdAt.localeCompare(left.createdAt) || left.id.localeCompare(right.id)
  ).slice(0, Math.max(0, limit));
}

type SummaryRect = { left: number; top: number; right: number; bottom: number };

/** All inputs are logical pixels. This panel can shrink, but never flips over the sidebar. */
export function operationSummaryPlacement(anchor: SummaryRect, bounds: SummaryRect, height: number) {
  const gap = 8, left = Math.max(bounds.left + gap, anchor.right + gap);
  const width = Math.max(0, Math.min(360, bounds.right - left - gap));
  const maxHeight = Math.max(0, bounds.bottom - bounds.top - gap * 2);
  const actualHeight = Math.min(Math.max(0, height), maxHeight);
  const top = Math.max(bounds.top + gap, Math.min(anchor.bottom - actualHeight, bounds.bottom - actualHeight - gap));
  return { left, top, width, maxHeight, compact: width < 220 || maxHeight < 200 };
}
