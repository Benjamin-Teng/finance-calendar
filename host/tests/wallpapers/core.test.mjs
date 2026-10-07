// host/tests/wallpapers/core.test.mjs
//
// 桌布繪圖頁共用模組純邏輯單元測試（task 3.1）：`node --test host/tests/wallpapers/`。
// 只用 Node 內建 test runner；被測的 core.mjs 不依賴 DOM。

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  DEFAULT_H,
  DEFAULT_W,
  FONT_REQUIREMENTS,
  KNOWN_PARAMS,
  checkLayout,
  cssFont,
  floorToStep,
  isValidTz,
  mergeConfig,
  mulberry32,
  normalizePayload,
  parseInstant,
  parseQuery,
  sizeScale,
  textBox,
  tzOffsetMin,
  tzParts,
  zonedToUtc,
} from '../../ui/wallpapers/lib/core.mjs';

const FIXTURES_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'tests', 'fixtures');
const FONT_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', '..', 'ui', 'wallpapers', 'fonts');

// ── 尺寸基準 ─────────────────────────────────────────────────────────────────────────

test('sizeScale：S = min(W,H)/2160', () => {
  assert.equal(sizeScale(3840, 2160), 1);
  assert.equal(sizeScale(2560, 1600), 1600 / 2160);
  assert.equal(sizeScale(1920, 1080), 0.5);
  assert.equal(sizeScale(2560, 1080), 0.5); // 超寬：短邊決定
  assert.equal(sizeScale(1080, 1920), 0.5); // 直式：短邊決定
});

// ── 決定性亂數 ───────────────────────────────────────────────────────────────────────

test('mulberry32：同 seed 同序列，與樣稿實作逐值相同', () => {
  const a = mulberry32(19700101);
  const b = mulberry32(19700101);
  const seqA = Array.from({ length: 1000 }, () => a());
  const seqB = Array.from({ length: 1000 }, () => b());
  assert.deepEqual(seqA, seqB);
  // 樣稿 bg-sessions.html 的 mulberry32 以同 seed 算出的前三個值
  assert.deepEqual(seqA.slice(0, 3), [0.939534186385572, 0.8845732707995921, 0.6872420234140009]);
});

test('mulberry32：不同 seed 序列不同；值域 [0,1)；平均約 0.5', () => {
  const a = Array.from({ length: 50 }, ((r) => () => r())(mulberry32(1)));
  const b = Array.from({ length: 50 }, ((r) => () => r())(mulberry32(2)));
  assert.notDeepEqual(a, b);
  const r = mulberry32(42);
  let sum = 0;
  for (let i = 0; i < 20000; i++) {
    const v = r();
    assert.ok(v >= 0 && v < 1);
    sum += v;
  }
  assert.ok(Math.abs(sum / 20000 - 0.5) < 0.01);
});

// ── 時區 ─────────────────────────────────────────────────────────────────────────────

test('tzOffsetMin：台北／東京無夏令，紐約與倫敦隨夏令變動', () => {
  const jul = Date.UTC(2026, 6, 1, 12, 0);
  const jan = Date.UTC(2026, 0, 15, 12, 0);
  assert.equal(tzOffsetMin('Asia/Taipei', jul), 480);
  assert.equal(tzOffsetMin('Asia/Tokyo', jan), 540);
  assert.equal(tzOffsetMin('America/New_York', jul), -240);
  assert.equal(tzOffsetMin('America/New_York', jan), -300);
  assert.equal(tzOffsetMin('Europe/London', jul), 60);
  assert.equal(tzOffsetMin('Europe/London', jan), 0);
});

test('zonedToUtc：夏令時間切換前後（紐約 2026-03-08、倫敦 2026-03-29）', () => {
  // 紐約 3/8 02:00 跳夏令：3/7 09:30 EST＝14:30Z；3/9 09:30 EDT＝13:30Z
  assert.equal(zonedToUtc('America/New_York', 2026, 3, 7, 9, 30), Date.UTC(2026, 2, 7, 14, 30));
  assert.equal(zonedToUtc('America/New_York', 2026, 3, 9, 9, 30), Date.UTC(2026, 2, 9, 13, 30));
  // 倫敦 3/29 01:00Z 跳夏令：3/27 08:00 GMT＝08:00Z；3/30 08:00 BST＝07:00Z
  assert.equal(zonedToUtc('Europe/London', 2026, 3, 27, 8, 0), Date.UTC(2026, 2, 27, 8, 0));
  assert.equal(zonedToUtc('Europe/London', 2026, 3, 30, 8, 0), Date.UTC(2026, 2, 30, 7, 0));
  // 無夏令：東京 09:00＝00:00Z
  assert.equal(zonedToUtc('Asia/Tokyo', 2026, 10, 2, 9, 0), Date.UTC(2026, 9, 2, 0, 0));
});

test('tzParts／zonedToUtc 往返一致（跨日、跨年）', () => {
  const ms = Date.UTC(2026, 11, 31, 20, 5, 9); // 台北已是 2027-01-01 04:05:09
  const p = tzParts('Asia/Taipei', ms);
  assert.deepEqual(p, { y: 2027, mo: 1, d: 1, h: 4, mi: 5, s: 9 });
  assert.equal(zonedToUtc('Asia/Taipei', p.y, p.mo, p.d, p.h, p.mi, p.s), ms);
});

test('tzParts：午夜是 00 不是 24（hourCycle h23）', () => {
  const p = tzParts('Asia/Taipei', Date.UTC(2026, 9, 1, 16, 0)); // 台北 10/2 00:00
  assert.equal(p.h, 0);
  assert.equal(p.d, 2);
});

test('isValidTz', () => {
  assert.equal(isValidTz('Asia/Taipei'), true);
  assert.equal(isValidTz('UTC'), true);
  assert.equal(isValidTz('Mars/Olympus'), false);
  assert.equal(isValidTz(''), false);
  assert.equal(isValidTz(undefined), false);
});

test('floorToStep：15 分鐘一格', () => {
  const q = 15 * 60000;
  const t = Date.UTC(2026, 9, 2, 13, 29, 59, 999);
  assert.equal(floorToStep(t, q), Date.UTC(2026, 9, 2, 13, 15));
  assert.equal(floorToStep(Date.UTC(2026, 9, 2, 13, 30), q), Date.UTC(2026, 9, 2, 13, 30));
});

// ── query 解析 ───────────────────────────────────────────────────────────────────────

const NOW = Date.UTC(2026, 9, 2, 6, 0);
const opts = { nowMs: NOW, localTz: 'Asia/Taipei' };

test('parseQuery：全缺漏 → 預設值、無警告、現在＝注入的 nowMs', () => {
  const q = parseQuery('', opts);
  assert.deepEqual(q, {
    w: DEFAULT_W,
    h: DEFAULT_H,
    nowMs: NOW,
    tz: 'Asia/Taipei',
    fixture: null,
    rid: null,
    hasT: false,
    warnings: [],
  });
  assert.equal(parseQuery(undefined, opts).w, DEFAULT_W);
});

test('parseQuery：w／h／tz／fixture 正常值', () => {
  const q = parseQuery('?w=2560&h=1080&tz=America/New_York&fixture=/f/x.json', opts);
  assert.equal(q.w, 2560);
  assert.equal(q.h, 1080);
  assert.equal(q.tz, 'America/New_York');
  assert.equal(q.fixture, '/f/x.json');
  assert.deepEqual(q.warnings, []);
});

test('parseQuery：w／h 不合法 → 預設值＋警告（不拋錯）', () => {
  for (const bad of ['abc', '-5', '1.5', '0', '15', '16385', '1e9', '0x10']) {
    const q = parseQuery(`?w=${bad}&h=1080`, opts);
    assert.equal(q.w, DEFAULT_W, `w=${bad}`);
    assert.equal(q.h, 1080);
    assert.equal(q.warnings.length, 1, `w=${bad}`);
  }
  assert.equal(parseQuery('?w=16&h=16384', opts).warnings.length, 0); // 邊界值合法
});

test('parseQuery：t 帶時區＝絕對時刻', () => {
  assert.equal(parseQuery('?t=2026-10-02T13:00:00Z', opts).nowMs, Date.UTC(2026, 9, 2, 13, 0));
  assert.equal(parseQuery('?t=2026-10-02T21:00:00%2B08:00', opts).nowMs, Date.UTC(2026, 9, 2, 13, 0));
  assert.equal(parseQuery('?t=2026-10-02T13:00Z', opts).hasT, true);
});

test('parseQuery：t 不帶時區＝解讀成 tz 當地時間（與機器時區無關）', () => {
  const tpe = parseQuery('?t=2026-10-02T21:00&tz=Asia/Taipei', { nowMs: NOW, localTz: 'America/New_York' });
  assert.equal(tpe.nowMs, Date.UTC(2026, 9, 2, 13, 0));
  const nyc = parseQuery('?t=2026-10-02T09:30&tz=America/New_York', opts);
  assert.equal(nyc.nowMs, Date.UTC(2026, 9, 2, 13, 30)); // EDT
  // 沒給 tz 時用系統時區（與樣稿「無時區＝本機時間」行為相同）
  assert.equal(parseQuery('?t=2026-10-02T21:00', opts).nowMs, Date.UTC(2026, 9, 2, 13, 0));
  // 純日期＝當地午夜；帶秒
  assert.equal(parseQuery('?t=2026-10-02', opts).nowMs, Date.UTC(2026, 9, 1, 16, 0));
  assert.equal(parseQuery('?t=2026-10-02T21:00:30', opts).nowMs, Date.UTC(2026, 9, 2, 13, 0, 30));
});

test('parseQuery：t 不合法 → 用現在＋警告', () => {
  for (const bad of ['yesterday', '2026-13-45T00:00', '2026-10-02T25:00', '2026-02-30', '2026-10-02T21:00+0800', '1759381200']) {
    const q = parseQuery(`?t=${encodeURIComponent(bad)}`, opts);
    assert.equal(q.nowMs, NOW, `t=${bad}`);
    assert.equal(q.hasT, false);
    assert.equal(q.warnings.length, 1, `t=${bad}`);
  }
});

test('parseQuery：tz 不合法 → 系統時區＋警告；t 改以系統時區解讀', () => {
  const q = parseQuery('?tz=Mars/Olympus&t=2026-10-02T21:00', opts);
  assert.equal(q.tz, 'Asia/Taipei');
  assert.equal(q.warnings.length, 1);
  assert.equal(q.nowMs, Date.UTC(2026, 9, 2, 13, 0));
});

test('parseInstant：純日期不被誤判成帶 -02 偏移', () => {
  assert.equal(parseInstant('2026-10-02', 'UTC'), Date.UTC(2026, 9, 2));
});

// ── 文字邊界框與版面檢查 ────────────────────────────────────────────────────────────────

test('textBox：由 actualBoundingBox 換算左上角與寬高', () => {
  const ctx = {
    measureText: () => ({
      actualBoundingBoxLeft: 30, // 以 x 為基準向左延伸 30（置中對齊時的一半寬）
      actualBoundingBoxRight: 30,
      actualBoundingBoxAscent: 20,
      actualBoundingBoxDescent: 5,
    }),
  };
  assert.deepEqual(textBox(ctx, 'a', 'hello', 100, 200), { label: 'a', x: 70, y: 180, w: 60, h: 25 });
});

test('checkLayout：框在畫面內且不相交 → ok', () => {
  const r = checkLayout(
    [
      { label: 'a', x: 0, y: 0, w: 100, h: 50 },
      { label: 'b', x: 100, y: 0, w: 100, h: 50 }, // 只共用一條邊，不算相交
      { label: 'c', x: 0, y: 50, w: 200, h: 50 }, // 貼齊畫面邊界
    ],
    200,
    100,
  );
  assert.deepEqual(r, { ok: true, outside: [], overlaps: [] });
});

test('checkLayout：偵測超出畫面與相交', () => {
  const r = checkLayout(
    [
      { label: 'in', x: 10, y: 10, w: 50, h: 20 },
      { label: 'hit', x: 40, y: 20, w: 50, h: 20 },
      { label: 'out-right', x: 180, y: 10, w: 30, h: 10 },
      { label: 'out-top', x: 10, y: -1, w: 10, h: 10 },
    ],
    200,
    100,
  );
  assert.equal(r.ok, false);
  assert.deepEqual(r.outside, ['out-right', 'out-top']);
  assert.deepEqual(r.overlaps, [['in', 'hit']]);
});

// ── 字型清單與實體檔案 ─────────────────────────────────────────────────────────────────

test('FONT_REQUIREMENTS：每個字型檔存在、fonts.css 宣告該家族與檔名、每個家族有 OFL 授權檔', () => {
  const css = readFileSync(path.join(FONT_DIR, 'fonts.css'), 'utf8');
  const families = new Set();
  for (const req of FONT_REQUIREMENTS) {
    assert.ok(existsSync(path.join(FONT_DIR, req.file)), `缺字型檔 ${req.file}`);
    assert.ok(css.includes(`font-family: "${req.family}"`), `fonts.css 缺家族 ${req.family}`);
    assert.ok(css.includes(`url("${req.file}")`), `fonts.css 缺檔名 ${req.file}`);
    families.add(req.family);
  }
  const ofl = {
    Cinzel: 'OFL-Cinzel.txt',
    'LXGW WenKai TC': 'OFL-LXGWWenKaiTC.txt',
    'Noto Serif TC': 'OFL-NotoSerifTC.txt',
    'Noto Sans TC': 'OFL-NotoSansTC.txt',
    'IBM Plex Mono': 'OFL-IBMPlexMono.txt',
  };
  assert.deepEqual([...families].sort(), Object.keys(ofl).sort());
  for (const [family, file] of Object.entries(ofl)) {
    const p = path.join(FONT_DIR, file);
    assert.ok(existsSync(p), `缺授權檔 ${file}（${family}）`);
    assert.match(readFileSync(p, 'utf8'), /SIL OPEN FONT LICENSE Version 1\.1/);
  }
});

test('FONT_REQUIREMENTS：涵蓋樣稿用到的字重（Cinzel 500/600/700、霞鶩文楷 400/700、Noto Serif TC 900、Noto Sans TC 500/600、Plex Mono 600）', () => {
  const have = FONT_REQUIREMENTS.map((r) => `${r.family}:${r.weight}`).sort();
  assert.deepEqual(
    have,
    [
      'Cinzel:500',
      'Cinzel:600',
      'Cinzel:700',
      'IBM Plex Mono:600',
      'LXGW WenKai TC:400',
      'LXGW WenKai TC:700',
      'Noto Sans TC:500',
      'Noto Sans TC:600',
      'Noto Serif TC:900',
    ].sort(),
  );
  assert.equal(cssFont(FONT_REQUIREMENTS[1], 40), '600 40px "Cinzel"');
});

// ── 未知 query 參數（fix round 1：D2 的 now＝t 用字不一致，不能被默默忽略）─────────────────

test('parseQuery：未知參數產生警告（now、fixtures、亂寫的）且不影響已知參數', () => {
  const q = parseQuery('?w=1920&h=1080&now=2026-10-02T13:00:00Z&fixtures=x&foo=1', opts);
  assert.equal(q.w, 1920);
  assert.equal(q.nowMs, NOW); // now= 沒有生效
  assert.equal(q.hasT, false);
  assert.equal(q.warnings.length, 3);
  assert.ok(q.warnings.some((x) => x.includes('now=') && x.includes('請用 t')));
  assert.ok(q.warnings.some((x) => x.includes('fixtures=') && x.includes('fixture')));
  assert.ok(q.warnings.some((x) => x.includes('foo=1')));
});

test('parseQuery：六個已知參數全帶時沒有警告；重複的未知參數只警告一次', () => {
  assert.deepEqual(KNOWN_PARAMS, ['w', 'h', 't', 'tz', 'fixture', 'rid']);
  const q = parseQuery('?w=1920&h=1080&t=2026-10-02T21:00&tz=Asia/Taipei&fixture=/a.json&rid=5', opts);
  assert.deepEqual(q.warnings, []);
  assert.equal(parseQuery('?x=1&x=2', opts).warnings.length, 1);
});

// ── 通道 payload 信封與設定合併（fix round 1：D7 的 {data, config}）──────────────────────────

test('normalizePayload：{data, config} 信封原樣取出', () => {
  const p = normalizePayload({ data: { quotes: [1] }, config: { thresholds: { a: 1 } } });
  assert.deepEqual(p, { data: { quotes: [1] }, config: { thresholds: { a: 1 } }, shape: 'envelope', warnings: [] });
});

test('normalizePayload：信封缺 config → 空設定、無警告；config 不是物件 → 空設定＋警告', () => {
  assert.deepEqual(normalizePayload({ data: { x: 1 } }).config, {});
  assert.equal(normalizePayload({ data: { x: 1 } }).warnings.length, 0);
  for (const bad of [null, [], 'str', 5]) {
    const p = normalizePayload({ data: { x: 1 }, config: bad });
    assert.deepEqual(p.config, {});
    assert.equal(p.warnings.length, 1);
  }
});

test('normalizePayload：裸 tw_events JSON 視為 {data: 它, config: {}}', () => {
  const raw = { updated: '2026-10-02 10:55', quotes: [], twii_daily: [] };
  const p = normalizePayload(raw);
  assert.equal(p.shape, 'bare');
  assert.equal(p.data, raw);
  assert.deepEqual(p.config, {});
});

test('normalizePayload：實際錄製的 tw_events 樣本是裸形狀；有字串或陣列型 data 鍵不算信封', () => {
  const sample = JSON.parse(readFileSync(path.join(FIXTURES_DIR, 'tw_events_sample.json'), 'utf8'));
  const p = normalizePayload(sample);
  assert.equal(p.shape, 'bare');
  assert.ok(Object.keys(p.data).length > 10);
  assert.equal(normalizePayload({ data: 'x', other: 1 }).shape, 'bare');
  assert.equal(normalizePayload({ data: [1, 2] }).shape, 'bare');
});

test('normalizePayload：不是 JSON 物件 → 拋錯（不退化成空資料）', () => {
  for (const bad of [null, undefined, [], 'str', 5, true]) {
    assert.throws(() => normalizePayload(bad), /必須是 JSON 物件/);
  }
});

test('mergeConfig：缺鍵補預設、深層合併、不認得的鍵保留、不改動輸入', () => {
  const defaults = { a: 1, nested: { x: 'd', y: [1, 2] }, list: ['p'] };
  const config = { nested: { x: 'c' }, extra: true };
  const { value, warnings } = mergeConfig(defaults, config);
  assert.deepEqual(value, { a: 1, nested: { x: 'c', y: [1, 2] }, list: ['p'], extra: true });
  assert.deepEqual(warnings, []);
  assert.deepEqual(defaults, { a: 1, nested: { x: 'd', y: [1, 2] }, list: ['p'] });
  value.nested.y.push(3);
  assert.deepEqual(defaults.nested.y, [1, 2]); // 深拷貝
  assert.deepEqual(mergeConfig(defaults, undefined).value, defaults);
  assert.deepEqual(mergeConfig(defaults, {}).value, defaults);
});

test('mergeConfig：陣列與純量整個取代；型別不符 → 預設＋警告', () => {
  const defaults = { n: 150, list: ['a', 'b'], obj: { k: 1 }, s: 'x' };
  const { value, warnings } = mergeConfig(defaults, { n: '150', list: ['z'], obj: [], s: null });
  assert.deepEqual(value, { n: 150, list: ['z'], obj: { k: 1 }, s: 'x' });
  assert.equal(warnings.length, 3);
  assert.ok(warnings.some((w) => w.includes('config.n')));
  assert.ok(warnings.some((w) => w.includes('config.obj')));
  assert.ok(warnings.some((w) => w.includes('config.s')));
  assert.equal(mergeConfig({ a: 1 }, 'bad').warnings[0], 'config 型別應為物件，改用預設');
});
