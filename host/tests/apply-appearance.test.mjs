// host/tests/apply-appearance.test.mjs
//
// 回歸測試：host/ui/common.js 的 `applyAppearance`（task 5.2；design.md D4「settings 事件」；
// specs/widget-host-lifecycle「設定持久化」Scenario「修改透明度」：所有小工具立即套用；
// specs/widget-host-windows「小工具外觀模式」Scenario「切換為純色模式」）。
//
// 這是十個小工具（clock／macro／fixed／custom1–5）在 `mount()` 與 `ctx.onSettings` 都會呼叫
// 的共用函式，行為錯了會讓「改透明度／主題色 → 所有小工具立即套用」這個 spec 要求整批失效，
// 值得單獨鎖住。用假的 `target`（只實作 `setProperty`）取代 `document.documentElement.style`，
// 不需要 jsdom；`applyAppearance` 的預設參數只在「呼叫端沒傳 target」時才會touch `document`，
// 本檔一律明確傳入假 target，`import` 本身不會因為 Node 沒有 `document` 而出錯。
//
// 執行：node host/tests/apply-appearance.test.mjs
// 通過條件：全部斷言成立，缺一個就 FAIL、exit 1。

import { applyAppearance } from '../ui/common.js';

let failures = 0;

function assertEqual(actual, expected, label) {
  const ok = actual === expected;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}（實際=${JSON.stringify(actual)} 期望=${JSON.stringify(expected)}）`);
  if (!ok) failures++;
}

function fakeTarget() {
  const props = new Map();
  return {
    setProperty(name, value) {
      props.set(name, value);
    },
    get(name) {
      return props.get(name);
    },
    has(name) {
      return props.has(name);
    },
  };
}

// ── 正常值：opacity／accent_color 都套用 ────────────────────────────────────────
{
  const t = fakeTarget();
  applyAppearance({ opacity: 0.7, accent_color: '#ff8800' }, t);
  assertEqual(t.get('--panel-o'), '0.7', '正常值：opacity 套用到 --panel-o');
  assertEqual(t.get('--accent'), '#ff8800', '正常值：accent_color 套用到 --accent');
}

// ── opacity 超出 0–1 範圍時夾限 ───────────────────────────────────────────────
{
  const t = fakeTarget();
  applyAppearance({ opacity: 1.5, accent_color: '#112233' }, t);
  assertEqual(t.get('--panel-o'), '1', 'opacity 超過 1 時夾限為 1');
}
{
  const t = fakeTarget();
  applyAppearance({ opacity: -0.3 }, t);
  assertEqual(t.get('--panel-o'), '0', 'opacity 小於 0 時夾限為 0');
}

// ── 型別不合法／缺漏：不寫入，保留呼叫端原有的 CSS 值 ─────────────────────────────
{
  const t = fakeTarget();
  applyAppearance({ opacity: 'not-a-number', accent_color: '#abc123' }, t);
  assertEqual(t.has('--panel-o'), false, 'opacity 型別不合法時不覆寫 --panel-o');
  assertEqual(t.get('--accent'), '#abc123', '同一筆 payload 裡其餘合法欄位仍照常套用');
}
{
  const t = fakeTarget();
  applyAppearance({ accent_color: 'e0aa54' }, t); // 缺 # 前綴，格式不合法
  assertEqual(t.has('--accent'), false, 'accent_color 缺 # 前綴時不覆寫 --accent');
}
{
  const t = fakeTarget();
  applyAppearance({ accent_color: '#zzzzzz' }, t); // 非 hex 字元
  assertEqual(t.has('--accent'), false, 'accent_color 含非 hex 字元時不覆寫 --accent');
}

// ── settings 為 null／undefined：整個函式是 no-op，不丟例外 ─────────────────────
{
  const t = fakeTarget();
  applyAppearance(null, t);
  applyAppearance(undefined, t);
  assertEqual(t.has('--panel-o') || t.has('--accent'), false, 'settings 為 null/undefined 時完全不寫入');
}

// ── 只給其中一個欄位：另一個保持不動 ─────────────────────────────────────────────
{
  const t = fakeTarget();
  applyAppearance({ opacity: 0.42 }, t);
  assertEqual(t.get('--panel-o'), '0.42', '只給 opacity 時正確套用');
  assertEqual(t.has('--accent'), false, '只給 opacity 時不動 --accent');
}

if (failures > 0) {
  console.error(`\n${failures} 項失敗`);
  process.exit(1);
}
console.log('\n全部通過');
