// host/tests/settings-wallpaper.test.mjs
//
// dynamic-wallpaper task 4.8：設定視窗動態桌布區塊的純邏輯（host/ui/settings-wallpaper.mjs）。
// - 休市表快過期：以頁面同一套 mergeThemeConfig（逐層合併）＋holidayExpiryReminder 判定（spec
//   wallpaper-themes「休市表快過期」；ledger：任一市場 through 早於明年 12/31 即算缺）。
// - 主題選單順序與文字、狀態提示（等待資料中）、Windows 備份提示、焦點提示文字照 spec。
//
// 執行：node --test host/tests/settings-wallpaper.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  THEME_OPTIONS,
  BACKUP_HINT,
  SPOTLIGHT_WARNING,
  holidayReminder,
  holidayNoticeText,
  statusNotices,
  localDateString,
} from '../ui/settings-wallpaper.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const BUILTIN = JSON.parse(
  readFileSync(path.join(__dirname, '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'),
);

/** 內建預設的市場表（id、名稱）＋固定的休市表，不隨日後更新預設檔而改變：TPE 只有 2026；HKG 的
 * 2027 只到 10/08（through 早於 2027-12-31 也算缺）；其餘四個市場 2026、2027 齊全。 */
const DEFAULTS = (() => {
  const year = (y, through = `${y}-12-31`) => ({ through, days: [] });
  const holidays = {};
  for (const { id } of BUILTIN.markets) {
    holidays[id] = { 2026: year(2026), 2027: year(2027) };
  }
  holidays.TPE = { 2026: year(2026) };
  holidays.HKG = { 2026: year(2026), 2027: year(2027, '2027-10-08') };
  return { ...BUILTIN, holidays };
})();

/** 把每個市場都補上 `year` 年（through 為該年 12/31）的使用者設定檔。 */
function withYearFor(ids, year) {
  const holidays = {};
  for (const id of ids) {
    holidays[id] = { ...DEFAULTS.holidays[id], [year]: { through: `${year}-12-31`, days: [] } };
  }
  return { holidays };
}

const ALL_IDS = DEFAULTS.markets.map((m) => m.id);

// DEFAULTS：缺明年的是 TPE、HKG（見上方）。
test('休市表：12/01 不提示、12/02 提示（預設缺明年的是 TPE、HKG）', () => {
  assert.equal(holidayReminder(DEFAULTS, {}, '2026-12-01'), null);
  const r = holidayReminder(DEFAULTS, {}, '2026-12-02');
  assert.deepEqual(r?.markets.map((m) => m.id), ['TPE', 'HKG']);
  assert.equal(r.year, 2027);
  assert.equal(r.markets[0].name, '台北');
  assert.equal(r.markets[1].name, '香港');
});

test('休市表：每個市場都已有下一年度就不提示', () => {
  assert.equal(holidayReminder(DEFAULTS, withYearFor(ALL_IDS, 2027), '2026-12-05'), null);
});

test('休市表：任一市場缺下一年度即提示（只缺那一個）', () => {
  // 補齊 TPE 的 2027；HKG 的 2027 仍只到 10/08 → 只剩 HKG。
  const r = holidayReminder(DEFAULTS, withYearFor(['TPE'], 2027), '2026-12-20');
  assert.deepEqual(r?.markets.map((m) => m.id), ['HKG']);
  assert.equal(r.markets[0].name, '香港');
});

test('休市表：跨年 1 月不提示', () => {
  assert.equal(holidayReminder(DEFAULTS, {}, '2027-01-15'), null);
  assert.equal(holidayReminder(DEFAULTS, {}, '2027-01-01'), null);
});

test('休市表：使用者檔只寫部分市場時，其餘市場以內建預設逐層補齊（不是頂層整個取代）', () => {
  // 只補 TPE 2027：頂層取代會讓其他市場「消失」而不提示；逐層合併後其他市場仍是預設值（已有 2027）。
  const r = holidayReminder(DEFAULTS, withYearFor(['TPE', 'HKG'], 2027), '2026-12-05');
  assert.equal(r, null, '其他市場的預設已含 2027，TPE、HKG 也補上了');
  // 反之只寫 TYO 2026（沒有 2027）：逐層合併時 TYO 仍保有預設的 2027，只有 TPE 缺。
  const r2 = holidayReminder(DEFAULTS, { holidays: { TYO: { 2026: DEFAULTS.holidays.TYO['2026'] } } }, '2026-12-05');
  assert.deepEqual(r2?.markets.map((m) => m.id), ['TPE', 'HKG']);
});

test('休市表：設定檔不是物件時以內建預設判定', () => {
  assert.deepEqual(holidayReminder(DEFAULTS, null, '2026-12-05')?.markets.map((m) => m.id), ['TPE', 'HKG']);
});

test('休市表提示文字照 spec，附市場與設定檔路徑', () => {
  const text = holidayNoticeText(
    { year: 2027, markets: [{ id: 'TPE', name: '台北' }] },
    'C:\\x\\wallpaper-config.json',
  );
  assert.match(text, /請更新明年的休市表/);
  assert.match(text, /台北（TPE）/);
  assert.match(text, /2027/);
  assert.ok(text.includes('C:\\x\\wallpaper-config.json'));
});

// task 6.4（審查 R2b nit）：下一年度只有部分月份的市場（HKG 2027 只到 10/08）不得說成「沒有資料」。
test('休市表提示文字：下一年度不完整的市場寫「只到 …」，完全沒有的才寫「沒有 … 年的資料」', () => {
  const r = holidayReminder(DEFAULTS, {}, '2026-12-05');
  assert.deepEqual(
    r?.markets.map((m) => [m.id, m.through]),
    [
      ['TPE', '2026-12-31'],
      ['HKG', '2027-10-08'],
    ],
  );
  const text = holidayNoticeText(r, '');
  assert.match(text, /請更新明年的休市表/);
  assert.ok(text.includes('香港（HKG）只到 2027-10-08'), text);
  assert.ok(!/香港（HKG）[^，、；]*沒有/.test(text), text);
  assert.ok(text.includes('台北（TPE）沒有 2027 年的資料'), text);
  assert.ok(text.includes('還沒有涵蓋到 2027 年底'), text);
});

test('主題選單：不接管＋五套主題，順序與名稱照 spec', () => {
  assert.deepEqual(
    THEME_OPTIONS.map((o) => [o.id, o.label]),
    [
      ['none', '不接管'],
      ['ridgeline', '脊線'],
      ['tearoff', '撕日曆'],
      ['astrolabe', '星盤'],
      ['contour', '等高線'],
      ['skyline', '天際線'],
    ],
  );
});

test('Windows 備份提示與焦點提示文字照 spec', () => {
  assert.ok(BACKUP_HINT.includes('記住我的喜好設定'));
  assert.ok(BACKUP_HINT.includes('設定 > 帳戶 > Windows 備份'));
  assert.ok(BACKUP_HINT.includes('新裝置'));
  assert.equal(SPOTLIGHT_WARNING, '停止接管時無法自動切回 Windows 焦點，需要到 Windows 設定手動切回');
});

test('狀態提示：等待資料中說明缺什麼、資料出現後自動開始', () => {
  const notices = statusNotices({
    coordinator_running: true,
    state: 'waiting_for_data',
    waiting_for: '加權指數日 K',
  });
  assert.equal(notices.length, 1);
  assert.match(notices[0].text, /^等待資料中/);
  assert.match(notices[0].text, /加權指數日 K/);
  assert.match(notices[0].text, /自動開始/);
});

test('狀態提示：一般狀態（不接管、最新、重畫中）沒有提示；協調迴圈沒在跑也沒有', () => {
  for (const state of ['idle', 'up_to_date', 'redrawing', 'starting', 'paused']) {
    assert.deepEqual(statusNotices({ coordinator_running: true, state }), [], state);
  }
  assert.deepEqual(statusNotices({ coordinator_running: false, state: 'waiting_for_data', waiting_for: 'x' }), []);
});

test('狀態提示：封鎖顯示原因（錯誤樣式）', () => {
  const [n] = statusNotices({ coordinator_running: true, state: 'blocked', blocked: 'Corrupt' });
  assert.equal(n.kind, 'err');
  assert.match(n.text, /Corrupt/);
});

test('本機日期字串', () => {
  assert.equal(localDateString(new Date(2026, 11, 2, 23, 59)), '2026-12-02');
  assert.equal(localDateString(new Date(2027, 0, 5, 0, 0)), '2027-01-05');
});
