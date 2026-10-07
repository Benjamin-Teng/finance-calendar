// host/tests/wallpapers/tearoff-model.test.mjs
//
// 撕日曆（task 3.4）的「宜」「忌」規則、干支年、日期雜湊、單行縮字的單元測試。純函式在
// host/ui/wallpapers/lib/tearoff-model.mjs，不依賴 DOM。
// 執行：`node --test "host/tests/wallpapers/*.test.mjs"`。
//
// 字句一律取自內建預設檔（與頁面相同的來源），不在測試裡重打；另以 spec.md 原文核對預設檔的
// 定稿字句（spec 寫的「出去走走　陪陪家人」等必須與預設檔逐字相同）。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { parseInstant } from '../../ui/wallpapers/lib/core.mjs';
import {
  JI_SALT,
  MAX_YI,
  YI_SALT,
  addDays,
  computeTearoff,
  dateHash,
  dayText,
  fitFontPx,
  ganzhiYear,
  inEarningsWeek,
  localDate,
  marginAlert,
  monthText,
  orderSeasonLabels,
  pickPhrase,
  tpeClosed,
  transformedAabb,
  tsmcEarningsOn,
  weekdayText,
} from '../../ui/wallpapers/lib/tearoff-model.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, '..', '..', '..');
const CFG = JSON.parse(readFileSync(path.join(REPO, 'host', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json'), 'utf8'));
const SPEC = readFileSync(path.join(REPO, 'openspec', 'changes', 'dynamic-wallpaper', 'specs', 'wallpaper-themes', 'spec.md'), 'utf8');
const clone = (v) => JSON.parse(JSON.stringify(v));

const P = CFG.phrases;
const CLOSED_YI = [P.closedDay.yi];
const CLOSED_JI = P.closedDay.ji;
const MARGIN_JI = [P.marginWarning.ji];
const TSMC = P.events.tsmcEarnings;
const EWEEK = P.events.earningsWeek;
const season = (m) => CFG.orderSeasons.filter((s) => s.months.includes(m)).map((s) => s.label);

/** 一般資料：無事件、融資正常、TWSE 休市表空。 */
const NORMAL_MARGIN = { date: '2026-10-01', short_margin_ratio: 2.49, maintenance_ratio: 195.08 };
const data = (extra = {}) => ({ events: [], holidays: [], margin: { ...NORMAL_MARGIN }, ...extra });
const tsmcEvent = (date, type = 'conference') => ({ date, type, code: '2330', name: '台積電', note: '法說會' });
const model = (date, d = data(), cfg = CFG) => computeTearoff(cfg, d, date);

// ── 定稿字句：預設檔與 spec 原文一致 ──────────────────────────────────────────────────

test('定稿字句：預設檔的休市日宜忌、融資警示、事件名稱與 spec 原文逐字相同', () => {
  assert.ok(SPEC.includes(`固定顯示「${P.closedDay.yi}」`), '休市日「宜」');
  assert.ok(SPEC.includes(`顯示兩行「${P.closedDay.ji[0]}」「${P.closedDay.ji[1]}」`), '休市日「忌」兩行');
  assert.equal(P.closedDay.ji.length, 2);
  assert.ok(SPEC.includes(`時顯示「${P.marginWarning.ji}」`), '融資警示');
  assert.ok(SPEC.includes(`「宜」顯示「${TSMC}」與「電子旺季」兩項`), '台積電財報');
  assert.ok(SPEC.includes(`財報密集週（3/31、5/15、8/14、11/14`) && EWEEK === '財報密集週', '財報密集週');
  assert.deepEqual(season(7), ['電子旺季']);
  assert.deepEqual(season(10), ['年底備貨']);
});

// ── 日期、星期、月份、干支 ───────────────────────────────────────────────────────────

test('本機日期：依顯示時區換算（同一時刻台北 23:30＝東京隔天 00:30）', () => {
  const ms = parseInstant('2026-10-02T23:30', 'Asia/Taipei');
  assert.equal(localDate('Asia/Taipei', ms), '2026-10-02');
  assert.equal(localDate('Asia/Tokyo', ms), '2026-10-03');
  assert.equal(localDate('America/New_York', ms), '2026-10-02');
});

test('日期工具：addDays 跨月跨年、星期、中文月份、日', () => {
  assert.equal(addDays('2026-12-31', 1), '2027-01-01');
  assert.equal(addDays('2026-03-01', -1), '2026-02-28');
  assert.equal(addDays('2024-03-01', -1), '2024-02-29');
  assert.equal(weekdayText('2026-07-28'), '週二'); // 樣稿的日期
  assert.equal(weekdayText('2026-10-03'), '週六');
  assert.equal(weekdayText('2026-10-04'), '週日');
  assert.deepEqual(
    ['01', '02', '03', '04', '05', '06', '07', '08', '09', '10', '11', '12'].map((m) => monthText(`2026-${m}-15`)),
    ['一月', '二月', '三月', '四月', '五月', '六月', '七月', '八月', '九月', '十月', '十一月', '十二月'],
  );
  assert.equal(dayText('2026-07-28'), '28');
  assert.equal(dayText('2026-10-05'), '5');
});

test('干支年：以農曆新年為界（2026-02-16 除夕乙巳、02-17 正月初一丙午；2025-01-28／29 甲辰／乙巳）', () => {
  for (const cfg of [undefined, CFG]) {
    // 2025 不在設定表 → 兩種呼叫都走 Intl；2026 設定表與 Intl 一致
    assert.equal(ganzhiYear('2026-02-16', cfg), '乙巳年');
    assert.equal(ganzhiYear('2026-02-17', cfg), '丙午年');
    assert.equal(ganzhiYear('2025-01-28', cfg), '甲辰年');
    assert.equal(ganzhiYear('2025-01-29', cfg), '乙巳年');
    assert.equal(ganzhiYear('2026-07-28', cfg), '丙午年'); // 樣稿「丙午年　週二」
    assert.equal(ganzhiYear('2026-01-01', cfg), '乙巳年'); // 西曆新年不換干支
    assert.equal(ganzhiYear('2027-02-05', cfg), '丙午年'); // 2027 除夕
    assert.equal(ganzhiYear('2027-02-07', cfg), '丁未年');
  }
});

// 2027 年農曆新年的朔離午夜只差幾分鐘，ICU（Node／Edge 的 Intl 中國曆）把初一算成 02-07；
// 官方（中央氣象署 116 年日曆資料表、人事行政總處 116 年辦公日曆表附表 1）為 02-06。
// 設定檔 lunarNewYear 表收錄官方日期，有該年時以它換年（task 3.4 修正輪 1）。
test('干支年：2027-02-06（官方正月初一）依設定檔為丁未年；不給設定時是 ICU 的丙午年', () => {
  assert.equal(CFG.lunarNewYear.dates['2027'].date, '2027-02-06');
  assert.equal(ganzhiYear('2027-02-06', CFG), '丁未年');
  assert.equal(ganzhiYear('2027-02-05', CFG), '丙午年');
  assert.equal(ganzhiYear('2027-02-06'), '丙午年'); // ICU 誤差：只靠 Intl 會錯這一天
  assert.equal(computeTearoff(CFG, data(), '2027-02-06').ganzhi, '丁未年'); // 頁面走的路徑
});

test('干支年：設定檔的換年日優先於 ICU；該年沒列或日期不合理時退回 Intl', () => {
  const cfg = clone(CFG);
  cfg.lunarNewYear.dates['2026'] = { date: '2026-02-20', src: 'cwa-2026' }; // 故意晚三天
  assert.equal(ganzhiYear('2026-02-18', cfg), '乙巳年'); // ICU 說丙午，設定說還沒換年
  assert.equal(ganzhiYear('2026-02-20', cfg), '丙午年');
  cfg.lunarNewYear.dates['2026'] = { date: '2026-02-14', src: 'cwa-2026' }; // 故意早三天
  assert.equal(ganzhiYear('2026-02-14', cfg), '丙午年');
  assert.equal(ganzhiYear('2026-02-13', cfg), '乙巳年');
  // 2030 沒列 → Intl（ICU 2030 初一＝02-02）
  assert.equal(ganzhiYear('2030-02-01', cfg), '己酉年');
  assert.equal(ganzhiYear('2030-02-02', cfg), '庚戌年');
  // 不合理的日期（不存在、跨年、不在 1/21–2/20）→ 該年退回 Intl
  for (const bad of ['2027-02-30', '2026-02-06', '2027-03-01', 'x']) {
    cfg.lunarNewYear.dates['2027'] = { date: bad, src: 'cwa-2027' };
    assert.equal(ganzhiYear('2027-02-06', cfg), '丙午年', bad);
  }
  delete cfg.lunarNewYear;
  assert.equal(ganzhiYear('2027-02-06', cfg), '丙午年');
});

test('干支年：只取兩字加「年」，不含西元年數字', () => {
  for (const d of ['2026-02-17', '2030-06-01', '1999-12-31']) {
    assert.match(ganzhiYear(d), /^[甲乙丙丁戊己庚辛壬癸][子丑寅卯辰巳午未申酉戌亥]年$/, d);
  }
});

// ── 同日同句：日期雜湊 ─────────────────────────────────────────────────────────────────

test('日期雜湊：決定性、無號 32 位元；宜與忌用不同 salt', () => {
  assert.equal(dateHash('yi|2026-11-03'), dateHash('yi|2026-11-03'));
  const h = dateHash('x');
  assert.ok(Number.isInteger(h) && h >= 0 && h < 2 ** 32);
  assert.notEqual(YI_SALT, JI_SALT);
  // 一整年裡，宜與忌的索引不會永遠相同（salt 有作用），且會用到詞庫大部分句子
  const pool = CFG.phrases.yiPool;
  let same = 0;
  const used = new Set();
  for (let i = 0; i < 365; i++) {
    const d = addDays('2026-01-01', i);
    const a = pool.indexOf(pickPhrase(pool, d, YI_SALT));
    const b = pool.indexOf(pickPhrase(pool, d, JI_SALT));
    if (a === b) same++;
    used.add(a);
  }
  assert.ok(same < 60, `宜忌同索引 ${same} 天`);
  assert.ok(used.size >= pool.length - 2, `一年只用到 ${used.size} 句`);
});

test('pickPhrase：同一天固定同一句、詞庫為空回傳 null', () => {
  const pool = CFG.phrases.jiPool;
  const a = pickPhrase(pool, '2026-11-03', JI_SALT);
  assert.ok(pool.includes(a));
  for (let i = 0; i < 5; i++) assert.equal(pickPhrase(pool, '2026-11-03', JI_SALT), a);
  assert.equal(pickPhrase([], '2026-11-03', JI_SALT), null);
});

// ── 休市日判定 ─────────────────────────────────────────────────────────────────────────

test('休市日：週六、週日', () => {
  assert.deepEqual(tpeClosed('2026-10-03', CFG, data()), { closed: true, reason: 'weekend' });
  assert.deepEqual(tpeClosed('2026-10-04', CFG, data()), { closed: true, reason: 'weekend' });
  assert.equal(tpeClosed('2026-10-05', CFG, data()).closed, false);
});

test('休市日：config.holidays.TPE 當年度的 days（國慶日補假 2026-10-09，週五）', () => {
  const r = tpeClosed('2026-10-09', CFG, data());
  assert.equal(r.closed, true);
  assert.equal(r.reason, 'config');
  assert.equal(r.name, '國慶日（補假）');
  // 只有 config 有、資料層沒有也算
  assert.equal(tpeClosed('2026-10-09', CFG, null).closed, true);
});

test('休市日：data.holidays（TWSE 休市日曆）有、config 沒有也算；半日市不算休市', () => {
  assert.deepEqual(tpeClosed('2027-01-04', CFG, data({ holidays: ['2027-01-04'] })), { closed: true, reason: 'data' });
  assert.equal(tpeClosed('2027-01-04', CFG, data()).closed, false); // 2027 config 未收、資料也沒有 → 交易日
  assert.equal(tpeClosed('2026-11-03', CFG, data({ holidays: 'oops' })).closed, false); // 型別錯 → 忽略
  const cfg = clone(CFG);
  cfg.holidays.TPE['2026'].days.push({ date: '2026-12-31', name: '測試半日市', half: true, close: '12:00' });
  assert.equal(tpeClosed('2026-12-31', cfg, data()).closed, false);
});

// ── 事件判定 ───────────────────────────────────────────────────────────────────────────

test('財報密集週：leadDays=7 含截止日（截止日 −6 為第一天、−7 不算、截止日隔天不算）', () => {
  const ew = CFG.earningsWeek;
  assert.equal(inEarningsWeek('2026-08-14', ew), true);
  assert.equal(inEarningsWeek('2026-08-08', ew), true); // −6
  assert.equal(inEarningsWeek('2026-08-07', ew), false); // −7
  assert.equal(inEarningsWeek('2026-08-15', ew), false);
  assert.equal(inEarningsWeek('2026-03-25', ew), true);
  assert.equal(inEarningsWeek('2026-03-24', ew), false);
  assert.equal(inEarningsWeek('2026-11-08', ew), true);
  assert.equal(inEarningsWeek('2026-05-09', ew), true);
  // 跨年的截止日（自訂 01-03）：前一年 12-28 起算
  assert.equal(inEarningsWeek('2026-12-28', { deadlines: ['01-03'], leadDays: 7 }), true);
  assert.equal(inEarningsWeek('2026-12-27', { deadlines: ['01-03'], leadDays: 7 }), false);
  // 不存在的截止日（非閏年 02-29）略過、不拋錯
  assert.equal(inEarningsWeek('2026-02-26', { deadlines: ['02-29'], leadDays: 7 }), false);
});

test('旺季：依月份命中；多筆都命中時全部回傳（上限由 computeTearoff 處理）', () => {
  assert.deepEqual(orderSeasonLabels('2026-07-16', CFG.orderSeasons), ['電子旺季']);
  assert.deepEqual(orderSeasonLabels('2026-10-06', CFG.orderSeasons), ['年底備貨']);
  assert.deepEqual(orderSeasonLabels('2026-11-03', CFG.orderSeasons), []);
});

test('台積電財報：events 中 2330 的 earnings／conference 且日期為當天；缺漏或型別錯不拋錯', () => {
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [tsmcEvent('2026-07-16')] })), true);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [tsmcEvent('2026-07-16', 'earnings')] })), true);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [tsmcEvent('2026-07-16', 'meeting')] })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [tsmcEvent('2026-07-17')] })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [{ ...tsmcEvent('2026-07-16'), code: '2303' }] })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: undefined })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: { code: '2330' } })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', data({ events: [null, 3, 'x'] })), false);
  assert.equal(tsmcEarningsOn('2026-07-16', null), false);
});

test('融資警示：嚴格小於門檻才觸發；等於不觸發；缺鍵或非有限數不觸發', () => {
  const t = CFG.thresholds;
  const m = (short_margin_ratio, maintenance_ratio) => data({ margin: { date: '2026-11-02', short_margin_ratio, maintenance_ratio } });
  assert.equal(marginAlert(m(1.9, 170), t), true);
  assert.equal(marginAlert(m(2.5, 149.99), t), true);
  assert.equal(marginAlert(m(2, 170), t), false); // 等於券資比門檻
  assert.equal(marginAlert(m(2.5, 150), t), false); // 等於維持率門檻
  assert.equal(marginAlert(m(2.5, 170), t), false);
  assert.equal(marginAlert(data({ margin: undefined }), t), false);
  assert.equal(marginAlert(data({ margin: null }), t), false);
  assert.equal(marginAlert(m('1.9', null), t), false); // 字串不算有限數
  assert.equal(marginAlert(m(Number.NaN, 140), t), true); // 一個壞、另一個有效且低 → 仍觸發
  assert.equal(marginAlert(null, t), false);
  // 門檻可在設定修改
  assert.equal(marginAlert(m(2.5, 170), { shortRatio: 3, maintenanceRatio: 150 }), true);
});

// ── spec「撕日曆的日期與『宜』」──────────────────────────────────────────────────────

test('spec 休市日：週六「宜」只顯示「出去走走　陪陪家人」（即使有法說會、旺季、財報密集週）', () => {
  const d = data({ events: [tsmcEvent('2026-08-08')] });
  const r = model('2026-08-08', d); // 週六、8 月旺季、財報密集週第一天、還有法說會
  assert.equal(r.closed, true);
  assert.deepEqual(r.yi, CLOSED_YI); // 字句與 spec 原文一致，見「定稿字句」測試
  assert.equal(r.yiFrom, 'closed');
});

test('spec 旺季中的台積電財報日：7 月交易日且為法說會日 →「台積電財報」與「電子旺季」兩項', () => {
  const r = model('2026-07-16', data({ events: [tsmcEvent('2026-07-16')] }));
  assert.equal(r.closed, false);
  assert.deepEqual(r.yi, [TSMC, '電子旺季']);
});

test('spec 無事件：交易日取宜詞庫一句，當天重畫多次（不同時刻）都是同一句', () => {
  const day = '2026-11-03'; // 週二、11 月非旺季、不在財報密集週（11-08 起）
  const r = model(day);
  assert.equal(r.yi.length, 1);
  assert.ok(CFG.phrases.yiPool.includes(r.yi[0]));
  assert.equal(r.yiFrom, 'pool');
  for (let i = 0; i < 3; i++) assert.deepEqual(model(day).yi, r.yi);
  // 同一本機日期的不同時刻（08:00 與 23:59）→ 同一句
  const early = localDate('Asia/Taipei', parseInstant(`${day}T08:00`, 'Asia/Taipei'));
  const late = localDate('Asia/Taipei', parseInstant(`${day}T23:59`, 'Asia/Taipei'));
  assert.deepEqual(model(early).yi, model(late).yi);
  // 忌也取詞庫、同日同句
  assert.equal(r.jiFrom, 'pool');
  assert.ok(CFG.phrases.jiPool.includes(r.ji[0]));
  assert.deepEqual(model(day).ji, r.ji);
});

test('spec 法說會資料缺漏：8 月交易日、資料檔沒有法說會 →「宜」只有「電子旺季」，無錯誤、無空白項', () => {
  const day = '2026-08-04'; // 週二，財報密集週（08-08 起）之前
  for (const d of [data({ events: undefined }), data({ events: [] }), data({ events: 'broken' }), data({ events: [tsmcEvent('2026-07-16')] }), null]) {
    const r = model(day, d);
    assert.deepEqual(r.yi, ['電子旺季']);
    assert.ok(r.yi.every((s) => typeof s === 'string' && s.trim() !== ''));
    assert.deepEqual(r.warnings, []);
  }
});

test('宜：優先序 台積電財報 → 財報密集週 → 旺季，最多兩項', () => {
  assert.equal(MAX_YI, 2);
  // 8/14 截止日當天（週五）＋法說會＋8 月旺季 → 只取前兩項
  assert.deepEqual(model('2026-08-14', data({ events: [tsmcEvent('2026-08-14')] })).yi, [TSMC, EWEEK]);
  // 財報密集週＋旺季
  assert.deepEqual(model('2026-08-12').yi, [EWEEK, '電子旺季']);
  // 只有財報密集週（11 月非旺季）
  assert.deepEqual(model('2026-11-10').yi, [EWEEK]);
  // 只有法說會（11 月）
  assert.deepEqual(model('2026-11-03', data({ events: [tsmcEvent('2026-11-03', 'earnings')] })).yi, [TSMC]);
});

test('宜：多個旺季同時命中時每筆各算一項，只取到兩項上限', () => {
  const cfg = clone(CFG);
  cfg.orderSeasons = [
    { label: '旺季甲', months: [7] },
    { label: '旺季乙', months: [6, 7] },
    { label: '旺季丙', months: [7, 8] },
  ];
  assert.deepEqual(model('2026-07-21', data(), cfg).yi, ['旺季甲', '旺季乙']);
  assert.deepEqual(model('2026-07-21', data({ events: [tsmcEvent('2026-07-21')] }), cfg).yi, [TSMC, '旺季甲']);
  assert.deepEqual(model('2026-06-16', data(), cfg).yi, ['旺季乙']);
});

test('休市日：config 休市日與 data.holidays 休市日一樣只顯示休市字句', () => {
  const a = model('2026-10-09'); // config（國慶日補假，週五）
  assert.deepEqual([a.closed, a.yi, a.ji], [true, CLOSED_YI, CLOSED_JI]);
  const b = model('2027-01-04', data({ holidays: ['2027-01-04'] })); // 只有 data.holidays
  assert.deepEqual([b.closed, b.yi, b.ji], [true, CLOSED_YI, CLOSED_JI]);
});

// ── spec「撕日曆的『忌』」──────────────────────────────────────────────────────────

test('spec 券資比過低：交易日且最新上市券資比 1.9% →「忌」顯示「借錢加碼」', () => {
  const r = model('2026-11-03', data({ margin: { date: '2026-10-30', short_margin_ratio: 1.9, maintenance_ratio: 170 } }));
  assert.deepEqual(r.ji, MARGIN_JI);
  assert.equal(r.jiFrom, 'margin');
  // task 6.4（審查 R3-low）：「最新的」＝輸出中那一份，但落後 ≥3 個交易日（與「資料過期標示」同一規則）
  // 就視同缺資料——兩週前的數字不據以產生「忌」，改取詞庫並記警告。
  const old = model('2026-11-17', data({ margin: { date: '2026-11-02', short_margin_ratio: 1.9, maintenance_ratio: 170 } }));
  assert.equal(old.jiFrom, 'pool');
  assert.ok(old.warnings.some((w) => w.includes('2026-11-02')), old.warnings.join('\n'));
});

test('融資資料過期（落後 ≥3 個交易日）視同缺資料：邊界與頁面傳入的現在時刻', () => {
  const low = (date) => data({ margin: { date, short_margin_ratio: 1.9, maintenance_ratio: 170 } });
  const at = (iso) => ({ nowMs: Date.parse(iso) });
  // 2026-11-17（週二）15:00 台北：最近已收盤交易日＝11/17。
  const afterClose = at('2026-11-17T15:00:00+08:00');
  // 11/13（週五）→ 落後 2（11/16、11/17）：照用。
  assert.equal(computeTearoff(CFG, low('2026-11-13'), '2026-11-17', afterClose).jiFrom, 'margin');
  // 11/12（週四）→ 落後 3：過期，視同缺資料。
  const stale = computeTearoff(CFG, low('2026-11-12'), '2026-11-17', afterClose);
  assert.equal(stale.jiFrom, 'pool');
  assert.ok(stale.warnings.some((w) => w.includes('2026-11-12')), stale.warnings.join('\n'));
  // 同一天 10:00（還沒收盤）：最近已收盤交易日＝11/16 → 11/12 落後 2，照用。
  assert.equal(computeTearoff(CFG, low('2026-11-12'), '2026-11-17', at('2026-11-17T10:00:00+08:00')).jiFrom, 'margin');
  // 日期缺漏或不合法：判斷不了是否過期，照舊使用（與過期標示相同：不顯示、不拋錯）。
  const undated = data({ margin: { short_margin_ratio: 1.9, maintenance_ratio: 170 } });
  assert.equal(computeTearoff(CFG, undated, '2026-11-17', afterClose).jiFrom, 'margin');
});

test('spec 休市日優先於融資警示：週日且維持率 140% →「忌」顯示休市兩行', () => {
  const r = model('2026-10-04', data({ margin: { date: '2026-10-02', short_margin_ratio: 2.5, maintenance_ratio: 140 } }));
  assert.deepEqual(r.ji, CLOSED_JI);
  assert.equal(r.jiFrom, 'closed');
});

test('忌：門檻等於時不觸發（取詞庫）；margin 缺鍵取詞庫；維持率單獨低於門檻也觸發', () => {
  assert.equal(model('2026-11-03', data({ margin: { short_margin_ratio: 2, maintenance_ratio: 150 } })).jiFrom, 'pool');
  assert.equal(model('2026-11-03', data({ margin: undefined })).jiFrom, 'pool');
  assert.deepEqual(model('2026-11-03', data({ margin: { short_margin_ratio: 2.5, maintenance_ratio: 149 } })).ji, MARGIN_JI);
});

// ── 設定防呆 ───────────────────────────────────────────────────────────────────────────

test('詞庫被使用者清空或全是空字串：退回內建預設詞庫並回警告（不留空白項）', () => {
  const cfg = clone(CFG);
  cfg.phrases.yiPool = [];
  cfg.phrases.jiPool = ['', '   ', 3];
  const r = computeTearoff(cfg, data(), '2026-11-03', { fallback: CFG });
  assert.ok(CFG.phrases.yiPool.includes(r.yi[0]));
  assert.ok(CFG.phrases.jiPool.includes(r.ji[0]));
  assert.equal(r.warnings.length, 2);
  assert.ok(r.warnings.every((w) => w.includes('詞庫')));
});

test('模型：日期、星期、月份、干支一併回傳', () => {
  const r = model('2026-07-28');
  assert.deepEqual([r.date, r.ganzhi, r.weekday, r.month, r.day], ['2026-07-28', '丙午年', '週二', '七月', '28']);
});

// ── 單行縮字（spec「詞庫字句過長」的純函式部分；頁面另以截圖驗證）────────────────────────

/** 假量測：每個字寬＝字級（全形字近似）。 */
const measureOf = (s) => (px) => [...s].length * px;

test('縮字：放得下時維持原字級', () => {
  assert.deepEqual(fitFontPx({ measure: measureOf('十個字十個字十個字字'), basePx: 48, maxW: 570, readablePx: 24 }), {
    px: 48,
    fits: true,
    belowReadable: false,
  });
});

test('縮字：20 字長句逐步縮小到放得下（整數字級、不低於可讀下限、寬度不超過可用寬度）', () => {
  const s = '把每筆交易的理由寫下來　收盤後再回頭檢查';
  assert.equal([...s].length, 20);
  const r = fitFontPx({ measure: measureOf(s), basePx: 48, maxW: 570, readablePx: 23.75 });
  assert.deepEqual(r, { px: 28, fits: true, belowReadable: false }); // 28×20＝560 ≤ 570；29×20＝580 > 570
});

test('縮字：沒有硬下限——低於可讀下限仍繼續縮到放進可用寬度，只標記 belowReadable（呼叫端記警告）', () => {
  const s = '一'.repeat(41);
  const r = fitFontPx({ measure: measureOf(s), basePx: 48, maxW: 570, readablePx: 23.75 });
  assert.deepEqual(r, { px: 13, fits: true, belowReadable: true }); // 13×41＝533 ≤ 570；14×41＝574 > 570
  assert.ok(measureOf(s)(r.px) <= 570);
  // 1px 都放不下（極端）：改用小數字級，仍放進可用寬度
  const huge = '一'.repeat(1000);
  const r2 = fitFontPx({ measure: measureOf(huge), basePx: 48, maxW: 570, readablePx: 24 });
  assert.equal(r2.fits, true);
  assert.equal(r2.belowReadable, true);
  assert.ok(r2.px > 0 && r2.px < 1 && measureOf(huge)(r2.px) <= 570, `px=${r2.px}`);
});

test('縮字：可讀下限高於原字級時，原字級放得下就不算低於下限', () => {
  assert.deepEqual(fitFontPx({ measure: measureOf('一二'), basePx: 20, maxW: 60, readablePx: 40 }), { px: 20, fits: true, belowReadable: false });
  assert.equal(fitFontPx({ measure: measureOf('一二三四'), basePx: 20, maxW: 60, readablePx: 40 }).belowReadable, true);
});

// ── 旋轉後的邊界框 ─────────────────────────────────────────────────────────────────────

test('transformedAabb：單位矩陣原樣；旋轉後取四角外接框', () => {
  const box = { label: 'x', x: 10, y: 20, w: 100, h: 40 };
  assert.deepEqual(transformedAabb(box, { a: 1, b: 0, c: 0, d: 1, e: 0, f: 0 }), box);
  // 平移
  assert.deepEqual(transformedAabb(box, { a: 1, b: 0, c: 0, d: 1, e: 5, f: -3 }), { label: 'x', x: 15, y: 17, w: 100, h: 40 });
  // 旋轉 90°（x' = -y, y' = x）
  const r = transformedAabb(box, { a: 0, b: 1, c: -1, d: 0, e: 0, f: 0 });
  assert.deepEqual(r, { label: 'x', x: -60, y: 10, w: 40, h: 100 });
  // 小角度旋轉：外接框比原框大，且包含四角
  const t = -0.022;
  const m = { a: Math.cos(t), b: Math.sin(t), c: -Math.sin(t), d: Math.cos(t), e: 0, f: 0 };
  const s = transformedAabb(box, m);
  assert.ok(s.w > 100 && s.h > 40);
});

// ── 截圖 fixture 的前提（fixture 是信封；config 與內建預設合併的方式同頁面：phrases 物件遞迴、詞庫陣列整份取代）──

const fixture = (name) => JSON.parse(readFileSync(path.join(HERE, 'fixtures', name), 'utf8'));
const withFixtureConfig = (fx) => ({ ...clone(CFG), phrases: { ...clone(CFG.phrases), ...fx.config.phrases } });

test('fixture tearoff-long-phrase：詞庫＝內建詞庫逐字＋一句 20 字，2029-03-13 宜與忌都選中那句 20 字', () => {
  const fx = fixture('tearoff-long-phrase.json');
  const yiPool = fx.config.phrases.yiPool;
  const jiPool = fx.config.phrases.jiPool;
  assert.deepEqual(yiPool.slice(0, -1), CFG.phrases.yiPool);
  assert.deepEqual(jiPool.slice(0, -1), CFG.phrases.jiPool);
  assert.equal([...yiPool.at(-1)].length, 20);
  assert.equal([...jiPool.at(-1)].length, 20);
  const r = computeTearoff(withFixtureConfig(fx), fx.data, '2029-03-13');
  assert.equal(r.closed, false);
  assert.deepEqual(r.yi, [yiPool.at(-1)]);
  assert.deepEqual(r.ji, [jiPool.at(-1)]);
});

test('fixture 情境：台積電七月法說會、券資比 1.9%、週日維持率 140% 的模型結果', () => {
  const july = fixture('tearoff-tsmc-july.json');
  assert.deepEqual(computeTearoff(CFG, july.data, '2026-07-16').yi, [TSMC, '電子旺季']);
  const short = fixture('tearoff-short-ratio.json');
  assert.equal(short.data.margin.short_margin_ratio, 1.9);
  assert.deepEqual(computeTearoff(CFG, short.data, '2026-11-03').ji, MARGIN_JI);
  const sun = fixture('tearoff-sunday-maintenance.json');
  assert.equal(sun.data.margin.maintenance_ratio, 140);
  const r = computeTearoff(CFG, sun.data, '2026-10-04');
  assert.deepEqual([r.yi, r.ji], [CLOSED_YI, CLOSED_JI]);
});
