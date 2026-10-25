import { useEffect, useState, type ReactNode } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import ReactMarkdown, { defaultUrlTransform } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { BookOpen, CircleAlert, LoaderCircle, LockKeyhole, RefreshCw } from 'lucide-react';
import { hasDeclaration, informationLinkPath, informationReadErrors, informationSections, type SkillInformationResult } from './information';

type Props = {
  skillId: string; localDependencies?: unknown; localVerification?: unknown; remoteCommit?: string | null; readGeneration?: number | null; sourceKey?: string;
  onReadOriginal: (documentPath: string, commit?: string, source?: 'local' | 'remote') => void;
};

// Only the immutable remote payload is reused. Local declarations are reread on
// each visit, even when a remote commit has not changed.
const cache = new Map<string, Promise<SkillInformationResult['remote']>>();
async function readInformation(skillId: string, remoteCommit?: string | null, readGeneration?: number | null, sourceKey = '', retry = false) {
  if (!isTauri()) throw new Error('请在 Skill Manager 桌面程序中读取来源信息。');
  const key = JSON.stringify([readGeneration, sourceKey, skillId, remoteCommit]);
  if (retry) cache.delete(key);
  const saved = readGeneration != null && remoteCommit && cache.get(key);
  if (saved) {
    const [local, remote] = await Promise.all([
      invoke<SkillInformationResult>('manager_request', { request: { action: 'get_skill_info', skillId, localOnly: true } }), saved,
    ]);
    return { ...local, remote };
  }
  const pending = invoke<SkillInformationResult>('manager_request', { request: { action: 'get_skill_info', skillId, ...(remoteCommit ? { remoteCommit } : {}) } });
  if (remoteCommit && readGeneration != null) {
    const remote = pending.then(value => value.remote);
    cache.set(key, remote);
    while (cache.size > 32) cache.delete(cache.keys().next().value!);
    void remote.then(value => { if (value.readStatus === 'error') cache.delete(key); }, () => cache.delete(key));
  }
  return pending;
}

function Declaration({ value }: { value: unknown }) {
  return typeof value === 'string' ? <p className="information-summary">{value}</p> : <pre className="information-summary">{JSON.stringify(value, null, 2)}</pre>;
}
function time(value?: string) {
  if (!value) return '读取时间未知';
  const date = new Date(value); return Number.isNaN(date.valueOf()) ? value : date.toLocaleString('zh-CN', { hour12: false });
}

export function SkillInformation({ skillId, localDependencies, localVerification, remoteCommit, readGeneration, sourceKey, onReadOriginal }: Props) {
  const [data, setData] = useState<SkillInformationResult>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [linkError, setLinkError] = useState<string>();
  useEffect(() => {
    let cancelled = false;
    setData(undefined); setError(undefined); setLinkError(undefined); setBusy(true);
    readInformation(skillId, remoteCommit, readGeneration, sourceKey, attempt > 0)
      .then(value => { if (!cancelled) setData(value); })
      .catch(reason => { if (!cancelled) setError(String(reason)); })
      .finally(() => { if (!cancelled) setBusy(false); });
    return () => { cancelled = true; };
  }, [skillId, remoteCommit, readGeneration, sourceKey, attempt]);

  const remote = data?.remote;
  const local = data?.local;
  const doc = remote?.document;
  const catalog = remote?.catalog;
  const localDeps = data ? local?.dependencies : localDependencies;
  const localVerify = data ? local?.verification : localVerification;
  const localDependencySection = local?.readStatus === 'ok' ? informationSections(local.markdown || '', 'dependencies') : '';
  const localVerificationSection = local?.readStatus === 'ok' ? informationSections(local.markdown || '', 'verification') : '';
  const dependencySection = doc?.readStatus === 'ok' ? informationSections(doc.markdown || '', 'dependencies') : '';
  const verificationSection = doc?.readStatus === 'ok' ? informationSections(doc.markdown || '', 'verification') : '';
  const provenance = (path?: string, commit?: string, checkedAt?: string, source?: string, privateSource?: boolean) => <div className="information-source"><span>{privateSource && <LockKeyhole size={12} />}{source || '本地'}{path && ` · ${path}`}</span>{commit && <span className="monospace" title={commit}>提交 {commit.slice(0, 12)}</span>}{checkedAt && <span>文档读取：{time(checkedAt)}</span>}{path && <button className="text-button" onClick={() => onReadOriginal(path, commit, commit ? 'remote' : 'local')}><BookOpen size={13} />查看原文</button>}</div>;
  const sourceError = informationReadErrors(data, error).join('；') || undefined;
  const md = (text: string, path: string, commit?: string): ReactNode => <article className="markdown information-markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml urlTransform={defaultUrlTransform} components={{
    a: ({ href, children }) => {
      const relative = href && informationLinkPath(path, href);
      if (relative) return <button className="text-button document-link" onClick={() => onReadOriginal(relative, commit, commit ? 'remote' : 'local')}>{children}</button>;
      if (href && /^https?:\/\//i.test(href)) return <a href={href} onClick={event => { event.preventDefault(); void openUrl(href).catch(reason => setLinkError(String(reason))); }}>{children}</a>;
      return <span>{children}</span>;
    },
    // Inline source sections do not make background requests to arbitrary image hosts.
    img: ({ alt }) => <span className="muted">{alt ? `[图片：${alt}]` : '[图片请查看原文]'}</span>,
  }}>{text}</ReactMarkdown></article>;
  return <section className="skill-information" aria-label="依赖与来源验证信息" data-outline-label="依赖与来源验证信息">
    <div className="section-title"><h3>依赖与验证说明</h3><button className="button ghost" disabled={busy} onClick={() => setAttempt(value => value + 1)}><RefreshCw size={14} />重新读取</button></div>
    <p className="muted information-caption">来源声明不代表本机已安装依赖或调用成功。</p>
    {busy && <p className="muted" role="status"><LoaderCircle size={14} className="spin" /> 正在读取来源信息…</p>}
    {sourceError && <div className="inline-error information-error" role="alert"><CircleAlert size={15} /><span>信息读取未完成：{sourceError}。可重新读取；可用的本地声明及已读取来源仍显示在下方。</span></div>}
    {linkError && <p className="inline-error" role="alert">链接暂时无法打开：{linkError}</p>}
    <details className="information-block" open><summary>依赖与权限</summary>
      {hasDeclaration(localDeps) && <><h4>{data ? '本地 SKILL.md 声明' : '登记时的本地声明'}</h4><Declaration value={localDeps} />{provenance(local?.declarationPath || 'SKILL.md', undefined, local?.checkedAt, '本地声明')}</>}
      {localDependencySection && <><h4>本地文档说明</h4>{md(localDependencySection, local?.documentPath || 'SKILL.md')}{provenance(local?.documentPath, undefined, local?.checkedAt, '本地文档')}</>}
      {hasDeclaration(catalog?.dependencies) && <><h4>来源目录索引声明</h4><Declaration value={catalog?.dependencies} />{provenance(catalog?.documentPath, remote?.commit, remote?.checkedAt, remote?.source, remote?.private)}</>}
      {dependencySection && <><h4>来源文档说明</h4>{md(dependencySection, doc?.documentPath || 'README.md', remote?.commit)}{provenance(doc?.documentPath, remote?.commit, doc?.checkedAt, remote?.source, remote?.private)}</>}
      {!busy && (data || error) && !hasDeclaration(localDeps) && !localDependencySection && !hasDeclaration(catalog?.dependencies) && !dependencySection && <p className="muted">{sourceError ? '尚不能确认依赖声明是否齐全，请重试或查看原文。' : '已读取的来源未声明依赖与权限。'}</p>}
    </details>
    <details className="information-block" open><summary>验证与局限</summary>
      {hasDeclaration(localVerify) && <><h4>本地文件中的验证声明</h4><Declaration value={localVerify} />{provenance(local?.declarationPath || 'SKILL.md', undefined, local?.checkedAt, '本地声明')}</>}
      {localVerificationSection && <><h4>本地文档中的验证说明</h4>{md(localVerificationSection, local?.documentPath || 'SKILL.md')}{provenance(local?.documentPath, undefined, local?.checkedAt, '本地文档')}</>}
      {hasDeclaration(catalog?.verification) && <><h4>来源验证摘要</h4><Declaration value={catalog?.verification} />{provenance(catalog?.documentPath, remote?.commit, remote?.checkedAt, remote?.source, remote?.private)}</>}
      {verificationSection && <><h4>来源验证详情</h4>{md(verificationSection, doc?.documentPath || 'README.md', remote?.commit)}{provenance(doc?.documentPath, remote?.commit, doc?.checkedAt, remote?.source, remote?.private)}</>}
      {!busy && (data || error) && !hasDeclaration(localVerify) && !localVerificationSection && !hasDeclaration(catalog?.verification) && !verificationSection && <p className="muted">{sourceError ? '尚不能确认来源验证信息是否齐全，请重试或查看原文。' : '已读取的来源未声明验证信息。'}</p>}
    </details>
    {data && <details className="information-read-status"><summary>读取来源与状态</summary><dl className="info-grid"><dt>本地文档</dt><dd>{local?.readStatus === 'ok' ? '已读取' : local?.readStatus === 'error' ? local.error : '尚无本地文件'}</dd><dt>目录索引</dt><dd>{remote?.readStatus === 'not_applicable' ? '未关联 GitHub' : catalog?.readStatus === 'ok' ? '已读取当前 Skill 条目' : catalog?.readStatus === 'error' ? catalog.error : '此提交未发现当前 Skill 条目'}</dd><dt>来源文档</dt><dd>{remote?.readStatus === 'not_applicable' ? '未关联 GitHub' : doc?.readStatus === 'ok' ? doc.documentPath : doc?.readStatus === 'error' ? doc.error : '此提交未发现文档'}</dd></dl>{doc?.readStatus === 'ok' && provenance(doc.documentPath, remote?.commit, doc.checkedAt, remote?.source, remote?.private)}</details>}
  </section>;
}
