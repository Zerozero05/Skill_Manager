import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = (name: string) => readFileSync(new URL(`../src/${name}`, import.meta.url), 'utf8');
const themeCss = source('themes.css');
type Palette = Record<string, string>;
const declarations = (body: string): Palette => Object.fromEntries([...body.matchAll(/(--[\w-]+)\s*:\s*([^;]+)(?:;|$)/g)].map(match => [match[1], match[2].trim()]));
const blocks = [...themeCss.matchAll(/(:root(?:\[data-theme=(\w+)\])?)\s*\{([^}]+)\}/g)];
const base = declarations(blocks.find(match => match[1] === ':root')![3]);
const themes = Object.fromEntries(['light', 'dark', 'eye'].map(name => [name, { ...base, ...declarations(blocks.find(match => match[2] === name)?.[3] || '') }]));
function resolve(palette: Palette, key: string): string {
  const value = palette[key]; assert(value, `Missing color ${key}`);
  const alias = /^var\((--[\w-]+)\)$/.exec(value);
  return alias ? resolve(palette, alias[1]) : value;
}
function luminance(hex: string): number {
  assert.match(hex, /^#[\da-f]{6}$/i, `Opaque sRGB color required: ${hex}`);
  const rgb = [1, 3, 5].map(index => parseInt(hex.slice(index, index + 2), 16) / 255).map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
  return rgb.reduce((sum, value, index) => sum + value * [.2126, .7152, .0722][index], 0);
}
function contrast(one: string, two: string): number {
  const values = [luminance(one), luminance(two)];
  return (Math.max(...values) + .05) / (Math.min(...values) + .05);
}
const atLeast = (one: string, two: string, minimum: number, label: string) => assert(contrast(one, two) >= minimum, `${label}: ${contrast(one, two).toFixed(3)} < ${minimum}`);

test('all three theme surfaces keep body, metadata, controls and selection readable', () => {
  for (const [name, palette] of Object.entries(themes)) {
    for (const surface of ['--page', '--sidebar', '--surface', '--surface-soft', '--surface-raised', '--input', '--hover', '--selected', '--code-bg']) {
      const background = resolve(palette, surface);
      atLeast(resolve(palette, '--text'), background, 7, `${name} body on ${surface}`);
      atLeast(resolve(palette, '--text-secondary'), background, 4.5, `${name} secondary on ${surface}`);
      atLeast(resolve(palette, '--control-line'), background, 3, `${name} boundary on ${surface}`);
      atLeast(resolve(palette, '--accent'), background, 3, `${name} focus on ${surface}`);
    }
    for (const tone of ['good', 'warn', 'bad', 'info', 'neutral']) {
      const foreground = tone === 'neutral' ? '--neutral-text' : `--${tone}`;
      atLeast(resolve(palette, foreground), resolve(palette, `--${tone}-bg`), 7, `${name} semantic ${tone}`);
    }
    for (const fill of ['--accent', '--accent-dark']) atLeast(resolve(palette, '--on-accent'), resolve(palette, fill), 7, `${name} primary ${fill}`);
    atLeast(resolve(palette, '--disabled-text'), resolve(palette, '--disabled-bg'), 4.5, `${name} disabled control`);
  }
});

test('category surfaces remain colorful and visible independently of the theme accent', () => {
  const css = source('category-colors.css');
  const colors = [...css.matchAll(/\[data-category-color=(\w+)\]\{([^}]+)\}/g)].map(match => ({ name: match[1], ...declarations(match[2]) }));
  assert.equal(colors.length, 8);
  assert.equal(new Set(colors.map(color => color['--category-fill'])).size, 8);
  const ink = declarations(css.match(/\[data-category-color\]\{([^}]+)\}/)![1])['--category-ink'];
  for (const color of colors) {
    atLeast(ink, color['--category-fill'], 7, `${color.name} label and selected picker`);
    for (const [name, palette] of Object.entries(themes)) {
      const dot = name === 'dark' ? color['--category-fill'] : color['--category-color'];
      atLeast(color['--category-fill'], resolve(palette, '--surface'), 1.5, `${name} ${color.name} filled label distinction`);
      for (const background of ['--surface', '--sidebar']) atLeast(dot, resolve(palette, background), 3, `${name} ${color.name} marker on ${background}`);
    }
  }
});

test('theme identity and palette authority remain separate from fonts, scale and component rules', () => {
  for (const name of ['styles.css', 'contrast.css']) {
    const css = source(name);
    assert(!/--(?:page|surface|text|accent|control-line|on-accent)\s*:/.test(css), `${name} redefines the central palette`);
  }
  assert.equal(resolve(themes.dark, '--page'), '#181818');
  assert.equal(resolve(themes.dark, '--sidebar'), '#1c1a1e');
  assert.equal(resolve(themes.dark, '--surface'), '#242424');
  assert.equal(resolve(themes.dark, '--input'), '#363636');
  assert(!/font(?:-size|-family|-weight)?\s*:|--ui-(?:zoom|font-size)\s*:/.test(themeCss), 'Theme colors do not override saved typography or scale');
});
