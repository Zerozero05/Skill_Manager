import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { isTauri, invoke } from '@tauri-apps/api/core';

export type TaskOrigin = { view?: string; skillId?: string; tab?: string; clientId?: string; sourceId?: string; paneId?: string; windowLabel?: string };
export type ManagerTask = { id: string; action: string; label: string; status: string; stage: string; createdAt: string; startedAt?: string; finishedAt?: string; origin: TaskOrigin; progress?: { completed: number; total: number }; cancellable: boolean; cancelRequested?: boolean; error?: string; resultAvailable?: boolean; resultExpired?: boolean; hidden?: boolean; result?: unknown };
export const terminalTask = (task: ManagerTask) => !['queued', 'running'].includes(task.status);
export const taskStatus: Record<string, string> = { queued: '排队中', running: '进行中', succeeded: '完成', partial: '部分完成', failed: '失败', cancelled: '已取消', interrupted: '已中断 · 需核实' };
const api = <T,>(action: string, args: Record<string, unknown> = {}) => invoke<T>('manager_request', { request: { action, ...args } });

export function useTasks() {
  const [tasks, setTasks] = useState<ManagerTask[]>([]);
  const mounted = useRef(true);
  const waiters = useRef(new Map<string, () => void>());
  const clearedIds = useRef(new Set<string>());
  const accept = useCallback((task: ManagerTask) => { waiters.current.get(task.id)?.(); if (mounted.current) setTasks(previous => {
    if (task.hidden || clearedIds.current.has(task.id)) return previous.filter(item => item.id !== task.id);
    const existing = previous.find(item => item.id === task.id);
    if (existing && terminalTask(existing) && !terminalTask(task)) return previous;
    return [task, ...previous.filter(item => item.id !== task.id)].sort((a, b) => b.createdAt.localeCompare(a.createdAt)).filter((item, index, all) => !terminalTask(item) || all.slice(0, index).filter(terminalTask).length < 100);
  }); }, []);
  const acceptClear = useCallback((value: { tasks?: ManagerTask[]; clearedTaskIds: string[] }) => {
    for (const id of value.clearedTaskIds) clearedIds.current.add(id);
    while (clearedIds.current.size > 2000) clearedIds.current.delete(clearedIds.current.values().next().value!);
    const current = value.tasks || [];
    if (mounted.current) setTasks(previous => [...current, ...previous.filter(task => !clearedIds.current.has(task.id) && !current.some(item => item.id === task.id))].sort((a, b) => b.createdAt.localeCompare(a.createdAt)));
  }, []);
  useEffect(() => {
    mounted.current = true;
    if (!isTauri()) return;
    let stop: (() => void) | undefined;
    let stopClear: (() => void) | undefined;
    void api<{ tasks: ManagerTask[] }>('list_tasks').then(value => { if (mounted.current) setTasks(previous => [...previous, ...value.tasks.filter(task => !task.hidden && !clearedIds.current.has(task.id) && !previous.some(item => item.id === task.id))].sort((a, b) => b.createdAt.localeCompare(a.createdAt))); }).catch(() => undefined);
    void listen<{ task: ManagerTask }>('task-updated', event => accept(event.payload.task)).then(unlisten => { if (mounted.current) stop = unlisten; else unlisten(); }).catch(() => undefined);
    void listen<{ tasks: ManagerTask[]; clearedTaskIds: string[] }>('task-history-cleared', event => acceptClear(event.payload)).then(unlisten => { if (mounted.current) stopClear = unlisten; else unlisten(); }).catch(() => undefined);
    return () => { mounted.current = false; stop?.(); stopClear?.(); };
  }, [accept, acceptClear]);
  const run = useCallback(async <T,>(label: string, action: string, args: Record<string, unknown>, origin: TaskOrigin, foreground = true, onStarted?: (task: ManagerTask) => void): Promise<T> => {
    const started = await api<{ task: ManagerTask }>('start_task', { request: { action, ...args }, label, origin, foreground });
    let task = started.task; accept(task); onStarted?.(task);
    while (!terminalTask(task)) {
      task = (await api<{ task: ManagerTask }>('get_task', { taskId: task.id })).task; accept(task);
      if (terminalTask(task)) break;
      await new Promise<void>(resolve => { const timer = setTimeout(() => { waiters.current.delete(task.id); resolve(); }, 700); waiters.current.set(task.id, () => { clearTimeout(timer); waiters.current.delete(task.id); resolve(); }); });
    }
    if (task.status === 'cancelled') throw new Error('任务已取消');
    if (task.status === 'failed' || task.status === 'interrupted') throw new Error(task.error || task.stage || '任务未完成');
    if (task.result === undefined) task = (await api<{ task: ManagerTask }>('get_task', { taskId: task.id })).task;
    return task.result as T;
  }, [accept]);
  const inspect = useCallback(async (id: string) => { const task = (await api<{ task: ManagerTask }>('get_task', { taskId: id })).task; accept(task); return task; }, [accept]);
  const cancel = useCallback(async (id: string) => { accept((await api<{ task: ManagerTask }>('cancel_task', { taskId: id })).task); }, [accept]);
  const clear = useCallback(async () => {
    const value = await api<{ tasks: ManagerTask[]; clearedTaskIds: string[] }>('clear_task_history');
    acceptClear(value);
  }, [acceptClear]);
  return { tasks, run, inspect, cancel, clear };
}
