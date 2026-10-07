// host/tests/compare/evidence.mjs
//
// compare.mjs 的證據檔輸出（fix F7，批次 A：結果原本只印到 stdout）。零副作用模組，給 compare.mjs
// 與 host/tests/compare-evidence.test.mjs 共用（compare.mjs 本身被 import 就會執行 run()，見其檔頭）。
//
// - `scrubEvidenceText(text, env)`：把使用者路徑改寫成環境變數字樣，規則比照
//   host/tools/lib/EvidenceLog.psm1：TEMP、LOCALAPPDATA、APPDATA、USERPROFILE 由長到短比對，
//   分隔字元 `\`、JSON 跳脫後的 `\\`、`/` 都認得，不分大小寫；目錄名後面必須是分隔字元、引號、
//   空白或字串結尾（`alicex` 不會被當成 `alice`）。
// - `evidenceName(opts)`：`--evidence` 沒給名稱時依模式命名（self、self-<panel>、
//   self-<panel>-inject-diff、widget-<id>、widget-<id>-advance）；給了名稱就只留安全字元。
// - `writeEvidence(dir, name, lines, env)`：去識別後寫成 `<dir>/compare-<name>.log`（UTF-8、LF），
//   回傳完整路徑。

import fs from 'node:fs';
import path from 'node:path';

const VARS = ['TEMP', 'LOCALAPPDATA', 'APPDATA', 'USERPROFILE'];

function escapeRegex(s) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function dirPattern(dir) {
  const parts = dir.replace(/[\\/]+$/, '').split(/[\\/]+/).filter(Boolean);
  return parts.map(escapeRegex).join('(?:\\\\+|/)');
}

export function scrubEvidenceText(text, env = process.env) {
  const rules = VARS.map((name) => ({ name, dir: env[name] }))
    .filter((r) => typeof r.dir === 'string' && r.dir.trim() !== '')
    .sort((a, b) => b.dir.length - a.dir.length)
    .map((r) => ({
      re: new RegExp(`${dirPattern(r.dir)}(?=[\\\\/"'\\s]|$)`, 'gi'),
      to: `%${r.name}%`,
    }));
  let out = String(text);
  for (const { re, to } of rules) out = out.replace(re, to);
  return out;
}

function safeName(s) {
  return String(s)
    .replace(/[^A-Za-z0-9_.-]+/g, '-')
    .replace(/\.{2,}/g, '')
    .replace(/^[-.]+|[-.]+$/g, '');
}

export function evidenceName(opts) {
  if (typeof opts.evidence === 'string' && opts.evidence !== '') {
    const n = safeName(opts.evidence);
    if (n) return n;
  }
  if (opts.widget) return `widget-${safeName(opts.widget)}${opts.advanceTo ? '-advance' : ''}`;
  let n = opts.panel ? `self-${safeName(opts.panel)}` : 'self';
  if (opts.injectDiff) n += '-inject-diff';
  return n;
}

export function writeEvidence(dir, name, lines, env = process.env) {
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `compare-${name}.log`);
  const body = lines.map((l) => scrubEvidenceText(l, env)).join('\n') + '\n';
  fs.writeFileSync(file, body, 'utf8');
  return file;
}
