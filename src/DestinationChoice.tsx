import { useId } from 'react';
import { Check, FolderOpen, LoaderCircle } from 'lucide-react';
import { destinationReady, type DestinationDraft } from './destination';
import './destination.css';

export function DestinationChoice({ kind, value, defaultPath, loading, error, busy, onChange, onChoose, onRetry }: {
  kind: 'download' | 'export'; value: DestinationDraft; defaultPath: string; loading: boolean; error?: string; busy: boolean;
  onChange: (choice: DestinationDraft) => void; onChoose: () => void; onRetry: () => void;
}) {
  const name = useId(), label = kind === 'export' ? 'ZIP 保存位置' : 'Skill 保存位置';
  const valid = destinationReady(value, defaultPath);
  return <fieldset className="destination-choice" disabled={busy}>
    <legend>{label}</legend>
    <div className="destination-options">
      <label className={`destination-option ${value.mode === 'default' && valid ? 'is-selected' : ''}`}>
        <input type="radio" name={name} checked={value.mode === 'default' && valid} disabled={busy || loading || !defaultPath} onChange={() => onChange({ mode: 'default', path: defaultPath })} />
        <span><strong>使用默认位置</strong><code>{loading ? '正在读取保存位置…' : defaultPath || '默认位置暂不可用'}</code></span>
        {loading ? <LoaderCircle className="spin" size={16} /> : value.mode === 'default' && valid ? <Check size={16} /> : null}
      </label>
      <button type="button" className={`destination-option destination-custom ${value.mode === 'custom' ? 'is-selected' : ''}`} aria-label={`自定义${label}`} onClick={onChoose} disabled={busy}>
        <FolderOpen size={18} /><span><strong>自定义位置</strong><code>{value.mode === 'custom' ? value.path : '选择其他文件夹或保存路径'}</code></span>{value.mode === 'custom' && <Check size={16} />}
      </button>
    </div>
    {kind === 'download' && <label className="destination-manual">自定义完整保存目录
      <input aria-label="Skill 完整保存目录" value={value.mode === 'custom' ? value.path : ''} placeholder="也可直接输入目录绝对路径" spellCheck={false} onChange={event => onChange({ mode: 'custom', path: event.target.value })} />
    </label>}
    {error && <div className="destination-error" role="alert">{error}<button type="button" className="text-button" onClick={onRetry} disabled={busy || loading}>重试</button></div>}
    {value.mode === 'default' && value.path && !loading && !error && !valid && <p className="destination-error" role="status">默认位置已更改，请重新选择本次保存位置。</p>}
    <p className="destination-current" role="status">{valid ? <>本次目标：<code>{value.path}</code></> : '尚未选择本次保存位置。'}</p>
  </fieldset>;
}
