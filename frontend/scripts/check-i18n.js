#!/usr/bin/env node
/**
 * Checks i18n translation files:
 *  1. every namespace exists for every language,
 *  2. en and zh-CN have exactly the same keys,
 *  3. no empty values,
 *  4. every 'ns.key' literal used in src/app exists in en (best effort, static keys only).
 * Exits non-zero on errors.
 */
const fs = require('fs');
const path = require('path');

const ROOT = path.resolve(__dirname, '..');
const I18N_DIR = path.join(ROOT, 'src/assets/i18n');
const APP_DIR = path.join(ROOT, 'src/app');
const LANGS = ['en', 'zh-CN'];

const errors = [];

function flatten(obj, prefix = '', out = {}) {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object' && !Array.isArray(v)) {
      flatten(v, key, out);
    } else {
      out[key] = v;
    }
  }
  return out;
}

function load(lang) {
  const dir = path.join(I18N_DIR, lang);
  const result = {};
  for (const file of fs.readdirSync(dir).filter(f => f.endsWith('.json'))) {
    const ns = file.replace(/\.json$/, '');
    try {
      result[ns] = flatten(JSON.parse(fs.readFileSync(path.join(dir, file), 'utf8')), ns);
    } catch (e) {
      errors.push(`${lang}/${file}: invalid JSON (${e.message})`);
      result[ns] = {};
    }
  }
  return result;
}

const data = Object.fromEntries(LANGS.map(l => [l, load(l)]));
const namespaces = new Set(LANGS.flatMap(l => Object.keys(data[l])));

for (const ns of namespaces) {
  for (const lang of LANGS) {
    if (!data[lang][ns]) errors.push(`missing namespace file ${lang}/${ns}.json`);
  }
  const [base, ...others] = LANGS;
  const baseKeys = Object.keys(data[base][ns] || {});
  for (const other of others) {
    const otherKeys = new Set(Object.keys(data[other][ns] || {}));
    for (const k of baseKeys) if (!otherKeys.has(k)) errors.push(`${other}: missing key ${k}`);
    const baseSet = new Set(baseKeys);
    for (const k of otherKeys) if (!baseSet.has(k)) errors.push(`${base}: missing key ${k}`);
  }
  for (const lang of LANGS) {
    for (const [k, v] of Object.entries(data[lang][ns] || {})) {
      if (v === '' || v === null) errors.push(`${lang}: empty value for ${k}`);
    }
  }
}

// Static key usage check
const allEn = Object.assign({}, ...Object.values(data.en));
const nsPattern = [...namespaces].join('|');
// Skips arguments of local prefixing helpers like `this.t('common.x')` (resolves to `sql.common.x`).
const keyRe = new RegExp(`(?<!\\bt\\()['"\`]((?:${nsPattern})\\.[A-Za-z0-9_.-]+)['"\`]`, 'g');
function walk(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(p);
    else if (/\.(ts|html)$/.test(entry.name) && !entry.name.endsWith('.spec.ts')) {
      const src = fs.readFileSync(p, 'utf8');
      let m;
      while ((m = keyRe.exec(src))) {
        const key = m[1];
        if (key.endsWith('.')) continue; // dynamic prefix, e.g. 'menu.functionTabs.' + name
        const isPrefix = Object.keys(allEn).some(k => k.startsWith(key + '.'));
        if (!(key in allEn) && !isPrefix) {
          errors.push(`${path.relative(ROOT, p)}: unknown key ${key}`);
        }
      }
    }
  }
}
walk(APP_DIR);

const total = Object.keys(allEn).length;
if (errors.length) {
  console.error(errors.join('\n'));
  console.error(`\n${errors.length} i18n error(s), ${total} keys.`);
  process.exit(1);
}
console.log(`i18n OK: ${namespaces.size} namespaces, ${total} keys, languages: ${LANGS.join(', ')}`);