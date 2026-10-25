import type { ReactNode } from 'react';
import { Check, CircleAlert, CircleHelp, FileCode2, Minus } from 'lucide-react';
import { exportResourceState, type ExportResource } from './export-selection';
import './file-layout.css';

export type FileResourceRowProps = {
  resource?: ExportResource;
  required?: boolean;
  evidencePending?: boolean;
  choiceLabel?: string;
  title: string;
  subtitle?: ReactNode;
  description?: ReactNode;
  source?: ReactNode;
  sizeText?: string;
  filePath?: string;
  onOpen?: () => void;
  onToggle?: (id: string, included: boolean) => void;
  disabled?: boolean;
  children?: ReactNode;
};

/** One resource owns one export control; attached references only manage their sources. */
export function FileResourceRow({ resource, required, evidencePending = false, choiceLabel, title, subtitle, description, source, sizeText, filePath, onOpen, onToggle, disabled = false, children }: FileResourceRowProps) {
  const automaticPending = evidencePending && (required || resource?.group === 'required' || resource?.source === 'generated');
  const state = exportResourceState(resource, !!automaticPending);
  const Icon = automaticPending ? CircleHelp : state.problem ? CircleAlert : state.automatic && state.available ? Check : resource ? Minus : CircleHelp;
  const name = <><FileCode2 size={17} aria-hidden="true" /><strong>{title}</strong></>;
  return <div className={`file-resource-row ${resource?.included && !automaticPending ? 'is-included' : ''} ${state.problem ? 'has-issues' : ''}`} data-export-resource={resource?.id} data-file-resource={resource?.id || filePath} data-file-path={filePath} data-outline-anchor={filePath ? `file:${filePath}` : undefined}>
    <div className="file-resource-main">
      <div className="file-resource-copy">
        {onOpen ? <button className="file-resource-open" type="button" onClick={onOpen} title={filePath || title}>{name}</button> : <div className="file-resource-name" title={filePath || title}>{name}</div>}
        {(subtitle || source) && <div className="file-resource-secondary">{subtitle && <span>{subtitle}</span>}{source && <span>{source}</span>}</div>}
        {description && <div className="file-resource-description">{description}</div>}
      </div>
      {sizeText && <span className="file-resource-size">{sizeText}</span>}
      <span className="file-resource-control">
        {state.selectable && resource ? <input type="checkbox" checked={resource.included} disabled={disabled || !onToggle} aria-label={choiceLabel || `加入 ZIP ${title}`} title={state.available ? '仅影响本次 ZIP，不改变长期用途' : '此项已不可用，可以取消本次选择'} onChange={event => { if (!event.target.checked || state.available) onToggle?.(resource.id, event.target.checked); }} /> : !resource && required === false ? <input type="checkbox" checked={false} disabled aria-label={choiceLabel || `加入 ZIP ${title}`} title="导出范围待核对" readOnly /> : <span className={`file-resource-state ${state.problem ? 'needs-review' : ''}`} role="img" aria-label={state.label} title={state.label}><Icon size={17} /></span>}
      </span>
    </div>
    {children && <div className="file-resource-details">{children}</div>}
  </div>;
}
