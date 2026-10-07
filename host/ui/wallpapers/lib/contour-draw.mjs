// host/ui/wallpapers/lib/contour-draw.mjs
//
// 等高線（task 3.5）的繪圖。畫風與構圖忠實移植樣稿 `assets/design-explore/07-contour/bg.html`
// （深青底漸層、極淡經緯格線、78 條等高線每 5 條一條計曲線、琥珀色標高三角與數字、航海圖邊框刻度、細雜訊），
// 只改下列項目（理由見 task 3.5 報告）：
//   - **高度場由盤中走勢推導**：把走勢畫成一道山脊——脊線的平面位置＝走勢圖（時間→橫向 xa..xb、價格→縱向
//     yLow..yHigh，價格高者在上），脊頂高度隨價格升高（當日低點 0.40、高點 1.00），所以最高峰就在當日高點。
//     高度＝窄山脊（σ1）＋山麓（σ2）＋寬緩坡（σ3）三層，都以「到走勢折線的距離」計算（緩坡讓遠處等高線像樣稿一樣成片延展，
//     不被雜訊打成小圈）。取代樣稿示意用的波動率微笑與五座固定山峰。
//   - 樣稿的雙尺度雜訊（等高線的自然曲折）、左上邊界平滑收斂、右側往小工具區漸淡保留；雜訊 seed＝資料交易日。
//     右側漸淡改從資料段右端之後才開始（xb＋0.03 到 xb＋0.45），不壓低走勢本身。
//   - **標高數字＝資料原值**：位置是走勢的局部高點（seriesPeaks，第一個一定是當日最高點），數字是該點的指數價位
//     （千分位、兩位小數）。樣稿的「16 + 高度×34 + 亂數」示意 IV 值拿掉，不捏造數值。
//   - 尺寸基準改用共用契約 `S = min(W,H)/2160`（樣稿 W/3840）；線寬保留樣稿的像素下限，16:9 與樣稿相同。
//     標高字級由樣稿 W/280（4K 約 14px）放大為 22×S，避免 1080p 只剩 7px；三種文字都加與底色同色的柔光，等高線穿過時仍可讀。
//   - 直式（W < H）時資料段放寬到 x 0.10–0.84。
//   - 走勢折線只畫資料點之間（xa..xb），沒有延伸到資料外的人造線段；折線兩端外的高度只隨距離衰減（圓頭），
//     不帶任何漲跌形狀（修正輪 1 檢查過，無須修改）。
//   - 新增文字：左下標示交易日（Plex Mono 600），落後 ≥ 3 個交易日時右下角顯示「資料停在 M/D」（Noto Sans TC 500）。
// 不用計時器與 requestAnimationFrame（design.md D2）。

import { mulberry32 } from './core.mjs';
import { formatPrice, seriesPeaks } from './intraday.mjs';
import { INTRADAY_FONTS, MONO, SANS, drawLabel, prepareIntraday, summarizeIntraday } from './intraday-page.mjs';

/** 等高線實際用到的字型（樣稿的標高字型 IBM Plex Mono 600＋過期標示的 Noto Sans TC 500）。 */
export const CONTOUR_FONTS = INTRADAY_FONTS;

/** 高度場網格（樣稿）。 */
export const GX = 300;
export const GY = 170;
const LEVELS = 78;
const MX = 0.055;
const MY = 0.065;

const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);
const smoothstep = (e0, e1, x) => {
  const t = clamp((x - e0) / (e1 - e0), 0, 1);
  return t * t * (3 - 2 * t);
};
const lerp = (a, b, t) => a + (b - a) * t;

/** 走勢在畫面上的位置（正規化座標）。 */
export function contourLayout(W, H) {
  const portrait = W < H;
  return { xa: portrait ? 0.1 : 0.08, xb: portrait ? 0.84 : 0.52, yLow: 0.78, yHigh: 0.36 };
}

/** 雙尺度雜訊（樣稿 makeNoise；亂數由呼叫端給）。 */
function makeNoise(rand, n, fLo, fHi) {
  const terms = [];
  let ampSum = 0;
  for (let i = 0; i < n; i++) {
    const amp = 1 / (i + 1);
    terms.push({ fx: fLo + rand() * (fHi - fLo), fy: fLo + rand() * (fHi - fLo), px: rand() * 6.283, py: rand() * 6.283, amp });
    ampSum += amp;
  }
  return (nx, ny) => {
    let s = 0;
    for (const t of terms) s += t.amp * Math.sin(nx * t.fx * 6.283 + t.px) * Math.cos(ny * t.fy * 6.283 + t.py);
    return s / ampSum;
  };
}

/** 走勢折線（畫面像素）與脊頂高度。 */
export function crestOf(norm, W, H, layout) {
  return norm.t.map((t, i) => ({
    x: lerp(layout.xa, layout.xb, t) * W,
    y: lerp(layout.yLow, layout.yHigh, norm.v[i]) * H,
    h: 0.4 + 0.6 * norm.v[i],
  }));
}

/**
 * 高度場（純函式，不依賴 DOM）：GX×GY 網格的高度與極值。`rand` 供雜訊用（呼叫端以交易日 seed 建立）。
 * @returns {{ vals: Float32Array, vmin: number, vmax: number, crest: Array<{x:number,y:number,h:number}> }}
 */
export function buildField({ W, H, norm, rand, layout = contourLayout(W, H), gx = GX, gy = GY }) {
  const noiseCoarse = makeNoise(rand, 6, 0.8, 3.6);
  const noiseFine = makeNoise(rand, 5, 7, 15);
  const crest = crestOf(norm, W, H, layout);
  const U = Math.min(W, H);
  const s1 = (0.06 * U) ** 2;
  const s2 = (0.22 * U) ** 2;
  const s3 = (0.5 * U) ** 2;
  const segs = crest.length === 1 ? [[crest[0], crest[0]]] : crest.slice(1).map((b, i) => [crest[i], b]);

  function terrainRaw(nx, ny) {
    const px = nx * W;
    const py = ny * H;
    let r1 = 0;
    let r2 = 0;
    let r3 = 0;
    for (const [a, b] of segs) {
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const len2 = dx * dx + dy * dy;
      const u = len2 > 0 ? clamp(((px - a.x) * dx + (py - a.y) * dy) / len2, 0, 1) : 0;
      const ex = px - (a.x + u * dx);
      const ey = py - (a.y + u * dy);
      const d2 = ex * ex + ey * ey;
      const hc = a.h + u * (b.h - a.h);
      r3 = Math.max(r3, hc * Math.exp(-d2 / s3));
      if (d2 > 9 * s2) continue;
      r2 = Math.max(r2, hc * Math.exp(-d2 / s2));
      if (d2 < 9 * s1) r1 = Math.max(r1, hc * Math.exp(-d2 / s1));
    }
    return 0.72 * r1 + 0.34 * r2 + 0.5 * r3 + noiseCoarse(nx, ny) * 0.1 + noiseFine(nx, ny) * 0.04;
  }

  // 邊界平滑收斂（樣稿）：smoothstep 權重對「邊界參考值」與原始值做 lerp；內部（權重皆 1）直接取原始值。
  const corner = terrainRaw(MX, MY);
  const supFrom = layout.xb + 0.03;
  const supTo = layout.xb + 0.45; // 直式時 xb 偏右，終點可超過 1，右緣只淡化一部分、不擠出密線
  function heightAt(nx, ny) {
    const tx = smoothstep(0, MX, nx);
    const ty = smoothstep(0, MY, ny);
    let h;
    if (tx >= 1 && ty >= 1) {
      h = terrainRaw(nx, ny);
    } else {
      const hTopRow = lerp(corner, terrainRaw(nx, MY), tx);
      const hThisRow = lerp(terrainRaw(MX, ny), terrainRaw(nx, ny), tx);
      h = lerp(hTopRow, hThisRow, ty);
    }
    // 右側往小工具區漸淡：從資料段右端之後才開始
    return h * (1 - 0.95 * smoothstep(supFrom, supTo, nx));
  }

  const vals = new Float32Array(gx * gy);
  let vmin = Infinity;
  let vmax = -Infinity;
  for (let j = 0; j < gy; j++) {
    for (let i = 0; i < gx; i++) {
      const v = heightAt(i / (gx - 1), j / (gy - 1));
      vals[j * gx + i] = v;
      if (v < vmin) vmin = v;
      if (v > vmax) vmax = v;
    }
  }
  return { vals, vmin, vmax, crest };
}

/** marching squares（樣稿）。 */
function marchLevel(vals, level, W, H) {
  const px = (i) => (i / (GX - 1)) * W;
  const py = (j) => (j / (GY - 1)) * H;
  const lerpPt = (x0, y0, x1, y1, v0, v1) => {
    const t = clamp(v1 === v0 ? 0.5 : (level - v0) / (v1 - v0), 0, 1);
    return [x0 + (x1 - x0) * t, y0 + (y1 - y0) * t];
  };
  const segs = [];
  for (let j = 0; j < GY - 1; j++) {
    for (let i = 0; i < GX - 1; i++) {
      const a = vals[j * GX + i];
      const b = vals[j * GX + i + 1];
      const c = vals[(j + 1) * GX + i + 1];
      const d = vals[(j + 1) * GX + i];
      const idx = (a > level ? 8 : 0) | (b > level ? 4 : 0) | (c > level ? 2 : 0) | (d > level ? 1 : 0);
      if (idx === 0 || idx === 15) continue;
      const x0 = px(i);
      const x1 = px(i + 1);
      const y0 = py(j);
      const y1 = py(j + 1);
      const T = lerpPt(x0, y0, x1, y0, a, b);
      const R = lerpPt(x1, y0, x1, y1, b, c);
      const B = lerpPt(x0, y1, x1, y1, d, c);
      const L = lerpPt(x0, y0, x0, y1, a, d);
      const center = (a + b + c + d) / 4;
      switch (idx) {
        case 1: segs.push([L, B]); break;
        case 2: segs.push([B, R]); break;
        case 3: segs.push([L, R]); break;
        case 4: segs.push([T, R]); break;
        case 5: if (center > level) { segs.push([T, R]); segs.push([L, B]); } else { segs.push([T, L]); segs.push([B, R]); } break;
        case 6: segs.push([T, B]); break;
        case 7: segs.push([T, L]); break;
        case 8: segs.push([T, L]); break;
        case 9: segs.push([T, B]); break;
        case 10: if (center > level) { segs.push([T, L]); segs.push([B, R]); } else { segs.push([T, R]); segs.push([L, B]); } break;
        case 11: segs.push([T, R]); break;
        case 12: segs.push([L, R]); break;
        case 13: segs.push([B, R]); break;
        case 14: segs.push([L, B]); break;
      }
    }
  }
  return segs;
}

/**
 * 畫一張等高線。回傳模型（prepareIntraday 的結果＋標高），摘要與測試用。
 * @param {object} env runWallpaper 傳入的環境
 * @param {{ defaults?: object }} [opts]
 */
export async function drawContour(env, opts = {}) {
  const model = await prepareIntraday(env, opts);
  model.layout = contourLayout(env.W, env.H);
  model.peaks = seriesPeaks(model.norm, { max: 3, minSepT: 0.2, minProm: 0.1 });
  paint(env, model);
  return model;
}

/** 一行摘要（截圖稿記進證據檔）。 */
export function summarize(model) {
  const pk = (model.peaks ?? []).map((i) => formatPrice(model.norm.prices[i]));
  return summarizeIntraday(model, `標高：${pk.join('、')}`);
}

function paint(env, model) {
  const { ctx, W, H, S } = env;
  const rand = mulberry32(model.seed);
  const field = buildField({ W, H, norm: model.norm, rand, layout: model.layout });
  const { vals, vmin, vmax, crest } = field;

  // ---- 深青底漸層（樣稿）----
  const bg = ctx.createLinearGradient(0, 0, W * 0.35, H);
  bg.addColorStop(0, '#06181a');
  bg.addColorStop(1, '#0d2a2c');
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  // ---- 極淡經緯格線（樣稿）----
  ctx.save();
  ctx.strokeStyle = 'rgba(180,220,215,0.05)';
  ctx.lineWidth = Math.max(1, 2 * S);
  const cols = 14;
  const rows = 8;
  for (let gx = 1; gx < cols; gx++) {
    const x = (W / cols) * gx;
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, H);
    ctx.stroke();
  }
  for (let gy = 1; gy < rows; gy++) {
    const y = (H / rows) * gy;
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(W, y);
    ctx.stroke();
  }
  ctx.restore();

  // ---- 等高線：高密度、每 5 條一條計曲線（樣稿）----
  const lo = vmin + (vmax - vmin) * 0.04;
  const hi = vmax - (vmax - vmin) * 0.015;
  for (let L = 0; L < LEVELS; L++) {
    const segs = marchLevel(vals, lo + (hi - lo) * (L / (LEVELS - 1)), W, H);
    if (!segs.length) continue;
    ctx.beginPath();
    for (const [a, b] of segs) {
      ctx.moveTo(a[0], a[1]);
      ctx.lineTo(b[0], b[1]);
    }
    if (L % 5 === 0) {
      ctx.strokeStyle = 'rgba(127,209,193,0.90)';
      ctx.lineWidth = Math.max(2.4, 2.56 * S);
    } else {
      ctx.strokeStyle = 'rgba(47,111,106,0.40)';
      ctx.lineWidth = Math.max(1.2, 1.48 * S);
    }
    ctx.stroke();
  }

  // ---- 標高三角與數字：走勢的局部高點，數字＝該點價位（資料原值）----
  const tri = Math.max(9, 14.77 * S);
  const labelFont = `600 ${Math.round(22 * S)}px ${MONO}`;
  for (const [n, i] of model.peaks.entries()) {
    const { x: cx, y: cy } = crest[i];
    ctx.beginPath();
    ctx.moveTo(cx, cy - tri);
    ctx.lineTo(cx - tri * 0.9, cy + tri * 0.6);
    ctx.lineTo(cx + tri * 0.9, cy + tri * 0.6);
    ctx.closePath();
    ctx.fillStyle = 'rgba(217,164,65,0.92)';
    ctx.fill();
    drawLabel(env, {
      label: `peak${n}`,
      text: formatPrice(model.norm.prices[i]),
      x: cx + tri * 1.1,
      y: cy - tri * 0.3,
      font: labelFont,
      color: 'rgba(217,164,65,0.92)',
      baseline: 'bottom',
      halo: { color: 'rgba(6,24,26,1)', blur: 10 * S },
    });
  }

  // ---- 邊框刻度（航海圖樣式，樣稿）----
  ctx.save();
  ctx.strokeStyle = 'rgba(200,230,225,0.22)';
  ctx.lineWidth = Math.max(1.5, 2.4 * S);
  const m2 = 46.08 * S;
  ctx.strokeRect(m2, m2, W - m2 * 2, H - m2 * 2);
  const tick = 23.04 * S;
  ctx.beginPath();
  for (let tx = 0; tx <= 24; tx++) {
    const txp = m2 + ((W - m2 * 2) / 24) * tx;
    const len = tx % 4 === 0 ? tick * 1.8 : tick;
    ctx.moveTo(txp, m2);
    ctx.lineTo(txp, m2 + len);
    ctx.moveTo(txp, H - m2);
    ctx.lineTo(txp, H - m2 - len);
  }
  for (let ty = 0; ty <= 14; ty++) {
    const typ = m2 + ((H - m2 * 2) / 14) * ty;
    const len = ty % 4 === 0 ? tick * 1.8 : tick;
    ctx.moveTo(m2, typ);
    ctx.lineTo(m2 + len, typ);
    ctx.moveTo(W - m2, typ);
    ctx.lineTo(W - m2 - len, typ);
  }
  ctx.stroke();
  ctx.restore();

  // ---- 細雜訊（樣稿 addDither 0.035）----
  {
    const tile = document.createElement('canvas');
    tile.width = 96;
    tile.height = 96;
    const tctx = tile.getContext('2d');
    const id = tctx.createImageData(96, 96);
    for (let p = 0; p < id.data.length; p += 4) {
      const g = Math.floor(rand() * 256);
      id.data[p] = g;
      id.data[p + 1] = g;
      id.data[p + 2] = g;
      id.data[p + 3] = 255;
    }
    tctx.putImageData(id, 0, 0);
    ctx.save();
    ctx.globalAlpha = 0.035;
    ctx.globalCompositeOperation = 'overlay';
    ctx.fillStyle = ctx.createPattern(tile, 'repeat');
    ctx.fillRect(0, 0, W, H);
    ctx.restore();
  }

  // ---- 文字：交易日（左下）與過期標示（右下角）----
  drawLabel(env, {
    label: 'caption',
    text: model.caption,
    x: model.layout.xa * W,
    y: H * 0.925,
    font: `600 ${Math.round(24 * S)}px ${MONO}`,
    color: 'rgba(200,230,225,0.62)',
    halo: { color: 'rgba(6,24,26,1)', blur: 10 * S },
  });
  if (model.stale.show) {
    drawLabel(env, {
      label: 'stale',
      text: model.stale.text,
      x: W - 110 * S, // 避開右緣刻度（最長 m2＋tick×1.8 ≈ 87×S）
      y: H * 0.925,
      font: `500 ${Math.round(26 * S)}px ${SANS}`,
      color: 'rgba(217,164,65,0.92)',
      align: 'right',
      halo: { color: 'rgba(6,24,26,1)', blur: 10 * S },
    });
  }
}
