import { CircleAlert, CircleHelp, ShieldCheck } from 'lucide-react';
import { runtimeAttentionCount, uniqueRuntimeIssues, type RuntimeIssue } from './runtime';
import { unattachedRuntimeIssues, type RuntimeReference } from './runtime-presentation';

export function RuntimeChecks({ issues, warnings, embedded = false, references = [] }: { issues?: RuntimeIssue[]; warnings?: string[]; embedded?: boolean; references?: readonly RuntimeReference[] }) {
  const checks = unattachedRuntimeIssues(uniqueRuntimeIssues(issues || []), references).filter(issue => issue.category !== 'resolved');
  const legacy = issues === undefined ? [...new Set(warnings || [])] : [];
  if (!checks.length && !legacy.length) return null;
  const groups = [
    { id: 'package', label: '包内文件与待核对引用', items: checks.filter(issue => issue.category === 'package_required' || issue.category === 'uncertain') },
    { id: 'external', label: '外部工具与配置', items: checks.filter(issue => issue.category === 'external') },
    { id: 'history', label: '来源、示例与参考信息', items: checks.filter(issue => issue.category === 'history') },
  ].filter(group => group.items.length);
  const attention = runtimeAttentionCount(checks);
  return <section className={`runtime-checks${embedded ? ' runtime-checks-embedded' : ''}`} aria-label="运行依赖检查结果">
    <div className={`runtime-check-summary ${attention ? 'needs-review' : ''}`}>
      {attention ? <CircleAlert size={15} /> : <ShieldCheck size={15} />}
      <strong>{attention ? `${attention} 项需要核对` : checks.length ? '依赖信息' : '旧版检查记录'}</strong>
      <span>文件可读不代表调用成功</span>
    </div>
    {groups.map(group => <details key={group.id} className="runtime-check-group" data-fold-key={`runtime:${group.id}`} open={group.items.some(issue => issue.severity === 'error' || issue.severity === 'warning') || undefined}>
      <summary data-outline-label={group.label} data-outline-level={embedded ? 2 : 1}>{group.label}<small>{group.items.length} 项</small></summary>
      {group.items.map((issue, index) => <div className={`runtime-check-item severity-${issue.severity}`} key={index}>
        <div><strong>{issue.category === 'external' ? '外部引用' : issue.severity === 'error' ? '缺失或不可用' : issue.severity === 'warning' ? '待核对' : '说明'}</strong><span>{issue.reason}</span></div>
        {!!issue.target && <code>{issue.target}</code>}
        {!!issue.source && <small>引用位置：{issue.source}</small>}
      </div>)}
    </details>)}
    {!!legacy.length && <details className="runtime-check-group" data-fold-key="runtime:legacy"><summary data-outline-label="旧版检查信息" data-outline-level={embedded ? 2 : 1}><CircleHelp size={14} />旧版检查信息<small>{legacy.length} 项</small></summary>
      <p className="muted small-text">重新核对后会按实际用途分类。</p>
      {legacy.map(note => <div className="runtime-check-item" key={note}>{note}</div>)}
    </details>}
  </section>;
}
