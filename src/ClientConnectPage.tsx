import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Search } from 'lucide-react';
import { CLIENT_OPTIONS, clientConnectChoices, clientConnectDraft, clientSelectionRange, clientSelectionShortcut, clientSkillName, completedClientBatchDraft, selectFilteredClientItems, type ClientBatchCompletion, type ClientConnection, type ClientConnectDraft, type ClientSkill, type ConnectionResult } from './client-selection';
import './clients.css';

export type ClientConnectPageProps = {
  skills: ClientSkill[]; connections: ClientConnection[]; client: string;
  initialDraft?: ClientConnectDraft; onDraftChange: (draft: ClientConnectDraft) => void;
  onConnect: (client: string, skillIds: string[]) => Promise<ConnectionResult[]>;
  resumedBatch?: ClientBatchCompletion;
  pendingBatch?: boolean;
  onClose?: () => void;
};
const editable = (target: EventTarget | null) => target instanceof HTMLElement && (target.isContentEditable || target.matches('input:not([type="checkbox"]),textarea,select'));
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

/** A normal operation page: the enclosing PaneFrame owns scrolling and transfer. */
export function ClientConnectPage({ skills, connections, client, initialDraft, onDraftChange, onConnect, resumedBatch, pendingBatch = false, onClose }: ClientConnectPageProps) {
  const [draft, setDraft] = useState(() => clientConnectDraft(initialDraft)), [results, setResults] = useState<ConnectionResult[]>(() => initialDraft?.batchClient === client ? initialDraft.results || [] : []), [connecting, setConnecting] = useState(false), [error, setError] = useState('');
  const pointerShift = useRef(false), alive = useRef(true), batchId = useRef(0), draftRef = useRef(draft); draftRef.current = draft;
  const latestDraftChange = useRef(onDraftChange); latestDraftChange.current = onDraftChange;
  const firstClient = useRef(client);
  useEffect(() => { alive.current = true; return () => { alive.current = false; batchId.current += 1; }; }, []);
  useEffect(() => { if (firstClient.current !== client) { firstClient.current = client; batchId.current += 1; setResults([]); setConnecting(false); const clean = clientConnectDraft(undefined); draftRef.current = clean; setDraft(clean); latestDraftChange.current(clean); } }, [client]);
  useEffect(() => {
    if (!resumedBatch) return;
    const next = completedClientBatchDraft(draftRef.current, client, resumedBatch); if (!next) return;
    batchId.current += 1; setConnecting(false); setError(''); setResults(resumedBatch.results);
    draftRef.current = next; setDraft(next); latestDraftChange.current(next);
  }, [resumedBatch?.taskId, client]);
  const available = useMemo(() => clientConnectChoices(skills, connections, client), [skills, connections, client]);
  const choices = useMemo(() => clientConnectChoices(skills, connections, client, draft.query), [skills, connections, client, draft.query]);
  const availableIds = available.map(skill => skill.id), filteredIds = choices.map(skill => skill.id), selectedIds = draft.selectedIds.filter(id => availableIds.includes(id));
  const availability = availableIds.join('\u0000');
  useEffect(() => {
    const next = draftRef.current.selectedIds.filter(id => availableIds.includes(id));
    if (next.length !== draftRef.current.selectedIds.length) { const value = { ...draftRef.current, selectedIds: next }; draftRef.current = value; setDraft(value); latestDraftChange.current(value); }
  }, [availability]);
  const hidden = selectedIds.filter(id => !filteredIds.includes(id)).length, clientName = CLIENT_OPTIONS.find(item => item.id === client)?.name || client;
  const alreadyConnected = skills.filter(skill => skill.localPath && !skill.readOnly).length - available.length;
  const busy = connecting || pendingBatch;
  const update = (next: ClientConnectDraft) => { draftRef.current = next; setDraft(next); latestDraftChange.current(next); };
  const choose = (id: string, checked: boolean, shift = false) => update({ ...draftRef.current, selectedIds: clientSelectionRange(draftRef.current.selectedIds, filteredIds, draftRef.current.anchorId, id, checked, shift), anchorId: id });
  const selectAll = () => update({ ...draftRef.current, selectedIds: selectFilteredClientItems(draftRef.current.selectedIds, filteredIds) });
  const listKeys = (event: KeyboardEvent) => { if (!busy && clientSelectionShortcut(event, editable(event.target)) === 'all') { event.preventDefault(); selectAll(); } };
  const connect = async (requested: string[]) => {
    if (busy) return; const ids = [...new Set(requested)].filter(id => availableIds.includes(id)); if (!ids.length) return;
    const generation = ++batchId.current, targetClient = client; setConnecting(true); setError(''); setResults([]); update({ ...draftRef.current, results: [], batchClient: targetClient });
    try {
      const next = await onConnect(targetClient, ids);
      if (!alive.current || generation !== batchId.current) return;
      setResults(next); const successful = new Set(next.filter(item => item.success && !item.error).map(item => item.id));
      update({ ...draftRef.current, selectedIds: draftRef.current.selectedIds.filter(id => !successful.has(id)), results: next, batchClient: targetClient });
    } catch (cause) { if (alive.current && generation === batchId.current) { const message = errorText(cause), failures = ids.map(id => ({ id, label: clientSkillName(skills.find(skill => skill.id === id)), success: false, error: message })); setError(message); setResults(failures); update({ ...draftRef.current, results: failures, batchClient: targetClient }); } }
    finally { if (alive.current && generation === batchId.current) setConnecting(false); }
  };
  return <section className="client-connect-page" aria-label="连接 Skill">
    <header><h2>连接到 {clientName}</h2><p>选择本地 Skill，使用原目录建立轻量入口。{alreadyConnected > 0 && `已排除 ${alreadyConnected} 项有效同源连接。`}</p></header>
    <label className="client-connect-search"><Search size={16} aria-hidden="true" /><input aria-label="搜索可连接 Skill" placeholder="名称或原目录" value={draft.query} onChange={event => update({ ...draftRef.current, query: event.target.value })} /></label>
    <div className="client-selection-tools"><button type="button" className="text-button" disabled={busy || !choices.length} onClick={selectAll}>选择当前筛选全部（{choices.length}）</button><button type="button" className="text-button" disabled={busy || !draft.selectedIds.length} onClick={() => update({ ...draftRef.current, selectedIds: [], anchorId: undefined })}>清空选择</button><span role="status">已选 {selectedIds.length} 项{hidden > 0 && ` · 筛选外 ${hidden} 项`}</span></div>
    <div className="client-choices client-selectable-list" role="group" aria-label="可连接 Skill 列表" tabIndex={0} onKeyDown={listKeys}>{choices.map(skill => {
      const selected = selectedIds.includes(skill.id);
      return <label key={skill.id} className={selected ? 'is-selected' : ''} tabIndex={0} onKeyDown={event => { if (!busy && event.target === event.currentTarget && event.key === ' ' && !event.ctrlKey && !event.altKey && !event.metaKey) { event.preventDefault(); choose(skill.id, !selected, event.shiftKey); } }}><input type="checkbox" aria-label={`选择 ${clientSkillName(skill)}`} checked={selected} disabled={busy} onClick={event => { pointerShift.current = event.shiftKey; }} onChange={event => { choose(skill.id, event.target.checked, pointerShift.current); pointerShift.current = false; }} /><span><strong>{clientSkillName(skill)}</strong><code title={skill.localPath || undefined}>{skill.localPath}</code></span></label>;
    })}{!choices.length && <p className="client-connect-empty">{available.length ? '没有匹配的可连接 Skill；已选项目仍保留。' : '当前可用 Skill 已连接，或尚无可连接的本地 Skill。'}</p>}</div>
    {error && <p className="inline-error" role="alert">{error}</p>}
    {results.length > 0 && <section className="client-batch-results" role="status"><h3>本批结果 · 成功 {results.filter(item => item.success && !item.error).length}／共 {results.length} 项</h3>{results.map(result => <p key={result.id} className={result.error ? 'error-text' : undefined}>{result.label}：{result.error || '连接成功'}</p>)}{results.some(result => result.error) && <button type="button" className="button" disabled={busy} onClick={() => void connect(results.filter(item => item.error).map(item => item.id))}>仅重试失败项</button>}</section>}
    <footer className="client-connect-footer"><p>{busy ? '正在逐项连接；切换标签后任务会继续，结果保留在操作记录。' : 'Shift 可连续选择；列表获得焦点后可用 Ctrl＋A 全选当前筛选、空格切换当前项。'}</p><div>{onClose && <button type="button" className="button" onClick={onClose}>关闭页面</button>}<button type="button" className="button primary" disabled={busy || !selectedIds.length} onClick={() => void connect(selectedIds)}>{busy ? '正在逐项连接…' : `连接所选（${selectedIds.length}）`}</button></div></footer>
  </section>;
}
