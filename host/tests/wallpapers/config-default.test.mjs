// host/tests/wallpapers/config-default.test.mjs
//
// 主題設定檔內建預設值的單元測試（task 3.2）：`node --test "host/tests/wallpapers/*.test.mjs"`（Node 22 不接受目錄引數）。
// JSON schema 驗證另走 `bash host/tests/wallpapers/check-config-schema.sh`（uvx check-jsonschema，一次性工具）；
// 這裡補 schema 表達不了的檢查：日期是否真實存在、是否落在週末、出處是否解得開、字句長度與用詞、
// 以及時段表與樣稿 MARKETS 逐欄相同（防搬錯）。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { mergeConfig } from '../../ui/wallpapers/lib/core.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.join(HERE, '..', '..', '..');
const DEFAULT_PATH = path.join(REPO, 'host', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json');
const SAMPLE_PATH = path.join(REPO, 'assets', 'design-explore', '05-astrolabe', 'bg-sessions.html');

// Windows 上 core.autocrlf 可能把檔案換成 CRLF；換行差異不是檔案內容問題，先正規化再檢查
const RAW = readFileSync(DEFAULT_PATH, 'utf8').replace(/\r\n/g, '\n');
const CFG = JSON.parse(RAW);

const isoToUtc = (s) => {
  const [y, m, d] = s.split('-').map(Number);
  const t = new Date(Date.UTC(y, m - 1, d));
  // 往返比對：2026-02-30 這類不存在的日期會被 Date 自動進位，往返後字串就不同
  assert.equal(t.toISOString().slice(0, 10), s, `${s} 不是真實存在的日期`);
  return t;
};

// ── 與 mergeConfig 的相容性 ────────────────────────────────────────────────────────────

test('預設檔：mergeConfig 以它為預設值時，空設定原樣取得、不產生警告', () => {
  const { value, warnings } = mergeConfig(CFG, {});
  assert.deepEqual(value, CFG);
  assert.deepEqual(warnings, []);
});

test('預設檔：使用者只改一個門檻，其餘鍵逐鍵補齊；型別錯的鍵退回預設並警告', () => {
  const { value, warnings } = mergeConfig(CFG, { thresholds: { shortRatio: 3 } });
  assert.equal(value.thresholds.shortRatio, 3);
  assert.equal(value.thresholds.maintenanceRatio, CFG.thresholds.maintenanceRatio);
  assert.deepEqual(value.markets, CFG.markets);
  assert.deepEqual(warnings, []);

  const bad = mergeConfig(CFG, { thresholds: { shortRatio: '2%' }, phrases: { yiPool: 'x' } });
  assert.equal(bad.value.thresholds.shortRatio, CFG.thresholds.shortRatio);
  assert.deepEqual(bad.value.phrases.yiPool, CFG.phrases.yiPool);
  assert.equal(bad.warnings.length, 2);
});

test('預設檔：使用者可只補一個年度的休市表，不影響其他年度', () => {
  const user = { holidays: { TPE: { 2028: { through: '2028-12-31', days: [] } } } };
  const { value, warnings } = mergeConfig(CFG, user);
  assert.deepEqual(warnings, []);
  assert.deepEqual(value.holidays.TPE['2026'], CFG.holidays.TPE['2026']);
  assert.equal(value.holidays.TPE['2028'].through, '2028-12-31');
});

// ── 時段表與樣稿 MARKETS 逐欄相同 ───────────────────────────────────────────────────────

test(
  '時段表：與樣稿 bg-sessions.html 的 MARKETS 逐欄相同',
  { skip: existsSync(SAMPLE_PATH) ? false : 'assets/ 不在 git 內，樣稿不存在時略過' },
  () => {
    const html = readFileSync(SAMPLE_PATH, 'utf8');
    const m = html.match(/\/\*MARKETS-BEGIN\*\/([\s\S]*?)\/\*MARKETS-END\*\//);
    assert.ok(m, '樣稿找不到 MARKETS-BEGIN／MARKETS-END 標記');
    assert.deepEqual(CFG.markets, JSON.parse(m[1]));
  },
);

test('時段表：六市場 id 與順序固定，時段欄位齊全', () => {
  assert.deepEqual(
    CFG.markets.map((x) => x.id),
    ['TYO', 'SEL', 'TPE', 'HKG', 'LON', 'NYC'],
  );
  for (const mk of CFG.markets) {
    assert.ok(mk.segments.length > 0, mk.id);
    for (const s of mk.segments) {
      for (const k of ['kind', 'start', 'end', 'days', 'label']) assert.ok(s[k], `${mk.id} 時段缺 ${k}`);
    }
  }
});

// ── 休市表 ─────────────────────────────────────────────────────────────────────────────

test('休市表：市場 id 與時段表一致，每個市場都有 2026 全年', () => {
  assert.deepEqual(Object.keys(CFG.holidays).sort(), CFG.markets.map((x) => x.id).sort());
  for (const id of Object.keys(CFG.holidays)) {
    const y = CFG.holidays[id]['2026'];
    assert.ok(y, `${id} 缺 2026`);
    assert.equal(y.through, '2026-12-31', `${id} 的 2026 應為完整年度`);
  }
});

test('休市表：每筆日期真實存在、落在所屬年度、非週末、遞增不重複、不超過 through、出處解得開', () => {
  const sources = CFG.holidaySources;
  for (const [id, years] of Object.entries(CFG.holidays)) {
    for (const [year, { through, days }] of Object.entries(years)) {
      isoToUtc(through);
      assert.ok(through.startsWith(year), `${id} ${year} 的 through ${through} 不在該年度`);
      let prev = '';
      for (const d of days) {
        const at = `${id} ${d.date}`;
        const wd = isoToUtc(d.date).getUTCDay();
        assert.ok(wd !== 0 && wd !== 6, `${at} 落在週末（週末不列）`);
        assert.ok(d.date.startsWith(year), `${at} 不在 ${year} 年度`);
        assert.ok(d.date > prev, `${at} 未遞增或重複`);
        assert.ok(d.date <= through, `${at} 超過 through ${through}`);
        prev = d.date;
        assert.ok(d.name.trim() === d.name && d.name.length > 0, `${at} 假日名空白`);
        assert.ok(Object.hasOwn(sources, d.src), `${at} 的出處 ${d.src} 不在 holidaySources`);
        if (d.half === true) {
          assert.match(d.close, /^([01]\d|2[0-3]):[0-5]\d$/, `${at} 半日市 close 格式`);
        } else {
          assert.equal(d.half, undefined, `${at} half 只能是 true 或不寫`);
          assert.equal(d.close, undefined, `${at} 非半日市不應有 close`);
        }
      }
    }
  }
});

test('休市表：出處登記簿的每一筆都有官方網址（https）、查證日期，且至少被一筆休市日引用', () => {
  const used = new Set();
  for (const years of Object.values(CFG.holidays)) {
    for (const { days } of Object.values(years)) for (const d of days) used.add(d.src);
  }
  for (const [id, s] of Object.entries(CFG.holidaySources)) {
    assert.match(s.url, /^https:\/\//, id);
    assert.match(s.checked, /^\d{4}-\d{2}-\d{2}$/, id);
    for (const r of s.related ?? []) assert.match(r.url, /^https:\/\//, id);
    assert.ok(used.has(id), `出處 ${id} 沒有任何休市日引用`);
  }
});

test('休市表：各市場各年度筆數（休市日／半日市）固定，防止誤刪；2027 TPE 未公布故不收', () => {
  const count = (id, y) => {
    const days = CFG.holidays[id]?.[y]?.days;
    if (!days) return null;
    return [days.filter((d) => !d.half).length, days.filter((d) => d.half).length];
  };
  assert.deepEqual(count('TPE', '2026'), [18, 0]);
  assert.deepEqual(count('TYO', '2026'), [19, 0]);
  assert.deepEqual(count('SEL', '2026'), [17, 0]);
  assert.deepEqual(count('HKG', '2026'), [14, 3]);
  assert.deepEqual(count('LON', '2026'), [8, 2]);
  assert.deepEqual(count('NYC', '2026'), [10, 2]);

  assert.equal(count('TPE', '2027'), null, '證交所尚未公布 2027');
  assert.deepEqual(count('TYO', '2027'), [17, 0]);
  assert.deepEqual(count('SEL', '2027'), [16, 0]);
  assert.deepEqual(count('HKG', '2027'), [12, 1]);
  assert.equal(CFG.holidays.HKG['2027'].through, '2027-10-08', 'HKEX 2027 只公布到 10 月');
  assert.deepEqual(count('LON', '2027'), [8, 2]);
  assert.deepEqual(count('NYC', '2027'), [10, 1]);
});

test('休市表：幾個關鍵日期（官方頁面原文核對過）', () => {
  const find = (id, date) => CFG.holidays[id][date.slice(0, 4)].days.find((d) => d.date === date);
  assert.equal(find('NYC', '2026-11-26').name, '感恩節');
  assert.deepEqual(find('NYC', '2026-11-27'), { date: '2026-11-27', name: '感恩節隔天', half: true, close: '13:00', src: 'nyse' });
  assert.equal(find('NYC', '2026-12-24').close, '13:00');
  assert.equal(find('LON', '2026-12-24').close, '12:30');
  assert.equal(find('LON', '2026-12-31').close, '12:30');
  assert.equal(find('HKG', '2026-12-24').close, '12:00');
  assert.equal(find('HKG', '2026-12-31').close, '12:00');
  assert.equal(find('TPE', '2026-02-17').name, '農曆春節');
  assert.equal(find('SEL', '2026-12-31').name, '年終休市');
  assert.equal(find('TYO', '2026-01-02').name, '年始休市');
  // 週六的假期不列
  assert.equal(
    CFG.holidays.TPE['2026'].days.some((d) => d.date === '2026-02-28'),
    false,
  );
});

// ── 旺季、財報密集週、門檻 ─────────────────────────────────────────────────────────────

test('旺季表、財報密集週、門檻：照 spec 預設值', () => {
  const byMonth = new Map();
  for (const s of CFG.orderSeasons) {
    for (const m of s.months) {
      assert.equal(byMonth.has(m), false, `月份 ${m} 重複出現在旺季表`);
      byMonth.set(m, s.label);
    }
  }
  assert.deepEqual([...byMonth.entries()].sort((a, b) => a[0] - b[0]), [
    [7, '電子旺季'],
    [8, '電子旺季'],
    [9, '電子旺季'],
    [10, '年底備貨'],
  ]);
  assert.deepEqual(CFG.earningsWeek.deadlines, ['03-31', '05-15', '08-14', '11-14']);
  assert.equal(CFG.earningsWeek.leadDays, 7);
  assert.equal(CFG.thresholds.shortRatio, 2);
  assert.equal(CFG.thresholds.maintenanceRatio, 150);
  for (const d of CFG.earningsWeek.deadlines) isoToUtc(`2026-${d}`);
});

// ── 宜忌字句 ───────────────────────────────────────────────────────────────────────────

test('宜忌固定字句：使用者定稿，逐字相符（含全形空格 U+3000）', () => {
  const p = CFG.phrases;
  assert.equal(p.closedDay.yi, '出去走走　陪陪家人');
  assert.deepEqual(p.closedDay.ji, ['懊悔無賣高買低', '輸錢怨氣帶回家']);
  assert.equal(p.marginWarning.ji, '借錢加碼');
  assert.equal(p.events.tsmcEarnings, '台積電財報');
  assert.equal(p.events.earningsWeek, '財報密集週');
});

test('宜忌詞庫：各約 20 句、每句 ≤ 12 字、不重複、無買賣建議或保證用語', () => {
  const BANNED = ['保證', '必賺', '穩賺', '一定賺', '包賺', '翻倍', '暴富', '飆', '噴出', '買進', '賣出', '加碼', '抄底', '低接', '推薦', '目標價'];
  const fixed = new Set([
    CFG.phrases.closedDay.yi,
    ...CFG.phrases.closedDay.ji,
    CFG.phrases.marginWarning.ji,
  ]);
  for (const [name, pool] of [
    ['yiPool', CFG.phrases.yiPool],
    ['jiPool', CFG.phrases.jiPool],
  ]) {
    assert.ok(pool.length >= 18 && pool.length <= 22, `${name} 句數 ${pool.length}`);
    assert.equal(new Set(pool).size, pool.length, `${name} 有重複句`);
    for (const s of pool) {
      assert.ok([...s].length <= 12, `${name}「${s}」超過 12 字（${[...s].length}）`);
      assert.equal(s.trim(), s, `${name}「${s}」前後有空白`);
      assert.equal(fixed.has(s), false, `${name}「${s}」與固定字句重複`);
      for (const w of BANNED) assert.equal(s.includes(w), false, `${name}「${s}」含「${w}」`);
    }
  }
});

// ── 檔案衛生 ───────────────────────────────────────────────────────────────────────────

test('檔案衛生：無 BOM、無反斜線、無控制字元（只允許換行）', () => {
  assert.notEqual(RAW.charCodeAt(0), 0xfeff, '不得有 BOM');
  assert.equal(RAW.includes('\\'), false, '預設檔不應出現反斜線（跳脫序列會藏 \\n、\\u 之類的問題）');
  // eslint-disable-next-line no-control-regex
  assert.equal(/[\u0000-\u0009\u000b-\u001f\u007f]/.test(RAW), false, '含控制字元');
  assert.ok(RAW.endsWith('}\n'));
});

test('農曆正月初一表（task 3.4 修正輪 1）：2026–2028 官方日期、出處為中央氣象署 https 網址且有查證日期、每個出處都被引用', () => {
  const lny = CFG.lunarNewYear;
  assert.deepEqual(
    Object.fromEntries(Object.entries(lny.dates).map(([y, e]) => [y, e.date])),
    { 2026: '2026-02-17', 2027: '2027-02-06', 2028: '2028-01-26' },
  );
  const used = new Set(Object.values(lny.dates).map((e) => e.src));
  for (const [id, s] of Object.entries(lny.sources)) {
    assert.match(s.url, /^https:\/\/www\.cwa\.gov\.tw\//, id);
    assert.match(s.checked, /^\d{4}-\d{2}-\d{2}$/, id);
    assert.ok(used.has(id), `出處 ${id} 沒被引用`);
  }
  for (const e of Object.values(lny.dates)) assert.ok(Object.hasOwn(lny.sources, e.src), e.src);
});
