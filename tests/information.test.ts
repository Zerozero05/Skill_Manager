import test from 'node:test';
import assert from 'node:assert/strict';
import { hasDeclaration, informationLinkPath, informationReadErrors, informationSections, type SkillInformationResult } from '../src/information.ts';

test('numbered Chinese README sections keep conditional wording, nested headings, tables and fenced code', () => {
  const md = '# Skill\n## 6. 依赖与权限\n仅可选脚本需要 Python。\n\n| 工具 | 条件 |\n| --- | --- |\n| Python | 可选 |\n\n### 样例\n```md\n## 验证与局限\n这个标题位于代码内\n```\n\n## 7. 验证与局限\n静态检查通过；未验证实际业务调用。\n## 8. 来源\nOther';
  const dependency = informationSections(md, 'dependencies');
  assert(dependency.startsWith('仅可选脚本需要 Python。')); assert(dependency.includes('| Python | 可选 |'));
  assert(dependency.includes('### 样例\n```md\n## 验证与局限')); assert(!dependency.includes('静态检查通过'));
  assert.equal(informationSections(md, 'verification'), '静态检查通过；未验证实际业务调用。');
});
test('ATX and setext English headings work while frontmatter and fenced fake headings do not', () => {
  const md = '---\ncompatibility: |\n  ## Dependencies\n  fake\n---\n~~~markdown\n# Dependencies\nfake\n~~~\nDependencies & Permissions\n---\nOptional only.\n\nVerification and limitations\n---\nNot invoked.\n\n# Other\nno';
  assert.equal(informationSections(md, 'dependencies'), 'Optional only.');
  assert.equal(informationSections(md, 'verification'), 'Not invoked.');
  assert.equal(informationSections('## Compatibility\nUses CLI\n## Tested\nStatic only\n', 'dependencies'), 'Uses CLI');
});
test('longer fences retain shorter marker lines and multiple matching sections are combined once', () => {
  const md = '## Dependencies\nA\n### Requirements\nB\n````md\n```\n## Verification\nfake\n````\n## Validation\nV1\n## Verification\nV2\n';
  assert.equal(informationSections(md, 'verification'), 'V1\n\nV2');
  assert.equal(informationSections('## 依赖与权限\n\n## Other\nText', 'dependencies'), '');
  assert(informationSections(md, 'dependencies').includes('## Verification\nfake'));
});
test('relative document links resolve within source root and protocols/traversal are rejected', () => {
  assert.equal(informationLinkPath('s/README.md', 'references/details.md#example'), 's/references/details.md');
  assert.equal(informationLinkPath('s/README.md', '../skill-catalog.json'), 'skill-catalog.json');
  for (const href of ['../../outside.md', 'javascript:alert(1)', 'https://example.com', '//host/x', '%00bad', '%zz', '..\\outside', '#x']) assert.equal(informationLinkPath('s/README.md', href), undefined);
});
test('source structured values retain fields instead of becoming an empty placeholder', () => {
  assert.equal(hasDeclaration({ summary: 'Format only', limitations: 'Not invoked' }), true);
  assert.equal(hasDeclaration(['Optional Python']), true);
  for (const value of [undefined, null, '', '  ', {}, []]) assert.equal(hasDeclaration(value), false);
});
test('partial source read failures remain unknown even when the snapshot itself is available', () => {
  const result: SkillInformationResult = { skillId: 's', local: { readStatus: 'ok' }, remote: {
    readStatus: 'ok', catalog: { readStatus: 'error', error: 'catalog malformed' }, document: { readStatus: 'error' },
  } };
  assert.deepEqual(informationReadErrors(result), ['catalog malformed', '来源文档读取未完成']);
  result.remote.remoteError = 'catalog malformed; 来源文档读取未完成';
  assert.deepEqual(informationReadErrors(result), ['catalog malformed; 来源文档读取未完成']);
  result.remote = { readStatus: 'ok', catalog: { readStatus: 'absent' }, document: { readStatus: 'ok', markdown: '# Without declarations' } };
  assert.deepEqual(informationReadErrors(result), []);
});
