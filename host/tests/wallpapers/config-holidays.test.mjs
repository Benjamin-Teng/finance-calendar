// host/tests/wallpapers/config-holidays.test.mjs
//
// 休市表語意與讀取端檢查（lib/config-holidays.mjs）的單元測試（task 3.2 修正輪）：
// `node --test "host/tests/wallpapers/*.test.mjs"`。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { mergeConfig } from '../../ui/wallpapers/lib/core.mjs';
import {
  holidayExpiryReminder,
  holidayOn,
  holidayOnAt,
  PHRASE_MAX_CHARS,
  isRealDate,
  lunarNewYearOf,
  unverifiedHolidayYears,
  validateThemeConfig,
} from '../../ui/wallpapers/lib/config-holidays.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_PATH = path.join(HERE, '..', '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json');
const CFG = JSON.parse(readFileSync(DEFAULT_PATH, 'utf8'));
const clone = (v) => JSON.parse(JSON.stringify(v));

// ── 預設檔本身 ─────────────────────────────────────────────────────────────────────────

test('validateThemeConfig：內建預設檔沒有任何警告', () => {
  assert.deepEqual(validateThemeConfig(CFG), []);
});

test('預設檔：每一筆休市日都有 src（schema 允許使用者省略，內建預設值不可省略）', () => {
  for (const [mk, years] of Object.entries(CFG.holidays)) {
    for (const [year, y] of Object.entries(years)) {
      for (const d of y.days) assert.ok(typeof d.src === 'string' && d.src, `${mk} ${year} ${d.date} 缺 src`);
    }
  }
});

// ── 裁定 a：date 是交易所當地日期 ──────────────────────────────────────────────────────

test('holidayOnAt：以交易所當地日期查表，不是台北或本機日期', () => {
  // 台北 2026-11-27 07:00（UTC 2026-11-26 23:00）：紐約仍是 11-26 18:00（感恩節，休市）
  const t1 = Date.UTC(2026, 10, 26, 23, 0);
  assert.equal(holidayOnAt(CFG, 'NYC', t1).status, 'closed');
  assert.equal(holidayOnAt(CFG, 'NYC', t1).name, '感恩節');
  // 同一刻台北已是 11-27（不在台北休市表內），東京 11-27 08:00
  assert.equal(holidayOnAt(CFG, 'TPE', t1).status, 'open');
  // 紐約 11-27 當地日期才是半日市（感恩節隔天，13:00 收）
  const t2 = Date.UTC(2026, 10, 27, 15, 0);
  assert.deepEqual(
    (({ status, close }) => ({ status, close }))(holidayOnAt(CFG, 'NYC', t2)),
    { status: 'half', close: '13:00' },
  );
  // 台北 2026-12-25 00:30 = UTC 12-24 16:30 = 紐約 12-24 11:30（半日市）；台北當天（12-25）本身是行憲紀念日
  const t3 = Date.UTC(2026, 11, 24, 16, 30);
  assert.equal(holidayOnAt(CFG, 'NYC', t3).status, 'half');
  assert.equal(holidayOnAt(CFG, 'TPE', t3).status, 'closed');
});

// ── 裁定 b、c：明列一律生效；through 之後未列＝未知（畫成交易日） ──────────────────────────

test('holidayOn：明列的日期不受 through 限制；未列且晚於 through 為 unknown；through 之內未列為 open', () => {
  const cfg = clone(CFG);
  // HKG 2027 through = 2027-10-08：之內未列＝open，之後未列＝unknown
  assert.equal(holidayOn(cfg, 'HKG', '2027-10-07').status, 'open');
  assert.equal(holidayOn(cfg, 'HKG', '2027-10-11').status, 'unknown');
  assert.equal(holidayOn(cfg, 'HKG', '2027-12-27').status, 'unknown');
  // 年度鍵不存在（TPE 2027）＝未知
  assert.equal(holidayOn(cfg, 'TPE', '2027-03-01').status, 'unknown');
  // 使用者補了 through 之後的休市日 → 照算休市
  cfg.holidays.HKG['2027'].days.push({ date: '2027-12-27', name: '聖誕節翌日' });
  assert.deepEqual(holidayOn(cfg, 'HKG', '2027-12-27'), { status: 'closed', name: '聖誕節翌日' });
});

// ── 裁定 e：使用者在 through 之後新增休市日，經 mergeConfig 不會被靜默丟掉 ─────────────────

test('mergeConfig：使用者補 through 之後的休市日、沒寫 through，日期保留且仍算休市，並收到警告', () => {
  const user = {
    holidays: { HKG: { 2027: { days: [{ date: '2027-12-27', name: '聖誕節翌日' }] } } },
  };
  const { value, warnings } = mergeConfig(CFG, user);
  assert.deepEqual(warnings, []);
  // days 是陣列、整份取代：使用者那一筆還在；through 由預設補上（2027-10-08）
  assert.equal(value.holidays.HKG['2027'].through, '2027-10-08');
  assert.deepEqual(value.holidays.HKG['2027'].days, user.holidays.HKG['2027'].days);
  // 沒被丟掉：查表仍是休市
  assert.equal(holidayOn(value, 'HKG', '2027-12-27').status, 'closed');
  // 並提醒使用者延伸 through
  const w = validateThemeConfig(value);
  assert.equal(w.length, 1);
  assert.match(w[0], /config\.holidays\.HKG\.2027.*晚於 through.*延伸/);
  // 使用者同時延伸 through → 無警告
  user.holidays.HKG['2027'].through = '2027-12-31';
  const ok = mergeConfig(CFG, user).value;
  assert.deepEqual(validateThemeConfig(ok), []);
});

test('mergeConfig：使用者新增整個新年度（含自己的 through）不影響其他年度', () => {
  const user = {
    holidays: { TPE: { 2027: { through: '2027-12-31', days: [{ date: '2027-01-01', name: '開國紀念日' }] } } },
  };
  const { value } = mergeConfig(CFG, user);
  assert.deepEqual(validateThemeConfig(value), []);
  assert.equal(holidayOn(value, 'TPE', '2027-01-01').status, 'closed');
  assert.equal(holidayOn(value, 'TPE', '2027-01-04').status, 'open');
  assert.equal(holidayOn(value, 'TPE', '2026-02-17').status, 'closed');
});

// ── 裁定 d：年底提醒 ───────────────────────────────────────────────────────────────────

test('holidayExpiryReminder：距 12/31 不足 30 天（12/02 起）且最晚 through 早於明年 12/31 才提醒', () => {
  // 預設檔：TPE 無 2027、HKG 2027 只到 10-08、其餘市場 2027 完整
  assert.deepEqual(holidayExpiryReminder(CFG, '2026-12-05').sort(), ['HKG', 'TPE']);
  assert.deepEqual(holidayExpiryReminder(CFG, '2026-12-02').sort(), ['HKG', 'TPE'], '12/02 距 12/31 29 天，不足 30 天，提醒');
  assert.deepEqual(holidayExpiryReminder(CFG, '2026-12-01'), [], '12/01 距 12/31 剛好 30 天，不算「不足」，不提醒');
  assert.deepEqual(holidayExpiryReminder(CFG, '2026-11-30'), [], '距年底 31 天，不提醒');
  assert.deepEqual(holidayExpiryReminder(CFG, '2026-06-15'), []);
  // 2027 年底：所有市場都沒有 2028 → 全部提醒
  assert.equal(holidayExpiryReminder(CFG, '2027-12-20').length, 6);
  // 使用者補齊後不再提醒
  const cfg = clone(CFG);
  cfg.holidays.HKG['2027'].through = '2027-12-31';
  cfg.holidays.TPE['2027'] = { through: '2027-12-31', days: [] };
  assert.deepEqual(holidayExpiryReminder(cfg, '2026-12-05'), []);
  // 壞輸入不拋錯
  assert.deepEqual(holidayExpiryReminder(CFG, 'garbage'), []);
  assert.deepEqual(holidayExpiryReminder(undefined, '2026-12-05'), []);
});

// ── 裁定 5：待複核旗標 ────────────────────────────────────────────────────────────────

test('unverifiedHolidayYears：KRX 2027 標為待複核，其餘年度視為已核對', () => {
  assert.deepEqual(unverifiedHolidayYears(CFG), [{ market: 'SEL', year: '2027' }]);
  assert.equal(CFG.holidays.SEL['2027'].through, '2027-12-31', 'through 照官方資料，不因待複核而縮短');
  assert.deepEqual(unverifiedHolidayYears({}), []);
});

// ── validateThemeConfig 的各類錯誤 ─────────────────────────────────────────────────────

test('validateThemeConfig：不存在的日期、年度鍵與年份不符、重複、晚於 through、市場打錯、半日市缺 close', () => {
  const cfg = clone(CFG);
  const nyc = cfg.holidays.NYC['2026'].days;
  nyc.push({ date: '2026-02-30', name: '不存在' });
  nyc.push({ date: '2027-01-01', name: '放錯年度' });
  nyc.push({ date: '2026-01-01', name: '重複' });
  delete nyc.find((d) => d.half).close;
  cfg.holidays.TPI = { 2026: { through: '2026-12-31', days: [] } };
  cfg.holidays.LON['2026'].days.push({ date: '2026-12-31', name: '亂入', src: 'no-such-source' });
  const w = validateThemeConfig(cfg).join('\n');
  assert.match(w, /NYC\.2026\.days\[\d+\]\.date（2026-02-30）不是 YYYY-MM-DD 的真實日期/);
  assert.match(w, /NYC\.2026\.days\[\d+\]\.date（2027-01-01）與年度鍵 2026 不符/);
  assert.match(w, /NYC\.2026\.days\[\d+\]\.date（2026-01-01）重複/);
  assert.match(w, /NYC\.2026\.days\[\d+\]：半日市需要 HH:MM 格式的 close/);
  assert.match(w, /holidays\.TPI：市場不在 markets 內/);
  assert.match(w, /LON\.2026\.days\[\d+\]\.src（no-such-source）不在 holidaySources 內/);
});

test('validateThemeConfig：through 不合法、財報截止日不存在、門檻非正數；壞輸入不拋錯', () => {
  const cfg = clone(CFG);
  cfg.holidays.TYO['2026'].through = '2026-13-01';
  cfg.holidays.TYO['2027'].through = '2026-12-31';
  cfg.earningsWeek.deadlines = ['02-30', '05-15', '8-14'];
  cfg.thresholds.shortRatio = 0;
  cfg.thresholds.maintenanceRatio = -150;
  const w = validateThemeConfig(cfg).join('\n');
  assert.match(w, /TYO\.2026\.through 不是 YYYY-MM-DD 的真實日期/);
  assert.match(w, /TYO\.2027\.through（2026-12-31）不在 2027 年/);
  assert.match(w, /deadlines\[0\]（02-30）不是 MM-DD 的真實日期/);
  assert.match(w, /deadlines\[2\]（8-14）不是 MM-DD 的真實日期/);
  assert.match(w, /thresholds\.shortRatio（0）必須是正數/);
  assert.match(w, /thresholds\.maintenanceRatio（-150）必須是正數/);

  for (const bad of [null, undefined, 42, 'x', [], { holidays: 5 }, { holidays: { TPE: 3 } }, { holidays: { TPE: { 2026: { days: 1 } } } }]) {
    assert.doesNotThrow(() => validateThemeConfig(bad));
  }
  assert.deepEqual(validateThemeConfig({}), []);
  assert.ok(validateThemeConfig(null).length > 0);
});

test('isRealDate', () => {
  assert.equal(isRealDate('2026-02-28'), true);
  assert.equal(isRealDate('2028-02-29'), true);
  assert.equal(isRealDate('2026-02-29'), false);
  assert.equal(isRealDate('2026-2-1'), false);
  assert.equal(isRealDate(20260101), false);
});

// ── 農曆正月初一表與撕日曆字句長度（task 3.4 修正輪 1）──────────────────────────────────

test('lunarNewYearOf：預設檔 2026–2028 的官方日期；沒列的年度或不合理的日期回傳 null', () => {
  assert.equal(lunarNewYearOf(CFG, 2026), '2026-02-17');
  assert.equal(lunarNewYearOf(CFG, '2027'), '2027-02-06');
  assert.equal(lunarNewYearOf(CFG, 2028), '2028-01-26');
  assert.equal(lunarNewYearOf(CFG, 2029), null);
  assert.equal(lunarNewYearOf({}, 2026), null);
  const c = clone(CFG);
  for (const bad of ['2027-02-30', '2026-02-06', '2027-01-20', '2027-02-21', 20270206, null]) {
    c.lunarNewYear.dates['2027'] = { date: bad, src: 'cwa-2027' };
    assert.equal(lunarNewYearOf(c, 2027), null, String(bad));
  }
  c.lunarNewYear.dates['2027'] = '2027-02-06'; // 少一層物件
  assert.equal(lunarNewYearOf(c, 2027), null);
});

test('validateThemeConfig：農曆正月初一表日期不合理、出處解不開、型別錯都回警告', () => {
  const c = clone(CFG);
  c.lunarNewYear.dates['2027'] = { date: '2027-03-01', src: 'cwa-2027' };
  c.lunarNewYear.dates['2029'] = { date: '2029-02-13', src: 'no-such-source' };
  c.lunarNewYear.dates['2030'] = 'oops';
  const w = validateThemeConfig(c);
  assert.ok(w.some((x) => x.startsWith('config.lunarNewYear.dates.2027.date（2027-03-01）')), w.join('\n'));
  assert.ok(w.some((x) => x.includes('dates.2029.src（no-such-source）不在 lunarNewYear.sources 內')), w.join('\n'));
  assert.ok(w.some((x) => x === 'config.lunarNewYear.dates.2030 型別應為物件'), w.join('\n'));
  assert.equal(w.length, 3, w.join('\n'));
  assert.deepEqual(validateThemeConfig({ lunarNewYear: [] }), ['config.lunarNewYear 型別應為物件']);
});

test(`validateThemeConfig：撕日曆字句超過 ${PHRASE_MAX_CHARS} 字回警告（剛好 20 字不警告），預設字句都不超過`, () => {
  assert.equal(PHRASE_MAX_CHARS, 20);
  const c = clone(CFG);
  const s20 = '一'.repeat(20);
  const s21 = '二'.repeat(21);
  c.phrases.yiPool = [...c.phrases.yiPool, s20, s21];
  c.phrases.closedDay.ji = [...c.phrases.closedDay.ji, s21];
  c.phrases.marginWarning.ji = s21;
  c.phrases.events.tsmcEarnings = s21;
  c.orderSeasons = [...c.orderSeasons, { label: s21, months: [1] }];
  const w = validateThemeConfig(c);
  const at = w.map((x) => x.split('（')[0]);
  assert.deepEqual(at, [
    'config.phrases.closedDay.ji[2]',
    'config.phrases.marginWarning.ji',
    'config.phrases.events.tsmcEarnings',
    `config.phrases.yiPool[${CFG.phrases.yiPool.length + 1}]`,
    `config.orderSeasons[${CFG.orderSeasons.length}].label`,
  ]);
  assert.ok(w.every((x) => x.includes('（21 字）超過 20 字')));
  // 全形空格算一個字；碼位計數（不是 UTF-16 長度）
  const c2 = clone(CFG);
  c2.phrases.jiPool = ['𠀀'.repeat(20)];
  assert.deepEqual(validateThemeConfig(c2), []);
});

// ── 4.1（3.2 延後項 N3）：查表函式遇到 mergeConfig 會放行的壞元素不拋錯，略過並記警告 ──────────

const hasWarning = (sink, needle) => [...sink].some((w) => w.includes(needle));

test('holidayOn：days 裡的 null 或非物件元素略過並記警告，其餘照查', () => {
  const cfg = clone(CFG);
  cfg.holidays.TPE['2026'].days.unshift(null, 7, 'x', ['2026-01-01']);
  const warnings = [];
  assert.equal(holidayOn(cfg, 'TPE', '2026-01-01', warnings).status, 'closed');
  assert.ok(hasWarning(warnings, 'holidays.TPE.2026.days'), warnings.join('\n'));
  // 經 mergeConfig（陣列整份取代）放行的新年度 days:[null]
  const merged = mergeConfig(CFG, { holidays: { TPE: { 2027: { through: '2027-12-31', days: [null] } } } }).value;
  const w2 = new Set();
  assert.equal(holidayOn(merged, 'TPE', '2027-03-01', w2).status, 'open');
  assert.ok(hasWarning(w2, 'holidays.TPE.2027.days'));
  // 不給收集器也不拋錯
  assert.doesNotThrow(() => holidayOn(merged, 'TPE', '2027-03-01'));
});

test('holidayOnAt：markets 含 null／非陣列、tz 打錯或缺、nowMs 不是有限數字 → 不拋錯，回 unknown 並記警告', () => {
  const t = Date.UTC(2026, 11, 24, 16, 30); // 台北 12-25（行憲紀念日）

  const withNull = mergeConfig(CFG, { markets: [null, ...CFG.markets] }).value;
  const w1 = [];
  assert.equal(holidayOnAt(withNull, 'TPE', t, w1).status, 'closed', '壞元素略過後仍查得到 TPE');
  assert.ok(hasWarning(w1, 'markets[0]'), w1.join('\n'));

  for (const markets of [{ TPE: {} }, 'TPE', 5]) {
    const w = [];
    assert.deepEqual(holidayOnAt({ ...CFG, markets }, 'TPE', t, w), { status: 'unknown' });
    assert.ok(hasWarning(w, 'markets'), `${JSON.stringify(markets)}：${w.join('\n')}`);
  }

  for (const tz of ['Asia/Taipe', '', undefined, 42]) {
    const cfg = clone(CFG);
    cfg.markets.find((m) => m.id === 'TPE').tz = tz;
    const w = [];
    assert.deepEqual(holidayOnAt(cfg, 'TPE', t, w), { status: 'unknown' }, `tz=${String(tz)}`);
    assert.ok(hasWarning(w, 'tz'), `tz=${String(tz)}：${w.join('\n')}`);
  }

  for (const nowMs of [NaN, Infinity, undefined, '2026-12-25']) {
    const w = [];
    assert.deepEqual(holidayOnAt(CFG, 'TPE', nowMs, w), { status: 'unknown' }, `nowMs=${String(nowMs)}`);
    assert.ok(w.length > 0, `nowMs=${String(nowMs)} 應記警告`);
  }
  assert.doesNotThrow(() => holidayOnAt({ ...CFG, markets: [null] }, 'TPE', NaN));
});

test('holidayExpiryReminder：markets 含 null 或沒有 id 的元素略過並記警告', () => {
  const cfg = clone(CFG);
  cfg.markets = [null, { name: '沒有 id' }, ...cfg.markets];
  const warnings = [];
  const ids = holidayExpiryReminder(cfg, '2026-12-05', warnings);
  assert.deepEqual(ids, holidayExpiryReminder(CFG, '2026-12-05'), '壞元素不影響其他市場的判斷');
  assert.ok(hasWarning(warnings, 'markets[0]'), warnings.join('\n'));
  assert.ok(hasWarning(warnings, 'markets[1]'), warnings.join('\n'));
  assert.doesNotThrow(() => holidayExpiryReminder({ markets: [null] }, '2026-12-05'));
});

test('unverifiedHolidayYears：holidays 型別錯或年度為 null 不拋錯', () => {
  for (const holidays of ['abc', 5, null, [], { SEL: null }, { SEL: { 2027: null } }, { SEL: 'x' }]) {
    assert.deepEqual(unverifiedHolidayYears({ holidays }), [], JSON.stringify(holidays));
  }
});

test('validateThemeConfig：markets 型別錯、元素不是物件、id 缺、tz 不是 Intl 認得的時區都回警告', () => {
  const cfg = clone(CFG);
  cfg.markets.find((m) => m.id === 'TPE').tz = 'Asia/Taipe';
  cfg.markets.push(null, { name: '沒有 id', tz: 'Asia/Tokyo' }, { id: 'XXX' });
  const w = validateThemeConfig(cfg);
  const at = (i) => `config.markets[${i}]`;
  const tpeIndex = cfg.markets.findIndex((m) => m?.id === 'TPE');
  assert.ok(hasWarning(w, `${at(tpeIndex)}.tz（Asia/Taipe）`), w.join('\n'));
  assert.ok(hasWarning(w, `${at(6)} 型別應為物件`), w.join('\n'));
  assert.ok(hasWarning(w, `${at(7)}.id`), w.join('\n'));
  assert.ok(hasWarning(w, `${at(8)}.tz（undefined）`), w.join('\n'));
  assert.ok(hasWarning(validateThemeConfig({ markets: { TPE: {} } }), 'config.markets 型別應為陣列'));
  assert.deepEqual(validateThemeConfig(CFG), [], '內建預設仍然沒有警告');
});
