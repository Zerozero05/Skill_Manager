import { useState } from 'react';
import { Search } from 'lucide-react';
import { terminalTask, taskStatus, type ManagerTask } from './tasks';
import { filterOperationHistory, operationHistoryDraft, type OperationHistoryDraft } from './operation-history';
import './operation-history.css';

export type { OperationHistoryDraft } from './operation-history';
export type OperationHistoryProps = {
  tasks: readonly ManagerTask[];
  initialDraft?: Partial<OperationHistoryDraft>;
  onDraftChange: (draft: OperationHistoryDraft) => void;
  onInspect: (task: ManagerTask) => void | Promise<void>;
  onLocate: (task: ManagerTask) => void | Promise<void>;
  onCancel: (task: ManagerTask) => void | Promise<void>;
  onClear: () => void | Promise<void>;
};

const dateText = (value: string) => { const date = new Date(value); return Number.isNaN(date.getTime()) ? value : date.toLocaleString('zh-CN', { hour12: false }); };
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

/** The enclosing PaneFrame owns scrolling and the window owns the shared task service. */
export function OperationHistory({ tasks, initialDraft, onDraftChange, onInspect, onLocate, onCancel, onClear }: OperationHistoryProps) {
  const [draft, setDraft] = useState(() => operationHistoryDraft(initialDraft));
  const [error, setError] = useState('');
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const activeCount = tasks.filter(task => !terminalTask(task)).length;
  const endedCount = tasks.length - activeCount;
  const filtered = filterOperationHistory(tasks, draft);
  const updateDraft = (patch: Partial<OperationHistoryDraft>) => { const next = { ...draft, ...patch }; setDraft(next); onDraftChange(next); };
  const perform = async (key: string, action: () => void | Promise<void>) => {
    if (pending.has(key)) return;
    setError(''); setPending(previous => new Set(previous).add(key));
    try { await action(); }
    catch (cause) { setError(errorText(cause)); }
    finally { setPending(previous => { const next = new Set(previous); next.delete(key); return next; }); }
  };
  return <section className="operation-history" aria-label="操作记录">
    <header className="operation-history-heading">
      <div><h1>操作记录</h1><p className="muted">{tasks.length} 条记录{activeCount > 0 && ` · ${activeCount} 项进行中`}</p></div>
      <button type="button" className="button" disabled={!endedCount || pending.has('clear')} onClick={() => void perform('clear', onClear)}>清空已结束记录</button>
    </header>
    <div className="operation-history-toolbar">
      <label className="operation-history-search"><Search size={16} aria-hidden="true" /><input aria-label="搜索操作记录" placeholder="搜索操作、阶段或错误" value={draft.search} onChange={event => updateDraft({ search: event.target.value })} /></label>
      <label className="operation-history-filter"><span>状态</span><select aria-label="筛选操作状态" value={draft.status} onChange={event => updateDraft({ status: operationHistoryDraft({ status: event.target.value }).status })}><option value="all">全部</option><option value="active">进行中</option><option value="ended">已结束</option><option value="failed">需关注</option></select></label>
    </div>
    {error && <div className="inline-error operation-history-error" role="alert">{error}</div>}
    <details className="operation-history-note"><summary>记录说明</summary><p className="muted small-text">记录在各窗口同步。查看结果或返回界面不会重复执行操作；关闭本页后，任务仍会继续。</p></details>
    <div className="operation-history-entries">
      {filtered.map(task => <article className={`operation-history-row ${task.status}`} key={task.id}>
        <div className="operation-history-row-title"><h2>{task.label}</h2><span className={`operation-history-status ${task.status}`}>{taskStatus[task.status] || task.status}</span></div>
        {(task.stage !== task.label || task.progress) && <p className="operation-history-stage">{task.stage}{task.progress && ` · ${task.progress.completed}/${task.progress.total}`}</p>}
        <p className="muted small-text operation-history-date"><time dateTime={task.createdAt}>{dateText(task.createdAt)}</time>{task.finishedAt && <span>结束于 <time dateTime={task.finishedAt}>{dateText(task.finishedAt)}</time></span>}</p>
        {task.error && <p className="operation-history-task-error">{task.error}</p>}
        {task.resultExpired && <p className="muted small-text">临时预览已过期，可返回对应界面重新检查。</p>}
        <div className="operation-history-actions"><button type="button" className="button" disabled={pending.has(`inspect:${task.id}`)} onClick={() => void perform(`inspect:${task.id}`, () => onInspect(task))}>查看结果</button><button type="button" className="button" disabled={pending.has(`locate:${task.id}`)} onClick={() => void perform(`locate:${task.id}`, () => onLocate(task))}>返回对应界面</button>{task.cancellable && !terminalTask(task) && <button type="button" className="button" disabled={task.cancelRequested || pending.has(`cancel:${task.id}`)} onClick={() => void perform(`cancel:${task.id}`, () => onCancel(task))}>{task.cancelRequested ? '正在取消…' : '取消任务'}</button>}</div>
      </article>)}
      {!filtered.length && <p className="operation-history-empty muted" role="status">{tasks.length ? '没有匹配的操作记录' : '暂无操作记录'}</p>}
    </div>
  </section>;
}
