// host/ui/wallpapers/lib/skyline-draw.mjs
//
// 天際線（task 3.6）的繪圖。畫風與構圖忠實移植樣稿 `assets/design-explore/09-skyline/bg.html`
// （黃昏漸層天空與早星、兩層霧紫遠山、最遠層淡紫灰樓群、地面暖光霧、中層樓群、台北 101、近層最暗的樓、
// 窗燈「大部分暗、少數特別亮」、極輕 overlay 雜訊），只改下列項目（理由見 task 3.6 報告）：
//   - **近層＝20 根真實日 K**（加權指數最後 20 個交易日，左舊右新）：等寬槽位均分畫面寬度，
//     樓身從地面到實體上緣（max(開, 收)），天線＝上影線（到最高價），實體（開..收）一段以紅／綠淡染並點亮較多窗燈，
//     樓頂一道紅（收 > 開）／綠（收 < 開）邊光，下影線（實體下緣到最低價）是樓面上一條細亮線。
//     價格→高度依 20 根的高低點與 20MA 正規化（skyline-model.mjs 的 priceRange；平盤不除以零）。
//     樣稿近層的亂數樓群全部拿掉，近層只剩資料。
//   - **20MA＝電線**：每根 K 的中心一個節點（y＝該根的 20MA），相鄰節點直線相連，電線冷藍色帶微光，畫在樓群前面；
//     沒有完整 20MA 的根不畫節點，也不往外延伸（資料外不畫任何東西）。
//   - **代表資料的顏色只給資料**（controller ruling 4）：上漲紅、下跌綠、電線冷藍。樣稿中層樓頂隨機的紅綠邊光
//     改為中性灰藍、拿掉中層的上下影線（避免讀成 K 線）；101 頂端的紅色航空燈改為暖白光。
//     窗燈（樣稿的暖金燈光）是裝飾，不是資料色；它的色相與紅、綠、電線都有差距（skyline-draw.test.mjs）。
//   - 台北 101 改畫在中層之前（樣稿在最前面），基部由中層與近層遮住：近層是等寬的 20 根 K，不能為 101 讓位或被它擋住。
//     中心放在第 6、7 根 K 之間的間隙正中（約 0.354W；樣稿 0.335W 正好壓在第 6 根正後方，修正輪 1）。
//   - 尺寸：線寬與小尺寸改用共用契約 `S = min(W,H)/2160`（樣稿 W/3840；16:9 相同）。垂直構圖以
//     `Href = min(H, 0.75W)` 為基準、貼齊畫面底部（16:9／16:10／21:9 都是 Href＝H，與樣稿相同；直式時城市不會被拉成細長條，
//     上方多出的是天空）。
//   - 新增文字：左上方兩行標示（日期範圍；最右一根的收盤價與 20MA，Plex Mono 600），落後 ≥ 3 個交易日時右下角
//     「資料停在 M/D」（Noto Sans TC 500，與 3.5 相同的字級、顏色；位置 0.925H，但不得高於資料可能的最低點——
//     直式時改落在資料帶下方的基座帶，修正輪 1）。資料帶（K 線、影線、電線與節點）的框也登記進 env.boxes，
//     截圖稿的版面檢查因此會斷言任何文字都不壓在資料上。
//   - 決定性：裝飾層（星點、遠山、遠層與中層樓群、101 窗燈、近層窗燈的明暗、雜訊）的亂數 seed＝最右一根 K 的日期。
// 不用計時器與 requestAnimationFrame（design.md D2）。

import { FONT_REQUIREMENTS, mulberry32 } from './core.mjs';
import { loadDefaultConfig } from './default-config.mjs';
import { formatPrice } from './intraday.mjs';
import { MONO, SANS, drawLabel } from './intraday-page.mjs';
import { CANDLE_COUNT, MA_PERIOD, buildSkylineModel, candleDirection, candleGeom, candleSlots } from './skyline-model.mjs';

/** 天際線實際用到的字型：標示 Plex Mono 600、過期標示 Noto Sans TC 500（與 3.5 相同）。 */
export const SKYLINE_FONTS = FONT_REQUIREMENTS.filter((r) => ['plexmono-600', 'notosans-500'].includes(r.id));

// ── 顏色 ─────────────────────────────────────────────────────────────────────────────

const RED = [224, 100, 90]; // #e0645a（樣稿）
const GRN = [70, 201, 138]; // #46c98a（樣稿）
const WIRE = [126, 214, 255]; // 20MA 電線：冷藍，與樣稿所有暖色裝飾區隔
const FLAT_K = [205, 210, 225]; // 開＝收（十字線）的邊光：中性
const WICK = [210, 220, 235]; // 影線（樣稿）
const INK = [7, 8, 15]; // 樓身（樣稿）
const LAMP = [255, 207, 122]; // #ffcf7a 窗燈（樣稿）
const STAR = [255, 250, 240];
const FAR_FILL = [86, 78, 112];
const FAR_DOT = [255, 224, 180];
const MID_TINT = [150, 160, 190]; // 中層樓頂邊光：中性灰藍（樣稿為隨機紅綠）
const BEACON = [255, 236, 214]; // 101 頂端航空燈：暖白（樣稿為紅）
const SPIRE = [220, 225, 235];
const MOUNT1 = [58, 56, 84];
const MOUNT2 = [70, 64, 92];
const STALE = [224, 170, 84]; // 過期標示（與 3.5 相同）
const CAPTION = [226, 222, 240];

/** 代表資料的顏色：只用在 20 根 K 與 20MA 電線。 */
export const DATA_COLORS = { up: RED, down: GRN, wire: WIRE };
/** 裝飾物件用到的顏色（測試核對不與資料色混淆）。天空漸層與地面暖光霧是背景大氣，不列入。 */
export const DECOR_COLORS = [
  { name: '窗燈／101 節縫', rgb: LAMP },
  { name: '星點', rgb: STAR },
  { name: '遠層樓身', rgb: FAR_FILL },
  { name: '遠層燈點', rgb: FAR_DOT },
  { name: '中層樓頂邊光', rgb: MID_TINT },
  { name: '101 航空燈', rgb: BEACON },
  { name: '101 尖塔', rgb: SPIRE },
  { name: '遠山 1', rgb: MOUNT1 },
  { name: '遠山 2', rgb: MOUNT2 },
];

const rgba = (c, a) => `rgba(${c[0]},${c[1]},${c[2]},${a})`;
const clamp = (v, lo, hi) => (v < lo ? lo : v > hi ? hi : v);
const lerp = (a, b, t) => a + (b - a) * t;

// ── 版面 ─────────────────────────────────────────────────────────────────────────────

/** 資料帶外擴（S 倍）：電線光暈 12S、節點半徑 5S、影線線寬，取 16S。 */
const DATA_PAD = 16;
/** 過期標示估計寬度（字級倍數）：「資料停在 12/31」約 6.9 字寬，取 8 保守。 */
const STALE_WIDTH_EM = 8;

/**
 * 版面（畫面座標）。垂直以 Href＝min(H, 0.75W) 為基準、貼齊畫面底部；16:9 時與樣稿相同（地面 0.995H、101 高 0.6H）。
 * 資料：價格最低（v=0）在 floorY、最高（v=1）在 floorY−span；每棟資料樓至少有 0.12Href 的「基座」。
 */
export function skylineLayout(W, H, S) {
  const Href = Math.min(H, W * 0.75);
  const baseY = H - 0.005 * Href;
  const portrait = W < H;
  const floorY = baseY - 0.12 * Href;
  const span = 0.28 * Href;
  const captionY1 = baseY - 0.72 * Href;
  const slots = candleSlots(CANDLE_COUNT, { xL: W * 0.012, xR: W * 0.988, fill: 0.7 });
  const staleFont = Math.round(26 * S);
  const staleHalo = 10 * S;
  const dataPad = DATA_PAD * S;
  return {
    Href,
    baseY,
    baseYFar: H - 0.035 * Href,
    baseYMid: H - 0.015 * Href,
    skyTop: H - Href,
    starMaxY: H - 0.5 * Href,
    mountY1: H - 0.4 * Href,
    mountY2: H - 0.375 * Href,
    // 101：樣稿在 0.335W，正好壓在第 6 根 K 正後方；改放第 6、7 根之間的間隙正中（約 0.354W，修正輪 1）
    towerCx: (slots[6].x + slots[6].w + slots[7].x) / 2,
    towerH: 0.6 * Href,
    floorY,
    span,
    slots,
    captionX: W * (portrait ? 0.06 : 0.09),
    captionY1,
    captionY2: captionY1 + 40 * S,
    staleX: W - 72 * S,
    // 過期標示：3.5 的 0.925H，但不得高於「資料可能的最低點（v=0）＋外擴＋柔光＋字高」——直式時 0.925H 會落進資料帶（修正輪 1）
    staleY: Math.max(H * 0.925, floorY + dataPad + staleHalo + staleFont),
    staleFont,
    staleHalo,
    dataPad,
  };
}

/**
 * 資料帶的框（畫面座標）：20 根 K 的最高價、最低價與 20MA 節點的 y 範圍，左右從第一槽到最後一槽，四周外擴 dataPad。
 * 頁面把它登記進 env.boxes（label 'data-band'），截圖稿的版面檢查因此會斷言所有文字都不壓在資料上。
 */
export function dataBandBox(candles, geom, L) {
  let top = Infinity;
  let bot = -Infinity;
  geom.forEach((g, k) => {
    const ys = candles[k].ma === null ? [g.yHigh, g.yLow] : [g.yHigh, g.yLow, g.yMa];
    top = Math.min(top, ...ys);
    bot = Math.max(bot, ...ys);
  });
  const first = L.slots[0];
  const last = L.slots[L.slots.length - 1];
  const x = first.x - L.dataPad;
  const y = top - L.dataPad;
  return { label: 'data-band', x, y, w: last.x + last.w + L.dataPad - x, h: bot + L.dataPad - y };
}

/** 過期標示的保守估計框（含柔光；右對齊在 staleX、基線 y）。供 Node 測試用，頁面另以實測墨跡登記。 */
export function staleBoxEstimate(L, y = L.staleY) {
  const f = L.staleFont;
  const w = f * STALE_WIDTH_EM;
  return { label: 'stale-est', x: L.staleX - w - L.staleHalo, y: y - f - L.staleHalo, w: w + 2 * L.staleHalo, h: f * 1.3 + 2 * L.staleHalo };
}

// ── 進入點 ───────────────────────────────────────────────────────────────────────────

/**
 * 畫一張天際線。缺資料或不足 20 筆 → 拋錯（runWallpaper 走 phase 'error'、宿主保留舊圖）。
 * @param {object} env runWallpaper 傳入的環境
 * @param {{ defaults?: object }} [opts] `defaults` 省略時讀內建預設設定檔
 */
export async function drawSkyline(env, opts = {}) {
  const defaults = opts.defaults ?? (await loadDefaultConfig());
  const cfg = env.withDefaults(defaults);
  const model = buildSkylineModel(env.data, cfg, env.nowMs);
  for (const w of model.warnings) env.warn(w);
  model.layout = skylineLayout(env.W, env.H, env.S);
  model.geom = model.candles.map((c) => candleGeom(c, model.range, model.layout));
  paint(env, model);
  return model;
}

/** 一行摘要（截圖稿記進證據檔）。 */
export function summarize(m) {
  const last = m.candles[m.candles.length - 1];
  let below = 0;
  let above = 0;
  let cross = 0;
  for (const c of m.candles) {
    if (c.ma === null) continue;
    if (c.ma < c.low) below++;
    else if (c.ma > c.high) above++;
    else cross++;
  }
  const s = m.stale;
  return (
    `${m.caption.range}｜資料 ${m.rowCount} 筆、K ${m.candles.length} 根、MA${MA_PERIOD} ${m.maCount} 根｜` +
    `最右 ${last.date} 開 ${formatPrice(last.open)} 高 ${formatPrice(last.high)} 低 ${formatPrice(last.low)} 收 ${formatPrice(last.close)}` +
    `（${candleDirection(last)}）MA${MA_PERIOD} ${last.ma === null ? '—' : formatPrice(last.ma)}｜` +
    `範圍 ${formatPrice(m.range.lo)}–${formatPrice(m.range.hi)}${m.range.flat ? '（平盤，以最小範圍置中）' : ''}｜` +
    `MA 在 K 下方 ${below}、上方 ${above}、穿過 ${cross}｜seed ${m.seed}｜` +
    `最近已收盤 ${s.lastClosed}，落後 ${s.behind} 個交易日｜過期標示：${s.text ?? '不顯示'}`
  );
}

// ── 繪圖 ─────────────────────────────────────────────────────────────────────────────

function paint(env, model) {
  const { ctx, W, H, S } = env;
  const L = model.layout;
  const Href = L.Href;
  const rand = mulberry32(model.seed);

  /** 窗燈（樣稿）：格子小而密，大部分暗、少數特別亮。 */
  function windows(x, y, w, h, cell, litChance, topSkip) {
    const cols = Math.max(1, Math.floor(w / cell));
    const rows = Math.max(1, Math.floor(h / cell));
    const mx = (w - cols * cell) / 2;
    const my = (h - rows * cell) / 2;
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        if (r < topSkip) continue;
        if (rand() > litChance) continue;
        const wx = x + mx + c * cell + cell * 0.3;
        const wy = y + my + r * cell + cell * 0.3;
        const ws = cell * 0.4;
        const bright = rand() < 0.13;
        const b = bright ? 0.7 + rand() * 0.3 : 0.1 + rand() * 0.2;
        ctx.fillStyle = rgba(LAMP, b.toFixed(2));
        ctx.fillRect(wx, wy, ws, ws);
      }
    }
  }

  /** 標示文字所在的區域（略大於兩行字的實際範圍）。 */
  const inCaptionArea = (px, py) =>
    px > L.captionX - 24 * S && px < L.captionX + 680 * S && py > L.captionY1 - 48 * S && py < L.captionY2 + 24 * S;

  // ---- 天空（樣稿漸層；直式時漸層只鋪在下方 Href 高度，上方延續最深的夜色）＋早星 ----
  const sky = ctx.createLinearGradient(0, L.skyTop, 0, H);
  sky.addColorStop(0, '#0b0d24');
  sky.addColorStop(0.4, '#241a44');
  sky.addColorStop(0.66, '#c8643c');
  sky.addColorStop(1, '#7a4230');
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, W, H);
  for (let i = 0; i < 46; i++) {
    const sx = rand() * W;
    const sy = rand() * L.starMaxY;
    const r = rand() * 1.6 + 0.4;
    const a = rand() * 0.5 + 0.3;
    if (inCaptionArea(sx, sy)) continue; // 標示文字後方不放星點（亂數照常取用，其他星點位置不變）
    ctx.beginPath();
    ctx.arc(sx, sy, r * 2 * S, 0, Math.PI * 2);
    ctx.fillStyle = rgba(STAR, a.toFixed(2));
    ctx.fill();
  }

  // ---- 遠山（樣稿：兩層霧紫）----
  const ridge = (baseY, amp, alpha, col) => {
    const n = 26;
    const pts = [];
    for (let i = 0; i <= n; i++) {
      pts.push([(W / n) * i, baseY + (rand() - 0.5) * Href * amp * 0.6 - Math.sin((i / n) * Math.PI) * Href * amp]);
    }
    ctx.beginPath();
    ctx.moveTo(0, H);
    ctx.lineTo(0, pts[0][1]);
    for (const [x, y] of pts) ctx.lineTo(x, y);
    ctx.lineTo(W, H);
    ctx.closePath();
    ctx.fillStyle = rgba(col, alpha);
    ctx.fill();
  };
  ridge(L.mountY1, 0.028, 0.2, MOUNT1);
  ridge(L.mountY2, 0.03, 0.26, MOUNT2);

  // ---- 最遠層（樣稿）：淡紫灰剪影＋極少量小燈點 ----
  let x = 0;
  while (x < W) {
    const w = lerp(W * 0.022, W * 0.05, rand());
    const hgt = lerp(Href * 0.035, Href * 0.085, rand());
    const top = L.baseYFar - hgt;
    ctx.fillStyle = rgba(FAR_FILL, 0.24);
    ctx.fillRect(x, top, w, hgt);
    if (rand() < 0.55) {
      const dx = x + rand() * w;
      const dy = top + hgt * (0.25 + rand() * 0.65);
      const d = Math.max(1, 2.4 * S);
      ctx.fillStyle = rgba(FAR_DOT, (0.14 + rand() * 0.16).toFixed(2));
      ctx.fillRect(dx, dy, d, d);
    }
    x += w + lerp(W * 0.002, W * 0.01, rand());
  }

  // ---- 地面暖光霧（樣稿）----
  const glow = ctx.createLinearGradient(0, L.baseY - Href * 0.16, 0, L.baseY + Href * 0.02);
  glow.addColorStop(0, 'rgba(255,150,90,0)');
  glow.addColorStop(1, 'rgba(255,150,90,0.09)');
  ctx.fillStyle = glow;
  ctx.fillRect(0, L.baseY - Href * 0.16, W, Href * 0.18);

  // ---- 台北 101（樣稿造型；改畫在中層之前，基部由中層與近層遮住）----
  taipei101(L.towerCx, L.baseY, L.towerH);

  // ---- 中層（樣稿的亂數樓群）：中性樓頂邊光、不畫影線（不讀成 K 線）；右側（小工具區）較矮較暗 ----
  const winCell = (w) => clamp(w / 7, 6.19 * S, 16 * S); // 樣稿 clamp(w/7, W/620, W/240)
  x = 0;
  while (x < W) {
    const w = lerp(W * 0.02, W * 0.046, rand());
    const dim = x / W > 0.47;
    const hgt = lerp(dim ? Href * 0.06 : Href * 0.1, dim ? Href * 0.14 : Href * 0.24, rand());
    const top = L.baseYMid - hgt;
    ctx.fillStyle = dim ? 'rgba(20,22,36,0.80)' : 'rgba(14,16,28,0.90)';
    ctx.fillRect(x, top, w, hgt);
    ctx.fillStyle = rgba(MID_TINT, dim ? 0.12 : 0.22);
    ctx.fillRect(x, top, w, Math.max(2 * S, hgt * 0.024));
    windows(x, top, w, hgt, winCell(w), dim ? 0.1 : 0.16, 1);
    x += w + lerp(W * 0.003, W * 0.01, rand());
  }

  // ---- 近層：20 根真實日 K ----
  model.candles.forEach((c, k) => candleBuilding(L.slots[k], model.geom[k], candleDirection(c)));

  // ---- 20MA 電線 ----
  const nodes = [];
  model.candles.forEach((c, k) => {
    if (c.ma !== null) nodes.push([L.slots[k].cx, model.geom[k].yMa]);
  });
  ctx.save();
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  if (nodes.length > 1) {
    ctx.beginPath();
    ctx.moveTo(nodes[0][0], nodes[0][1]);
    for (let i = 1; i < nodes.length; i++) ctx.lineTo(nodes[i][0], nodes[i][1]);
    ctx.strokeStyle = rgba(WIRE, 0.9);
    ctx.lineWidth = 3.2 * S;
    ctx.shadowColor = rgba(WIRE, 0.6);
    ctx.shadowBlur = 12 * S;
    ctx.stroke();
  }
  ctx.shadowBlur = 0;
  for (const [nx, ny] of nodes) {
    ctx.beginPath();
    ctx.arc(nx, ny, 5 * S, 0, Math.PI * 2);
    ctx.fillStyle = rgba(WIRE, 0.95);
    ctx.fill();
  }
  ctx.restore();

  // ---- 極輕 overlay 雜訊（樣稿 addDither(0.03)）----
  const tile = document.createElement('canvas');
  tile.width = tile.height = 96;
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
  ctx.globalAlpha = 0.03;
  ctx.globalCompositeOperation = 'overlay';
  ctx.fillStyle = ctx.createPattern(tile, 'repeat');
  ctx.fillRect(0, 0, W, H);
  ctx.restore();

  // ---- 資料帶登記進版面檢查（不是文字；讓截圖稿斷言所有文字都不壓在 K 線與電線上，修正輪 1）----
  env.boxes.push(dataBandBox(model.candles, model.geom, L));

  // ---- 文字：左上方兩行標示、右下角過期標示 ----
  const capFont = `600 ${Math.round(24 * S)}px ${MONO}`;
  const capHalo = { color: 'rgba(11,13,36,0.9)', blur: 8 * S };
  drawLabel(env, { label: 'caption-range', text: model.caption.range, x: L.captionX, y: L.captionY1, font: capFont, color: rgba(CAPTION, 0.78), halo: capHalo });
  drawLabel(env, { label: 'caption-values', text: model.caption.values, x: L.captionX, y: L.captionY2, font: capFont, color: rgba(CAPTION, 0.78), halo: capHalo });
  if (model.stale.show) {
    drawLabel(env, {
      label: 'stale',
      text: model.stale.text,
      x: L.staleX,
      y: L.staleY,
      font: `500 ${L.staleFont}px ${SANS}`,
      color: rgba(STALE, 0.88),
      align: 'right',
      halo: { color: rgba(INK, 0.95), blur: L.staleHalo },
    });
  }

  // ── 內部函式 ──

  /** 一根 K 線＝一棟樓：天線＝上影線、樓身到實體上緣、實體淡染＋窗燈較多、樓頂紅綠邊光、下影線為樓面細亮線。 */
  function candleBuilding(slot, g, dir) {
    const { x: bx, w, cx } = slot;
    const tint = dir === 'up' ? RED : dir === 'down' ? GRN : FLAT_K;
    const lineW = Math.max(1, 1.75 * S); // 樣稿 max(1, W/2200)
    ctx.strokeStyle = rgba(WICK, 0.42);
    ctx.lineWidth = lineW;
    if (g.yHigh < g.yBodyTop) {
      ctx.beginPath();
      ctx.moveTo(cx, g.yBodyTop);
      ctx.lineTo(cx, g.yHigh);
      ctx.stroke();
    }
    const hgt = L.baseY - g.yBodyTop;
    ctx.fillStyle = rgba(INK, 0.97);
    ctx.fillRect(bx, g.yBodyTop, w, hgt);
    const bodyH = g.yBodyBot - g.yBodyTop;
    ctx.fillStyle = rgba(tint, 0.13);
    ctx.fillRect(bx, g.yBodyTop, w, bodyH);
    // 窗燈：實體段落點亮較多（「亮著的樓層＝當日開收區間」），其下樓層稀疏暗燈
    const cell = winCell(w);
    const cols = Math.max(1, Math.floor(w / cell));
    const rows = Math.max(1, Math.floor(hgt / cell));
    const mx = (w - cols * cell) / 2;
    for (let r = 0; r < rows; r++) {
      const wy = g.yBodyTop + r * cell + cell * 0.3;
      const ws = cell * 0.4;
      const inBody = wy + ws / 2 <= g.yBodyBot;
      for (let c = 0; c < cols; c++) {
        if (rand() > (inBody ? 0.34 : 0.07)) continue;
        const bright = rand() < (inBody ? 0.2 : 0.05);
        const b = bright ? 0.7 + rand() * 0.3 : 0.1 + rand() * 0.2;
        ctx.fillStyle = rgba(LAMP, b.toFixed(2));
        ctx.fillRect(bx + mx + c * cell + cell * 0.3, wy, ws, ws);
      }
    }
    if (g.yLow > g.yBodyBot) {
      ctx.beginPath();
      ctx.moveTo(cx, g.yBodyBot);
      ctx.lineTo(cx, g.yLow);
      ctx.stroke();
    }
    ctx.fillStyle = rgba(tint, 0.85);
    ctx.fillRect(bx, g.yBodyTop, w, clamp(bodyH, 2 * S, 6 * S));
  }

  /** 台北 101（樣稿）：基座＋8 節上寬下窄的斗狀疊塔、節縫亮邊、尖塔＋頂端航空燈。 */
  function taipei101(cx, baseY, totalH) {
    const baseW = totalH * 0.2;
    const podiumH = totalH * 0.085;
    const podiumTop = baseY - podiumH;
    ctx.fillStyle = 'rgba(7,8,15,0.98)';
    ctx.fillRect(cx - baseW / 2, podiumTop, baseW, podiumH + 1);
    windows(cx - baseW / 2, podiumTop, baseW, podiumH, Math.max(8 * S, 3), 0.3, 0);
    const segs = 8;
    const segH = (totalH * 0.8) / segs;
    let y = podiumTop;
    const topWStart = baseW * 0.88;
    const topWEnd = totalH * 0.078;
    for (let i = 0; i < segs; i++) {
      const segTopW = lerp(topWStart, topWEnd, i / (segs - 1));
      const segBotW = segTopW * 0.73;
      const top = y - segH;
      ctx.beginPath();
      ctx.moveTo(cx - segBotW / 2, y);
      ctx.lineTo(cx - segTopW / 2, top);
      ctx.lineTo(cx + segTopW / 2, top);
      ctx.lineTo(cx + segBotW / 2, y);
      ctx.closePath();
      ctx.fillStyle = 'rgba(7,8,15,0.985)';
      ctx.fill();
      ctx.fillStyle = rgba(LAMP, 0.16);
      ctx.fillRect(cx - segTopW / 2, top, segTopW, Math.max(1, segH * 0.05));
      windows(cx - segTopW / 2, top, segTopW, segH, Math.max(6.86 * S, 2.5), 0.32, 0);
      y = top;
    }
    const spireH = totalH * 0.15;
    ctx.strokeStyle = rgba(SPIRE, 0.55);
    ctx.lineWidth = Math.max(1.5, 2.74 * S);
    ctx.beginPath();
    ctx.moveTo(cx, y);
    ctx.lineTo(cx, y - spireH);
    ctx.stroke();
    ctx.beginPath();
    ctx.arc(cx, y - spireH, Math.max(2.5, 4.27 * S), 0, Math.PI * 2);
    ctx.fillStyle = rgba(BEACON, 1);
    ctx.shadowColor = rgba(BEACON, 1);
    ctx.shadowBlur = 17.45 * S;
    ctx.fill();
    ctx.shadowBlur = 0;
  }
}
