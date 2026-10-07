// host/ui/wallpapers/lib/ridgeline-draw.mjs
//
// 脊線（task 3.5）的繪圖。畫風與構圖忠實移植樣稿 `assets/design-explore/01-ridgeline/bg.html`
// （深藍底、60 條由遠到近互相遮擋的脊線、最近一條金線帶光暈、霧藍大氣、右上與左側壓暗、極輕雜訊），
// 只改下列項目（理由見 task 3.5 報告）：
//   - **主脊（金線）就是真實盤中走勢**：時間→橫軸 x0..x1、價格→高度（當日低點在基線、高點在基線上方 amp），
//     線性對應、不加樣稿的 2.4 次方與振幅包絡（那兩者會扭曲走勢的比例）。樣稿「挑一個最高點在 22%–42% 的亂數日」的迴圈拿掉。
//     資料段外以開盤／收盤價位**水平**延伸到畫面左右緣（修正輪 1：資料外不得有任何漲跌形狀），畫成細虛線、無光暈，
//     讀作「無資料」；山形的遮擋填色在畫面左右緣才垂直收到底。
//   - 其餘 59 條仍是樣稿的亂數日，屬裝飾；亂數 seed＝資料交易日（YYYYMMDD），同一份資料同一張圖。
//     **金色只留給真實資料**（修正輪 1）：樣稿次近的淡金線改為鋼藍；最靠近金線的 5 條裝飾線降低分鐘級雜訊
//     （decorNoiseWeight），不讀成第二條價格序列。
//   - 尺寸基準改用共用契約 `S = min(W,H)/2160`（樣稿 W/3840）；16:9 與樣稿相同。構圖位置仍用畫面比例。
//   - 直式（W < H）時資料段放寬到 x 0.08–0.84，避免走勢擠成窄條。
//   - 新增文字：金線基線下方標示交易日（Plex Mono 600），落後 ≥ 3 個交易日時右下角顯示「資料停在 M/D」（Noto Sans TC 500）。
// 不用計時器與 requestAnimationFrame（design.md D2）。

import { mulberry32 } from './core.mjs';
import { goldRidgePath } from './intraday.mjs';
import { INTRADAY_FONTS, MONO, SANS, drawLabel, prepareIntraday, summarizeIntraday } from './intraday-page.mjs';

/** 脊線實際用到的字型。 */
export const RIDGELINE_FONTS = INTRADAY_FONTS;

const BG0 = '#08111e';
const STEEL = [159, 179, 200]; // #9fb3c8
const GOLD = [224, 170, 84]; // #e0aa54
const N = 60; // 脊線條數
/** 金色（只用於真實資料）與脊線條數，供測試核對裝飾層不用金色。 */
export const DATA_GOLD = GOLD;
export const N_RIDGES = N;
/** 樣稿的分鐘雜訊權重。 */
const NOISE_W = 0.26;
/** 最靠近金線的幾條裝飾線降低雜訊。 */
const NEAR_SMOOTH = 6;
const PTS = 270; // 裝飾脊線的點數（樣稿：一天 270 分鐘）

/** 版面（畫面座標）。金線的資料段、基線與振幅；樣稿 v=1 時金線高度＝rowH × ampRows(1) × 2.6。 */
export function ridgeLayout(W, H, S) {
  const topMargin = H * 0.16;
  const bottomMargin = H * 0.2;
  const rowH = (H - topMargin - bottomMargin) / (N - 1);
  const portrait = W < H;
  return {
    topMargin,
    rowH,
    baseY: topMargin + (N - 1) * rowH,
    amp: rowH * ampRows(1) * 2.6,
    x0: W * (portrait ? 0.08 : 0.09),
    x1: W * (portrait ? 0.84 : 0.6),
    captionY: topMargin + (N - 1) * rowH + 64 * S,
    staleX: W - 72 * S,
    staleY: H * 0.925,
  };
}

/** 每條線最高峰以「幾條線距」表示（樣稿）。 */
function ampRows(f) {
  return 1 + Math.pow(f, 1.6) * 6.5;
}

/** 振幅包絡（樣稿）：峰心 x≈32%，兩側升餘弦收平。只用於裝飾脊線。 */
function ampEnvelope(xf) {
  const d = (xf - 0.32) / 0.34;
  if (Math.abs(d) >= 1) return 0.08;
  return 0.08 + 0.92 * 0.5 * (1 + Math.cos(Math.PI * d));
}

/**
 * 裝飾脊線 i 的分鐘雜訊權重：遠處＝樣稿 0.26；離金線第 k 條（k＝N−1−i < NEAR_SMOOTH）＝0.26×k/6，越近越平滑
 * （修正輪 1：緊鄰金線的鋸齒線會讀成第二條價格序列）。權重只影響混合比例，不改變亂數的取用順序。
 */
export function decorNoiseWeight(i) {
  const k = N - 1 - i;
  return k >= NEAR_SMOOTH ? NOISE_W : (NOISE_W * k) / NEAR_SMOOTH;
}

/** 樣稿的亂數盤中日（大結構＋分鐘雜訊），值域 [-1,1]。只用於裝飾脊線；noiseW＝分鐘雜訊權重（樣稿 0.26）。 */
function genDay(rnd, noiseW = NOISE_W) {
  const shapeFreqs = [
    { f: 1.6 + rnd() * 2.4, a: 0.5 + rnd() * 0.3, p: rnd() * 6.283 },
    { f: 4.5 + rnd() * 4.0, a: 0.18 + rnd() * 0.16, p: rnd() * 6.283 },
  ];
  const gapOpen = (rnd() - 0.5) * 0.5;
  const hasShock = rnd() < 0.45;
  const shockAt = Math.floor(PTS * (0.5 + rnd() * 0.4));
  const shockLen = Math.max(5, Math.floor(PTS * (0.03 + rnd() * 0.045)));
  const shockAmp = (rnd() - 0.4) * 1.4;
  const shape = new Array(PTS);
  const noise = new Array(PTS);
  let walk = 0;
  for (let i = 0; i < PTS; i++) {
    const t = i / (PTS - 1);
    let sv = gapOpen * Math.exp(-t * 5);
    for (const k of shapeFreqs) sv += k.a * Math.sin(t * Math.PI * k.f + k.p);
    if (hasShock) {
      const d = i - shockAt;
      if (Math.abs(d) < shockLen) sv += shockAmp * 0.5 * (1 + Math.cos((Math.PI * d) / shockLen));
    }
    shape[i] = sv;
    const vol = t < 0.08 ? 0.044 : t < 0.72 ? 0.015 : 0.022 + (t - 0.72) * 0.06;
    walk += (rnd() - 0.5) * vol;
    noise[i] = walk;
  }
  const range = (a) => {
    const mn = Math.min(...a);
    const mx = Math.max(...a);
    return { mn, mx: Math.max(mx, mn + 0.001) };
  };
  const rs = range(shape);
  const rn = range(noise);
  const arr = new Array(PTS);
  for (let i = 0; i < PTS; i++) {
    const sN = ((shape[i] - rs.mn) / (rs.mx - rs.mn)) * 2 - 1;
    const nN = ((noise[i] - rn.mn) / (rn.mx - rn.mn)) * 2 - 1;
    arr[i] = sN * (1 - noiseW) + nN * noiseW;
  }
  const mn2 = Math.min(...arr);
  const range2 = Math.max(Math.max(...arr) - mn2, 0.001);
  for (let i = 0; i < PTS; i++) arr[i] = ((arr[i] - mn2) / range2) * 2 - 1;
  return arr;
}

/** 裝飾脊線樣式（i＝0 最遠、N−2 最近）：一律樣稿的鋼藍，越遠越淡；樣稿次近那條的淡金改為鋼藍（金色只留給資料）。 */
export function decorRowStyle(i, rowH) {
  const f = i / (N - 1);
  return { c: STEEL, a: 0.035 + Math.pow(f, 2.0) * 0.36, ampPx: rowH * ampRows(f) };
}

const rgba = (c, a) => `rgba(${c[0]},${c[1]},${c[2]},${a})`;

/**
 * 畫一張脊線。回傳模型（prepareIntraday 的結果＋版面），摘要與測試用。
 * @param {object} env runWallpaper 傳入的環境
 * @param {{ defaults?: object }} [opts]
 */
export async function drawRidgeline(env, opts = {}) {
  const model = await prepareIntraday(env, opts);
  model.layout = ridgeLayout(env.W, env.H, env.S);
  paint(env, model);
  return model;
}

/** 一行摘要（截圖稿記進證據檔）。 */
export function summarize(model) {
  return summarizeIntraday(model);
}

function paint(env, model) {
  const { ctx, W, H, S } = env;
  const L = model.layout;
  const rnd = mulberry32(model.seed);

  // ---- 底色 ----
  ctx.fillStyle = BG0;
  ctx.fillRect(0, 0, W, H);

  // ---- 裝飾脊線（0..N−2）：樣稿的亂數日，seed＝資料交易日 ----
  const days = [];
  for (let i = 0; i < N - 1; i++) days.push(genDay(rnd, decorNoiseWeight(i)));

  const strokePath = (pts, from, to) => {
    ctx.beginPath();
    ctx.moveTo(pts[from][0], pts[from][1]);
    for (let p = from + 1; p <= to; p++) ctx.lineTo(pts[p][0], pts[p][1]);
  };
  const occlude = (pts) => {
    // 先用底色填滿「線到畫布底部」整塊，遮住更遠的線（樣稿）
    ctx.beginPath();
    ctx.moveTo(pts[0][0], H);
    for (const [x, y] of pts) ctx.lineTo(x, y);
    ctx.lineTo(pts[pts.length - 1][0], H);
    ctx.closePath();
    ctx.fillStyle = BG0;
    ctx.fill();
  };
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';

  for (let i = 0; i < N - 1; i++) {
    const day = days[i];
    const baseY = L.topMargin + i * L.rowH;
    const st = decorRowStyle(i, L.rowH);
    const pts = new Array(PTS);
    for (let p = 0; p < PTS; p++) {
      const x = (p / (PTS - 1)) * W;
      const v = (day[p] + 1) / 2;
      pts[p] = [x, baseY - Math.pow(v, 2.4) * st.ampPx * 2.6 * ampEnvelope(x / W)];
    }
    occlude(pts);
    strokePath(pts, 0, PTS - 1);
    ctx.lineWidth = 2.4 * S;
    ctx.strokeStyle = rgba(st.c, st.a);
    ctx.shadowBlur = 0;
    ctx.stroke();
  }

  // ---- 主脊（金線）：真實盤中走勢 ----
  const gold = goldRidgePath(model.norm, { W, x0: L.x0, x1: L.x1, baseY: L.baseY, amp: L.amp });
  occlude(gold.pts); // 填色在畫面左右緣才垂直收到底
  // 資料外的水平延伸（開盤／收盤價位）：細虛線、低透明度、無光暈＝「無資料」
  ctx.save();
  ctx.lineWidth = 1.4 * S;
  ctx.setLineDash([6 * S, 12 * S]);
  ctx.lineCap = 'butt';
  ctx.strokeStyle = rgba(GOLD, 0.32);
  ctx.shadowBlur = 0;
  if (gold.dataFrom > 0) {
    strokePath(gold.pts, 0, gold.dataFrom);
    ctx.stroke();
  }
  if (gold.dataTo < gold.pts.length - 1) {
    strokePath(gold.pts, gold.dataTo, gold.pts.length - 1);
    ctx.stroke();
  }
  ctx.restore();
  // 資料段：樣稿金線（3.2×S、0.97、光暈）
  ctx.lineWidth = 3.2 * S;
  strokePath(gold.pts, gold.dataFrom, gold.dataTo);
  ctx.strokeStyle = rgba(GOLD, 0.97);
  ctx.shadowColor = 'rgba(224,170,84,0.6)';
  ctx.shadowBlur = 16 * S;
  ctx.stroke();
  ctx.shadowBlur = 0;
  if (gold.dataFrom === gold.dataTo) {
    // 單點序列：畫一個點，避免資料段零長度看不見
    const [x, y] = gold.pts[gold.dataFrom];
    ctx.beginPath();
    ctx.arc(x, y, 4 * S, 0, Math.PI * 2);
    ctx.fillStyle = rgba(GOLD, 0.97);
    ctx.fill();
  }

  // ---- 大氣（樣稿）----
  const fog = ctx.createRadialGradient(W * 0.2, H * 0.6, H * 0.05, W * 0.26, H * 0.55, H * 1.1);
  fog.addColorStop(0, 'rgba(27,42,61,0.34)');
  fog.addColorStop(0.55, 'rgba(27,42,61,0.14)');
  fog.addColorStop(1, 'rgba(8,17,30,0)');
  ctx.fillStyle = fog;
  ctx.fillRect(0, 0, W, H);

  const topOv = ctx.createLinearGradient(0, 0, 0, H * 0.26);
  topOv.addColorStop(0, 'rgba(5,9,16,0.34)');
  topOv.addColorStop(1, 'rgba(5,9,16,0)');
  ctx.fillStyle = topOv;
  ctx.fillRect(0, 0, W, H * 0.26);

  const corner = ctx.createRadialGradient(W * 0.98, H * 0.02, H * 0.05, W * 0.98, H * 0.02, H * 1.15);
  corner.addColorStop(0, 'rgba(5,9,16,0.5)');
  corner.addColorStop(0.5, 'rgba(5,9,16,0.2)');
  corner.addColorStop(1, 'rgba(5,9,16,0)');
  ctx.fillStyle = corner;
  ctx.fillRect(0, 0, W, H);

  const lGrad = ctx.createLinearGradient(0, 0, W * 0.075, 0);
  lGrad.addColorStop(0, 'rgba(3,6,10,0.5)');
  lGrad.addColorStop(1, 'rgba(3,6,10,0)');
  ctx.fillStyle = lGrad;
  ctx.fillRect(0, 0, W, H);

  const bGrad = ctx.createLinearGradient(0, H * 0.93, 0, H);
  bGrad.addColorStop(0, 'rgba(3,6,10,0)');
  bGrad.addColorStop(1, 'rgba(3,6,10,0.5)');
  ctx.fillStyle = bGrad;
  ctx.fillRect(0, H * 0.93, W, H * 0.07);

  // ---- 極輕微雜訊（樣稿）----
  const tile = 160;
  const off = document.createElement('canvas');
  off.width = off.height = tile;
  const octx = off.getContext('2d');
  const img = octx.createImageData(tile, tile);
  for (let k = 0; k < img.data.length; k += 4) {
    const val = 128 + (rnd() * 2 - 1) * 5;
    img.data[k] = val;
    img.data[k + 1] = val;
    img.data[k + 2] = val;
    img.data[k + 3] = 10;
  }
  octx.putImageData(img, 0, 0);
  ctx.globalCompositeOperation = 'overlay';
  ctx.fillStyle = ctx.createPattern(off, 'repeat');
  ctx.globalAlpha = 0.5;
  ctx.fillRect(0, 0, W, H);
  ctx.globalAlpha = 1;
  ctx.globalCompositeOperation = 'source-over';

  // ---- 文字：交易日（金線基線下方、資料段起點對齊）與過期標示（右下角）----
  drawLabel(env, {
    label: 'caption',
    text: model.caption,
    x: L.x0,
    y: L.captionY,
    font: `600 ${Math.round(24 * S)}px ${MONO}`,
    color: rgba(STEEL, 0.72),
  });
  if (model.stale.show) {
    drawLabel(env, {
      label: 'stale',
      text: model.stale.text,
      x: L.staleX,
      y: L.staleY,
      font: `500 ${Math.round(26 * S)}px ${SANS}`,
      color: rgba(GOLD, 0.88),
      align: 'right',
    });
  }
}
