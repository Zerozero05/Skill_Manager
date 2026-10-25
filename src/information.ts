export type ReadStatus = 'ok' | 'absent' | 'error';
export type InformationDocument = {
  readStatus: ReadStatus; error?: string; markdown?: string; documentPath?: string;
  checkedAt?: string; baseUrl?: string; source?: string; private?: boolean; commit?: string;
};
export type SkillInformationResult = {
  skillId: string;
  local: InformationDocument & { dependencies?: unknown; verification?: unknown; declarationPath?: string };
  remote: {
    readStatus: 'ok' | 'error' | 'not_applicable'; remoteError?: string; commit?: string;
    checkedAt?: string; source?: string; private?: boolean;
    catalog?: { readStatus: ReadStatus; error?: string; documentPath?: string; dependencies?: unknown; verification?: unknown };
    document?: InformationDocument;
  };
};

const DEPENDENCY_HEADINGS = new Set([
  '依赖', '依赖与权限', '依赖与兼容性', '依赖与环境', '环境与依赖', '运行依赖', '兼容性',
  'dependencies', 'dependency', 'dependencies and permissions', 'dependencies permissions',
  'requirements', 'requirements and permissions', 'compatibility', 'prerequisites',
]);
const VERIFICATION_HEADINGS = new Set([
  '验证', '验证与局限', '验证与限制', '验证情况', '验证信息', '测试与局限', '测试与限制',
  'verification', 'verification and limitations', 'verification limitations', 'validation',
  'validation and limitations', 'validation limitations', 'testing', 'testing and limitations',
  'tests and limitations', 'tested', 'test results',
]);

function normalizedHeading(text: string) {
  return text.normalize('NFKC').replace(/!?(?:\[([^\]]+)\])\([^)]*\)/g, '$1')
    .replace(/[*_`]/g, '').replace(/\s+#+\s*$/, '').trim()
    .replace(/^(?:\d+(?:\.\d+)*|[一二三四五六七八九十]+)[\s.、):：-]+/, '')
    .replace(/\s*&\s*/g, ' and ').replace(/\s+/g, ' ').replace(/[：:]$/, '').trim().toLowerCase();
}

/** Extract full Markdown sections, preserving their nested headings, tables and fenced code. */
export function informationSections(markdown: string, kind: 'dependencies' | 'verification'): string {
  const aliases = kind === 'dependencies' ? DEPENDENCY_HEADINGS : VERIFICATION_HEADINGS;
  const lines = markdown.replace(/^\uFEFF/, '').split(/\r?\n/);
  const headings: { index: number; body: number; level: number; title: string }[] = [];
  let fence: { marker: string; length: number } | undefined;
  let frontmatter = lines[0]?.trim() === '---';
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    if (frontmatter) { if (index && /^(?:---|\.\.\.)\s*$/.test(line)) frontmatter = false; continue; }
    const marker = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (marker && marker[1][0] === fence.marker && marker[1].length >= fence.length && !marker[2].trim()) fence = undefined;
      continue;
    }
    if (marker) { fence = { marker: marker[1][0], length: marker[1].length }; continue; }
    const atx = /^ {0,3}(#{1,6})(?:\s+|$)(.*)$/.exec(line);
    if (atx) { headings.push({ index, body: index + 1, level: atx[1].length, title: normalizedHeading(atx[2]) }); continue; }
    const underline = /^ {0,3}(=+|-+)\s*$/.exec(lines[index + 1] || '');
    if (line.trim() && !/^\s{4}/.test(line) && underline) {
      headings.push({ index, body: index + 2, level: underline[1][0] === '=' ? 1 : 2, title: normalizedHeading(line) }); index++;
    }
  }
  const sections: string[] = [];
  let consumedUntil = -1;
  for (let index = 0; index < headings.length; index++) {
    const heading = headings[index];
    if (heading.index < consumedUntil || !aliases.has(heading.title)) continue;
    const end = headings.slice(index + 1).find(next => next.level <= heading.level)?.index ?? lines.length;
    const section = lines.slice(heading.body, end).join('\n').trim();
    if (section) sections.push(section);
    consumedUntil = end;
  }
  return sections.join('\n\n');
}

export function hasDeclaration(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === 'string') return value.trim().length > 0;
  if (Array.isArray(value)) return value.length > 0;
  if (typeof value === 'object') return Object.keys(value).length > 0;
  return true;
}

/** A successful snapshot does not imply that each catalog/document read succeeded. */
export function informationReadErrors(data?: SkillInformationResult, requestError?: string): string[] {
  const remote = data?.remote;
  const messages = [
    requestError,
    data?.local.readStatus === 'error' ? data.local.error || '本地说明读取未完成' : undefined,
    remote?.remoteError,
    remote?.catalog?.readStatus === 'error' ? remote.catalog.error || '目录索引读取未完成' : undefined,
    remote?.document?.readStatus === 'error' ? remote.document.error || '来源文档读取未完成' : undefined,
    remote?.readStatus === 'error' && !remote.remoteError && remote.catalog?.readStatus !== 'error' && remote.document?.readStatus !== 'error' ? '远端信息读取未完成' : undefined,
  ].filter((value): value is string => !!value);
  return messages.filter((message, index) => !messages.some((other, otherIndex) => otherIndex < index && other.includes(message)));
}

/** Resolve only safe relative Markdown paths; protocols are handled separately by the renderer. */
export function informationLinkPath(documentPath: string, href: string): string | undefined {
  if (!href || href.startsWith('#') || /^[a-z][a-z\d+.-]*:/i.test(href) || href.startsWith('//')) return undefined;
  let decoded: string;
  try { decoded = decodeURIComponent(href.split(/[?#]/, 1)[0]); } catch { return undefined; }
  if (decoded.includes('\\') || /[\u0000-\u001f]/.test(decoded)) return undefined;
  const parts = decoded.startsWith('/') ? [] : documentPath.split('/').slice(0, -1);
  for (const part of decoded.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') { if (!parts.length) return undefined; parts.pop(); }
    else parts.push(part);
  }
  return parts.length ? parts.join('/') : undefined;
}
