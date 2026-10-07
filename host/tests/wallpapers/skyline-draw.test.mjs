// host/tests/wallpapers/skyline-draw.test.mjs
//
// 天際線（task 3.6）的繪圖規則：代表資料的顏色（上漲紅、下跌綠、20MA 電線）只給真實資料，裝飾層不用（controller ruling 4）；
// 版面在五種尺寸都落在畫面內。執行：`node --test "host/tests/wallpapers/*.test.mjs"`

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { DATA_COLORS, DECOR_COLORS, SKYLINE_FONTS, dataBandBox, skylineLayout, staleBoxEstimate } from '../../ui/wallpapers/lib/skyline-draw.mjs';
import { checkLayout, sizeScale } from '../../ui/wallpapers/lib/core.mjs';
import { buildSkylineModel, candleGeom } from '../../ui/wallpapers/lib/skyline-model.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const SAMPLE = JSON.parse(readFileSync(path.join(HERE, '..', '..', '..', 'tests', 'fixtures', 'tw_events_sample.json'), 'utf8'));

const FIVE = [
  [3840, 2160],
  [2560, 1600],
  [1920, 1080],
  [2560, 1080],
  [1080, 1920],
];

/** RGB → { hue（度，無彩度時 null）, chroma }。 */
function hc([r, g, b]) {
  const mx = Math.max(r, g, b);
  const mn = Math.min(r, g, b);
  const d = mx - mn;
  if (d === 0) return { hue: null, chroma: 0 };
  let h;
  if (mx === r) h = ((g - b) / d) % 6;
  else if (mx === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return { hue: (h * 60 + 360) % 360, chroma: d };
}
const hueDiff = (a, b) => Math.min(Math.abs(a - b), 360 - Math.abs(a - b));

test('資料色：上漲紅、下跌綠、20MA 電線三色互不相同', () => {
  const { up, down, wire } = DATA_COLORS;
  assert.notDeepEqual(up, down);
  assert.notDeepEqual(up, wire);
  assert.notDeepEqual(down, wire);
});

test('裝飾層不用資料色：不等於任何資料色；有彩度的裝飾色（chroma ≥ 60）與紅、綠色相差 ≥ 30°、與電線色相差 ≥ 60°', () => {
  const data = Object.values(DATA_COLORS);
  assert.ok(DECOR_COLORS.length >= 6, '裝飾色清單應涵蓋窗燈、遠層、中層、101、星點、遠山');
  for (const { name, rgb } of DECOR_COLORS) {
    for (const d of data) assert.notDeepEqual(rgb, d, name);
    const c = hc(rgb);
    if (c.chroma < 60) continue;
    assert.ok(hueDiff(c.hue, hc(DATA_COLORS.up).hue) >= 30, `${name} 色相 ${c.hue.toFixed(0)}° 太接近上漲紅`);
    assert.ok(hueDiff(c.hue, hc(DATA_COLORS.down).hue) >= 30, `${name} 色相 ${c.hue.toFixed(0)}° 太接近下跌綠`);
    assert.ok(hueDiff(c.hue, hc(DATA_COLORS.wire).hue) >= 60, `${name} 色相 ${c.hue.toFixed(0)}° 太接近電線色`);
  }
});

test('skylineLayout：五種尺寸下 20 根 K 的槽位都在畫面內、資料最高點在地面之上且低於標示文字列、標示在畫面內', () => {
  for (const [W, H] of FIVE) {
    const S = sizeScale(W, H);
    const L = skylineLayout(W, H, S);
    const tag = `${W}x${H}`;
    assert.equal(L.slots.length, 20, tag);
    for (const s of L.slots) assert.ok(s.x >= 0 && s.x + s.w <= W, tag);
    assert.ok(L.floorY < L.baseY, tag);
    const top = L.floorY - L.span; // v=1（最高價）的 y
    assert.ok(top > 0 && top < L.floorY, tag);
    assert.ok(L.captionY2 < top, `${tag} 第二行標示（${L.captionY2}）應在資料最高點（${top}）之上`);
    assert.ok(L.captionY1 > 0 && L.captionX > 0 && L.captionX < W / 2, tag);
    assert.ok(L.staleX < W && L.staleY < H, tag);
    assert.ok(L.staleY > L.floorY, `${tag} 過期標示基線應在資料最低點之下`);
  }
});

test('skylineLayout：16:9 的構圖基準與樣稿相同（地面 0.995H、101 高 0.6H、101 在 0.335W 附近）', () => {
  const L = skylineLayout(3840, 2160, 1);
  assert.ok(Math.abs(L.baseY - 2160 * 0.995) < 1e-9);
  assert.ok(Math.abs(L.towerCx - 3840 * 0.335) < 3840 * 0.02, `towerCx ${L.towerCx}`);
  assert.ok(Math.abs(L.towerH - 2160 * 0.6) < 1e-9);
});

test('修正輪 1：101 中心落在第 6、7 根 K 之間的間隙正中（不壓在任何一根 K 的正後方），五種尺寸皆然', () => {
  for (const [W, H] of FIVE) {
    const L = skylineLayout(W, H, sizeScale(W, H));
    const tag = `${W}x${H}`;
    const gapL = L.slots[6].x + L.slots[6].w;
    const gapR = L.slots[7].x;
    assert.ok(Math.abs(L.towerCx - (gapL + gapR) / 2) < 1e-9, tag);
    for (const s of L.slots) assert.ok(L.towerCx <= s.x || L.towerCx >= s.x + s.w, `${tag} 101 中心落在槽 ${s.cx} 內`);
  }
});

// ── 修正輪 1：過期標示不得與資料帶重疊 ─────────────────────────────────────────────────

const STALE_NOW = Date.parse('2026-10-06T13:30:00+08:00'); // 落後 3：顯示「資料停在 10/1」
const SAMPLE_FIX = (name) => JSON.parse(readFileSync(path.join(HERE, 'fixtures', name), 'utf8')).data;
const intersects = (a, b, W, H) => checkLayout([a, b], W, H).overlaps.length > 0;

/** 某尺寸、某資料下的資料帶框與過期標示估計框（staleY 可覆寫成舊位置做反證）。 */
function staleVsBand(W, H, data, staleY) {
  const S = sizeScale(W, H);
  const L = skylineLayout(W, H, S);
  const m = buildSkylineModel(data, {}, STALE_NOW);
  assert.equal(m.stale.show, true);
  const geom = m.candles.map((c) => candleGeom(c, m.range, L));
  const band = dataBandBox(m.candles, geom, L);
  const marker = staleBoxEstimate(L, staleY ?? L.staleY);
  return { L, band, marker };
}

test('修正輪 1：五種尺寸 × 大漲／大跌／一般，落後 3 時「資料停在 M/D」（含柔光的估計框）不與資料帶（K 線、影線、MA 電線與節點）相交，且在畫面內', () => {
  for (const name of ['skyline-rally.json', 'skyline-crash.json', null]) {
    const data = name ? SAMPLE_FIX(name) : SAMPLE;
    for (const [W, H] of FIVE) {
      const tag = `${W}x${H} ${name ?? 'sample'}`;
      const { L, band, marker } = staleVsBand(W, H, data);
      assert.ok(!intersects(band, marker, W, H), `${tag} 標示 ${JSON.stringify(marker)} 與資料帶 ${JSON.stringify(band)} 相交`);
      assert.ok(marker.x >= 0 && marker.y >= 0 && marker.x + marker.w <= W && marker.y + marker.h <= H, `${tag} 標示超出畫面`);
      // 與資料無關的幾何保證：標示（含柔光）的上緣低於資料可能的最低點（v=0）再加資料帶外擴
      assert.ok(marker.y >= L.floorY + L.dataPad, `${tag} 標示上緣 ${marker.y} 高於資料帶下緣 ${L.floorY + L.dataPad}`);
      assert.ok(marker.y + marker.h <= L.baseY, `${tag} 標示下緣超過地面`);
    }
  }
});

test('修正輪 1 反證：舊位置（staleY＝0.925H）在 1080×1920 的大漲、大跌資料下都與資料帶相交（上一個測試對舊版必定失敗）', () => {
  for (const name of ['skyline-rally.json', 'skyline-crash.json']) {
    const { band, marker } = staleVsBand(1080, 1920, SAMPLE_FIX(name), 1920 * 0.925);
    assert.ok(intersects(band, marker, 1080, 1920), `${name}：舊位置應相交`);
  }
});

test('dataBandBox：涵蓋每根 K 的最高、最低與 MA，左右含節點，外擴 dataPad；MA 為 null 的根只看高低', () => {
  const L = skylineLayout(1920, 1080, 0.5);
  const m = buildSkylineModel(SAMPLE, {}, STALE_NOW);
  const geom = m.candles.map((c) => candleGeom(c, m.range, L));
  const b = dataBandBox(m.candles, geom, L);
  const ys = geom.flatMap((g) => [g.yHigh, g.yLow, g.yMa]);
  assert.equal(b.label, 'data-band');
  assert.ok(Math.abs(b.y - (Math.min(...ys) - L.dataPad)) < 1e-9);
  assert.ok(Math.abs(b.y + b.h - (Math.max(...ys) + L.dataPad)) < 1e-9);
  assert.ok(Math.abs(b.x - (L.slots[0].x - L.dataPad)) < 1e-9);
  assert.ok(Math.abs(b.x + b.w - (L.slots[19].x + L.slots[19].w + L.dataPad)) < 1e-9);
  const noMa = m.candles.map((c) => ({ ...c, ma: null }));
  const g2 = noMa.map((c) => candleGeom(c, m.range, L));
  const b2 = dataBandBox(noMa, g2, L);
  assert.ok(Math.abs(b2.y - (Math.min(...g2.map((g) => g.yHigh)) - L.dataPad)) < 1e-9);
});

test('天際線實際用到的字型：IBM Plex Mono 600（標示）、Noto Sans TC 500（過期標示）', () => {
  assert.deepEqual(
    SKYLINE_FONTS.map((f) => f.id),
    ['notosans-500', 'plexmono-600'],
  );
});
