// host/tests/compare-evidence.test.mjs
//
// 回歸測試：host/tests/compare/evidence.mjs（fix F7，批次 A：compare.mjs 的結果只輸出到 stdout，
// 沒有證據檔）。compare.mjs 加 `--evidence [名稱]` 後把同一份輸出寫成
// host/tools/evidence/compare-<名稱>.log，寫出前把使用者路徑改寫成環境變數字樣（規則比照
// host/tools/lib/EvidenceLog.psm1：TEMP、LOCALAPPDATA、APPDATA、USERPROFILE 由長到短，分隔字元
// `\`、`\\`、`/` 都認得，不分大小寫）。
//
// 執行：node host/tests/compare-evidence.test.mjs
// 通過條件：全部斷言成立，缺一個就 FAIL、exit 1。

import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { scrubEvidenceText, evidenceName, writeEvidence } from './compare/evidence.mjs';

let failures = 0;
function assertEqual(actual, expected, label) {
  const ok = JSON.stringify(actual) === JSON.stringify(expected);
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}（實際=${JSON.stringify(actual)} 期望=${JSON.stringify(expected)}）`);
  if (!ok) failures++;
}

const env = {
  USERPROFILE: 'C:\\Users\\alice',
  APPDATA: 'C:\\Users\\alice\\AppData\\Roaming',
  LOCALAPPDATA: 'C:\\Users\\alice\\AppData\\Local',
  TEMP: 'C:\\Users\\alice\\AppData\\Local\\Temp',
};

// ── 1. 去識別 ─────────────────────────────────────────────────────────────────
assertEqual(
  scrubEvidenceText('fixture=C:\\Users\\alice\\AppData\\Local\\Temp\\fc\\tw.json', env),
  'fixture=%TEMP%\\fc\\tw.json',
  'TEMP 先於 LOCALAPPDATA 比對',
);
assertEqual(scrubEvidenceText('d=C:\\Users\\alice\\AppData\\Local\\x', env), 'd=%LOCALAPPDATA%\\x', 'LOCALAPPDATA');
assertEqual(scrubEvidenceText('d=C:\\Users\\alice\\AppData\\Roaming\\x', env), 'd=%APPDATA%\\x', 'APPDATA');
assertEqual(scrubEvidenceText('repo=C:\\Users\\alice\\src\\fc', env), 'repo=%USERPROFILE%\\src\\fc', 'USERPROFILE');
assertEqual(scrubEvidenceText('url=file:///C:/Users/alice/src/a.html', env), 'url=file:///%USERPROFILE%/src/a.html', '斜線分隔');
assertEqual(scrubEvidenceText('{"p":"C:\\\\Users\\\\alice\\\\src"}', env), '{"p":"%USERPROFILE%\\\\src"}', 'JSON 跳脫的雙反斜線');
assertEqual(scrubEvidenceText('P=c:\\users\\ALICE\\src', env), 'P=%USERPROFILE%\\src', '不分大小寫');
assertEqual(scrubEvidenceText('D:\\projects\\finance-calendar\\x', env), 'D:\\projects\\finance-calendar\\x', '不是使用者目錄者不動');
assertEqual(scrubEvidenceText('C:\\Users\\alicex\\y', env), 'C:\\Users\\alicex\\y', '名稱前綴相同的別的目錄不動');
assertEqual(scrubEvidenceText('a', {}), 'a', '環境變數都沒有時原樣');

// ── 2. 檔名 ───────────────────────────────────────────────────────────────────
assertEqual(evidenceName({ evidence: true, widget: null, panel: null, advanceTo: null, injectDiff: null }), 'self', '自比對五面板 → self');
assertEqual(evidenceName({ evidence: true, widget: null, panel: 'dynamic', advanceTo: null, injectDiff: null }), 'self-dynamic', '自比對單一面板');
assertEqual(evidenceName({ evidence: true, widget: null, panel: 'clock', advanceTo: null, injectDiff: 'clockTime:x' }), 'self-clock-inject-diff', '反例');
assertEqual(evidenceName({ evidence: true, widget: 'macro', panel: null, advanceTo: null, injectDiff: null }), 'widget-macro', '跨版本');
assertEqual(evidenceName({ evidence: true, widget: 'macro', panel: null, advanceTo: '2026-09-28T04:01:00+08:00', injectDiff: null }), 'widget-macro-advance', '跨日');
assertEqual(evidenceName({ evidence: 'batchA run1', widget: 'clock' }), 'batchA-run1', '自訂名稱：不安全字元換成 -');
assertEqual(evidenceName({ evidence: '../../etc' }), 'etc', '自訂名稱不得跳出證據目錄');

// ── 3. 寫檔（暫存目錄，不碰 repo 的 evidence）─────────────────────────────────
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'compare-evidence-'));
try {
  const file = writeEvidence(dir, 'widget-clock', ['[compare] --fixture=C:\\Users\\alice\\AppData\\Local\\Temp\\f.json', 'PASS'], env);
  assertEqual(path.basename(file), 'compare-widget-clock.log', '檔名 compare-<名稱>.log');
  const text = fs.readFileSync(file, 'utf8');
  assertEqual(text, '[compare] --fixture=%TEMP%\\f.json\nPASS\n', '內容已去識別、LF 行尾');
} finally {
  fs.rmSync(dir, { recursive: true, force: true });
}

if (failures > 0) {
  console.log(`\n[compare-evidence] FAIL：${failures} 項`);
  process.exit(1);
}
console.log('\n[compare-evidence] 全部通過');
