import { useCallback, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Check, ChevronDown, RefreshCw } from 'lucide-react';
import { FloatingMenu, type OpenMenu } from './FloatingMenu';
import type { useTasks } from './tasks';
import './checks.css';

export type CheckMode = 'auto' | 'manual';
export function CheckControl({ snapshot, tasks, onSnapshot, disabled = false }: { snapshot: unknown; tasks: ReturnType<typeof useTasks>; onSnapshot: (value: unknown) => void; disabled?: boolean }) {
  const mode: CheckMode = (snapshot as { checkMode?: CheckMode } | undefined)?.checkMode === 'manual' ? 'manual' : 'auto';
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const [saving, setSaving] = useState(false), [error, setError] = useState('');
  const running = tasks.tasks.some(task => ['queued', 'running'].includes(task.status) && task.action === 'check_now');
  const close = useCallback((focus = false) => setMenu(previous => { if (focus && previous?.anchor.isConnected) previous.anchor.focus(); return null; }), []);
  async function change(next: CheckMode) {
    if (next === mode || saving) return;
    setSaving(true); setError('');
    try { onSnapshot(await invoke('manager_request', { request: { action: 'set_check_mode', mode: next } })); }
    catch (cause) { setError(String(cause)); }
    finally { setSaving(false); }
  }
  async function check() {
    setError('');
    try { onSnapshot(await tasks.run('正在检查变化', 'check_now', {}, { view: 'changes' })); }
    catch (cause) { setError(String(cause)); }
  }
  return <div className="global-check-control">
    <div className="check-button-group">
      <button className="button ghost" aria-label="检查本地与 GitHub 变化" disabled={disabled || running || !snapshot} onClick={() => void check()}><RefreshCw size={15} className={running ? 'spin' : ''} /><span>检查</span></button>
      <button className="button ghost check-mode" aria-haspopup="menu" aria-expanded={!!menu} aria-label="检查方式" disabled={disabled || saving || !snapshot} title={mode === 'auto' ? '自动发现本地变化，每分钟检查 GitHub 提交' : '仅在点击检查时核对；不自动扫描'} onClick={event => setMenu(menu ? null : { id: 'check-mode', anchor: event.currentTarget, label: '检查方式', items: (['auto', 'manual'] as const).map(value => ({ id: value, label: <><Check size={14} style={{ visibility: mode === value ? 'visible' : 'hidden' }} />{value === 'auto' ? '自动检查' : '手动检查'}</>, action: () => void change(value) })) })}>{mode === 'auto' ? '自动' : '手动'}<ChevronDown size={13} /></button>
    </div>
    {menu && <FloatingMenu menu={menu} onClose={close} />}
    {error && <div className="check-error" role="alert"><span>{error}</span><button className="text-button" onClick={() => setError('')}>关闭</button></div>}
  </div>;
}
