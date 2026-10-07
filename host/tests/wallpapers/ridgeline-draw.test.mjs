// host/tests/wallpapers/ridgeline-draw.test.mjs
//
// 脊線（task 3.5 修正輪 1）的裝飾層規則：金色只留給真實資料；最靠近金線的裝飾脊線降低分鐘級鋸齒，
// 不讀成第二條價格序列。執行：`node --test "host/tests/wallpapers/*.test.mjs"`

import { test } from 'node:test';
import assert from 'node:assert/strict';

import { DATA_GOLD, N_RIDGES, decorNoiseWeight, decorRowStyle } from '../../ui/wallpapers/lib/ridgeline-draw.mjs';

/** RGB → 色相（度）。 */
function hue([r, g, b]) {
  const mx = Math.max(r, g, b);
  const mn = Math.min(r, g, b);
  if (mx === mn) return null;
  const d = mx - mn;
  let h;
  if (mx === r) h = ((g - b) / d) % 6;
  else if (mx === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return (h * 60 + 360) % 360;
}

test('裝飾脊線（0..N−2）都不用金色，也不用接近金色的色相（差 ≥ 90°）', () => {
  const goldHue = hue(DATA_GOLD);
  for (let i = 0; i < N_RIDGES - 1; i++) {
    const st = decorRowStyle(i, 10);
    assert.notDeepEqual(st.c, DATA_GOLD, `row ${i}`);
    const h = hue(st.c);
    if (h !== null) {
      const diff = Math.min(Math.abs(h - goldHue), 360 - Math.abs(h - goldHue));
      assert.ok(diff >= 90, `row ${i} 色相 ${h.toFixed(0)}° 與金色 ${goldHue.toFixed(0)}° 太近`);
    }
  }
});

test('裝飾脊線的分鐘雜訊權重：遠處維持樣稿 0.26，最靠近金線的幾條遞減（越近越平滑）', () => {
  assert.equal(decorNoiseWeight(0), 0.26);
  assert.equal(decorNoiseWeight(N_RIDGES - 8), 0.26);
  const near = [N_RIDGES - 2, N_RIDGES - 3, N_RIDGES - 4, N_RIDGES - 5, N_RIDGES - 6].map(decorNoiseWeight);
  for (let k = 1; k < near.length; k++) assert.ok(near[k] > near[k - 1], JSON.stringify(near));
  assert.ok(near[0] <= 0.26 / 5, `緊鄰金線那條 ${near[0]}`);
  assert.ok(near.every((w) => w < 0.26));
});
