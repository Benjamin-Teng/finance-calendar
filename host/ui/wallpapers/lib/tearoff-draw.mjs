// host/ui/wallpapers/lib/tearoff-draw.mjs
//
// 撕日曆（task 3.4）的繪圖：把 `tearoff-model.mjs` 算好的模型畫到 canvas。畫風與構圖忠實移植樣稿
// `assets/design-explore/03-tearoff/bg.html`（暗漆紅底、回紋暗花、左側微傾的撕頁日曆、下方撕剩鋸齒紙邊、
// 裝訂條、累加游標排版），只改資料來源與下列項目（理由見 task 3.4 報告）：
//   - 日期、星期、干支年、月份來自本機日期；「宜」「忌」來自規則（tearoff-model.mjs）與 `env.config` 的詞庫／門檻。
//   - 尺寸基準改用共用契約 `S = min(W,H)/2160`，日曆頁寬＝864×S（樣稿是 W×0.225）：16:9 與樣稿完全相同；
//     其他長寬比依短邊等比，直式（1080×1920）不會縮成小紙片、超寬（2560×1080）的紙邊不會掉出畫面底部。
//   - 「宜」「忌」可有多項，每項一行（標籤只畫在第一行）；行數多到頁面放不下時整組等比縮小字級與行距。
//   - 每一項單行呈現；超過可用寬度時逐步縮小該行字級直到放得下（不截斷、不換行、不超出頁面），沒有硬下限。
//     低於可讀下限（可用寬度 ÷ 24，即 24 個全形字的寬度）時記警告；每行另以實際墨跡框核對仍在紙頁內，不在就記警告。
//   - 字型全部內建：Noto Serif TC 900（日）、霞鶩文楷 TC 400（年／月／宜忌內容）、700（宜忌標籤）。
//   - 每個文字元素經目前的旋轉矩陣換算成畫面座標的外接框，登記到 `env.boxes`（版面斷言用）。
// 不用計時器與 requestAnimationFrame（design.md D2）。

import { FONT_REQUIREMENTS, mulberry32, textBox } from './core.mjs';
import { computeTearoff, fitFontPx, localDate, transformedAabb } from './tearoff-model.mjs';
import { DEFAULT_CONFIG_URL, loadDefaultConfig } from './default-config.mjs';

/** 撕日曆實際用到的字型（`runWallpaper({ fonts })`）。 */
export const TEAROFF_FONT_IDS = ['notoserif-900', 'wenkai-400', 'wenkai-700'];
export const TEAROFF_FONTS = FONT_REQUIREMENTS.filter((r) => TEAROFF_FONT_IDS.includes(r.id));

// 內建預設設定檔的讀取在 task 3.5 抽到 default-config.mjs（脊線、等高線共用），這裡原樣轉出。
export { DEFAULT_CONFIG_URL, loadDefaultConfig };

const WENKAI = '"LXGW WenKai TC"';
const SERIF = '"Noto Serif TC"';
const SEED = 20260728; // 樣稿的亂數種子（鋸齒紙邊與雜訊）

const PAPER = [239, 230, 210];
const RED = '#c8242b';
const RED_DEEP = '#8c161c';
const GOLD = [201, 162, 74];

/** 可讀下限：可用寬度 ÷ 24（24 個全形字）。低於它仍照縮（不超出頁面），只記警告。20 字的詞庫句不會碰到。 */
export const READABLE_FONT_CHARS = 24;

/**
 * 畫一張撕日曆。回傳資料模型（另加 `lines`：宜忌每行的字級與縮字結果，摘要與測試用）。
 * @param {object} env runWallpaper 傳入的環境
 * @param {{ defaults?: object }} [opts] `defaults` 省略時讀內建預設檔
 */
export async function drawTearoff(env, opts = {}) {
  const defaults = opts.defaults ?? (await loadDefaultConfig());
  const cfg = env.withDefaults(defaults);
  const model = computeTearoff(cfg, env.data, localDate(env.tz, env.nowMs), { fallback: defaults, nowMs: env.nowMs });
  for (const w of model.warnings) env.warn(w);
  model.lines = paint(env, model);
  return model;
}

/** 一行摘要（截圖稿記進證據檔）。 */
export function summarize(model) {
  const state = model.closed ? `休市（${model.closedReason}）` : '交易日';
  const shrunk = (model.lines ?? []).filter((l) => l.px !== l.basePx).map((l) => `${l.label} ${l.basePx}→${+l.px.toFixed(3)}px${l.belowReadable ? '（低於可讀下限）' : ''}${l.inPage ? '' : '（超出紙頁！）'}`);
  return (
    `${model.date} ${model.ganzhi ?? '（無干支）'}　${model.weekday} ${model.month}${model.day}日｜${state}｜` +
    `宜：${model.yi.join('、')}（${model.yiFrom}）｜忌：${model.ji.join('、')}（${model.jiFrom}）｜縮字：${shrunk.join('、') || '無'}`
  );
}

function paint(env, model) {
  const { ctx, W, H, S, boxes } = env;
  const rnd = mulberry32(SEED);
  const lines = [];

  /** 寫字並登記「畫面座標」的外接框（目前的旋轉矩陣已套用）。 */
  function text(label, str, x, y) {
    ctx.fillText(str, x, y);
    boxes.push(transformedAabb(textBox(ctx, label, str, x, y), ctx.getTransform()));
  }

  // ---- 背景：暗漆紅黑漸層 ----
  ctx.fillStyle = '#140a0a';
  ctx.fillRect(0, 0, W, H);
  const bgGrad = ctx.createLinearGradient(W * 0.05, H * 0.05, W * 0.85, H * 0.95);
  bgGrad.addColorStop(0, '#3a1216');
  bgGrad.addColorStop(0.42, '#2a0f12');
  bgGrad.addColorStop(1, '#140a0a');
  ctx.fillStyle = bgGrad;
  ctx.fillRect(0, 0, W, H);
  const glow = ctx.createRadialGradient(W * 0.24, H * 0.2, H * 0.02, W * 0.26, H * 0.32, H * 0.65);
  glow.addColorStop(0, 'rgba(120,40,32,0.42)');
  glow.addColorStop(1, 'rgba(20,10,10,0)');
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, W, H);

  // ---- 回紋暗花底紋：大面積、低對比 ----
  {
    const t = Math.round(150 * S);
    const off = document.createElement('canvas');
    off.width = off.height = t;
    const o = off.getContext('2d');
    o.strokeStyle = `rgba(${GOLD[0]},${GOLD[1]},${GOLD[2]},0.55)`;
    o.lineWidth = Math.max(1.2, 1.7 * S);
    const s1 = t * 0.5;
    const s2 = t * 0.26;
    const frets = (cx, cy) => {
      o.beginPath();
      o.moveTo(cx - s1 / 2, cy - s1 / 2);
      o.lineTo(cx + s1 / 2, cy - s1 / 2);
      o.lineTo(cx + s1 / 2, cy + s2 / 2);
      o.lineTo(cx - s2 / 2, cy + s2 / 2);
      o.lineTo(cx - s2 / 2, cy - s2 / 2);
      o.lineTo(cx + s2 * 0.05, cy - s2 / 2);
      o.stroke();
    };
    frets(t * 0.28, t * 0.28);
    frets(t * 0.78, t * 0.78);
    const pat = ctx.createPattern(off, 'repeat');
    ctx.save();
    ctx.globalAlpha = 0.12;
    ctx.fillStyle = pat;
    ctx.fillRect(0, 0, W, H);
    ctx.restore();
  }

  // ---- 撕頁日曆：直式（約 3:4），位置在左中偏下 ----
  const stackW = 864 * S; // 樣稿 W×0.225（3840 寬時 864）；改依短邊等比
  const pageH = stackW * 1.34;
  // 橫式與樣稿相同（W×0.24）；直式時改為讓紙頁左緣落在左側壓暗帶（W×0.10）之外，否則紙面左側會被壓暗
  const stackCX = Math.max(W * 0.24, W * 0.1 + stackW * 0.5 + 24 * S);
  const stackTop = H * 0.255;
  const rot = -0.022; // 微微傾斜

  ctx.save();
  ctx.translate(stackCX, stackTop + pageH * 0.5);
  ctx.rotate(rot);
  ctx.translate(-stackCX, -(stackTop + pageH * 0.5));

  // 陰影（光源左上，影子往右下）
  ctx.save();
  ctx.shadowColor = 'rgba(0,0,0,0.55)';
  ctx.shadowBlur = 60 * S;
  ctx.shadowOffsetX = 26 * S;
  ctx.shadowOffsetY = 34 * S;
  ctx.fillStyle = 'rgba(0,0,0,0.001)';
  ctx.fillRect(stackCX - stackW / 2, stackTop, stackW, pageH);
  ctx.restore();

  // 下方撕剩鋸齒紙邊（幾層，越下面越暗越窄）
  function jagged(x, y, w, h, teeth, amp, shadeAlpha) {
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + w, y);
    ctx.lineTo(x + w, y + h * 0.35);
    for (let i = 0; i <= teeth; i++) {
      const xx = x + w - (w / teeth) * i;
      const base = y + h * 0.35 + h * 0.65;
      const yy = base - (i % 2 === 0 ? amp * (0.5 + rnd() * 0.5) : amp * 0.12 * rnd());
      ctx.lineTo(xx, yy);
    }
    ctx.lineTo(x, y + h * 0.35);
    ctx.closePath();
    ctx.fillStyle = `rgba(${PAPER[0]},${PAPER[1]},${PAPER[2]},${shadeAlpha})`;
    ctx.fill();
  }
  // 撕剩鋸齒紙邊：整疊高度抓頁高的約 7%，7 層、每層之間的落差夠大，才看得出一層一層。
  // 越下面（i 越大）越暗越窄＝越裡面的舊頁。
  const stubY = stackTop + pageH - 4 * S;
  const stubTotalH = pageH * 0.07;
  const stubs = 7;
  const stepY = stubTotalH / stubs;
  const bandH = stepY * 2.3;
  for (let i = stubs; i >= 1; i--) {
    const inset = i * (stackW * 0.013);
    jagged(stackCX - stackW / 2 + inset * 0.3, stubY + (stubs - i) * stepY, stackW - inset * 0.6, bandH, 18, stepY * 0.62, 0.46 - i * 0.032);
  }

  // 最上頁（本頁）
  const pageY = stackTop;
  const pageX = stackCX - stackW / 2;
  ctx.fillStyle = `rgb(${PAPER[0]},${PAPER[1]},${PAPER[2]})`;
  ctx.fillRect(pageX, pageY, stackW, pageH);

  // 頁面內側光影（左上略亮、右下略暗，呼應光源）
  const pshade = ctx.createLinearGradient(pageX, pageY, pageX + stackW, pageY + pageH);
  pshade.addColorStop(0, 'rgba(255,250,235,0.10)');
  pshade.addColorStop(0.5, 'rgba(0,0,0,0)');
  pshade.addColorStop(1, 'rgba(20,10,8,0.22)');
  ctx.fillStyle = pshade;
  ctx.fillRect(pageX, pageY, stackW, pageH);
  // 整體再壓暗一階（在暗處只被微光照到）
  ctx.fillStyle = 'rgba(10,4,4,0.28)';
  ctx.fillRect(pageX, pageY, stackW, pageH);

  // 裝訂條
  const bindH = pageH * 0.052;
  ctx.fillStyle = RED_DEEP;
  ctx.fillRect(pageX, pageY, stackW, bindH);
  ctx.fillStyle = RED;
  ctx.fillRect(pageX, pageY, stackW, bindH * 0.62);
  for (let r = 0; r < 3; r++) {
    const rx = pageX + stackW * (0.22 + r * 0.28);
    ctx.beginPath();
    ctx.fillStyle = '#3a1216';
    ctx.arc(rx, pageY + bindH * 0.5, bindH * 0.16, 0, 6.283);
    ctx.fill();
  }

  // 裝訂處殘留的撕角碎紙（前幾頁被撕掉後，孔邊留下的小紙角）
  function tornFleck(rx0, w0) {
    ctx.beginPath();
    ctx.moveTo(rx0, pageY + bindH);
    ctx.lineTo(rx0 + w0, pageY + bindH);
    ctx.lineTo(rx0 + w0 * 0.7, pageY + bindH + 5 * S);
    ctx.lineTo(rx0 + w0 * 0.35, pageY + bindH + 3 * S);
    ctx.closePath();
    ctx.fillStyle = 'rgba(200,60,60,0.35)';
    ctx.fill();
  }
  tornFleck(pageX + stackW * 0.1, stackW * 0.07);
  tornFleck(pageX + stackW * 0.62, stackW * 0.09);

  // 文字：一律 textBaseline='top'，用「累加游標」排版，逐段算實際佔用高度再往下推
  // （樣稿註解：位置若照頁高固定比例猜，字一大就會「壓到裝訂條」「宜／忌互疊」）。
  const cx = pageX + stackW / 2;
  let cursorY = pageY + bindH + stackW * 0.06; // 裝訂條下方留的邊距

  ctx.textAlign = 'center';
  ctx.textBaseline = 'top';

  // 年／週：頁寬約 5%
  const f1 = stackW * 0.05;
  ctx.fillStyle = 'rgba(90,20,22,0.85)';
  ctx.font = `${Math.round(f1)}px ${WENKAI}`;
  text('year', model.ganzhi ? `${model.ganzhi}　${model.weekday}` : model.weekday, cx, cursorY);
  cursorY += f1 * 1.35 + stackW * 0.035;

  // 日：大字
  const f2 = stackW * 0.44;
  ctx.fillStyle = RED;
  ctx.font = `900 ${Math.round(f2)}px ${SERIF}`;
  text('day', model.day, cx, cursorY);
  cursorY += f2 * 1.02 + stackW * 0.025;

  // 月：小字
  const f3 = stackW * 0.062;
  ctx.font = `${Math.round(f3)}px ${WENKAI}`;
  ctx.fillStyle = 'rgba(60,20,20,0.7)';
  text('month', model.month, cx, cursorY);
  cursorY += f3 * 1.35 + stackW * 0.09;

  // 宜／忌：頁寬約 6%，行距 1.6 倍，標籤與內容間隔 1 字寬；每項一行。
  // 行數多到頁面放不下（頁底留頁寬 5%）時，整組等比縮小。
  const groups = [
    { key: 'yi', label: '宜', items: model.yi },
    { key: 'ji', label: '忌', items: model.ji },
  ];
  const nLines = groups.reduce((n, g) => n + Math.max(1, g.items.length), 0);
  const availH = pageY + pageH - stackW * 0.05 - cursorY;
  const f4 = Math.min(stackW * 0.06, availH / (1.6 * nLines - 0.6));
  const lineH = f4 * 1.6;
  const padX = stackW * 0.11;
  const contentX = padX + f4 * 2.0; // 標籤本身佔約 1 字寬，再留 1 字寬間隔
  const availW = stackW - padX - contentX; // 右側留與左側相同的 padX
  const readablePx = availW / READABLE_FONT_CHARS;
  const basePx = Math.round(f4 * 0.92);
  ctx.textAlign = 'left';

  for (const g of groups) {
    ctx.font = `700 ${Math.round(f4)}px ${WENKAI}`;
    ctx.fillStyle = RED_DEEP;
    text(`${g.key}:label`, g.label, pageX + padX, cursorY);
    g.items.forEach((item, i) => {
      const fit = fitFontPx({
        measure: (px) => {
          ctx.font = `${px}px ${WENKAI}`;
          return ctx.measureText(item).width;
        },
        basePx,
        maxW: availW,
        readablePx,
      });
      ctx.font = `${fit.px}px ${WENKAI}`;
      ctx.fillStyle = 'rgba(70,25,24,0.82)';
      // 縮小的行在原本那一行的高度內置中
      const y = cursorY + (basePx - fit.px) * 0.5;
      text(`${g.key}:${i}`, item, pageX + contentX, y);
      // 紙頁座標（旋轉前）的實際墨跡框：右緣不得超過紙頁右緣減 padX 的一半（spec：不超出頁面）
      const local = textBox(ctx, '', item, pageX + contentX, y);
      const inPage = fit.fits && local.x + local.w <= pageX + stackW - padX * 0.5;
      lines.push({ label: `${g.key}:${i}`, text: item, basePx, px: fit.px, belowReadable: fit.belowReadable, inPage, availW: Math.round(availW) });
      if (fit.belowReadable) {
        env.warn(`「${item}」縮到 ${+fit.px.toFixed(3)}px，低於可讀下限 ${+readablePx.toFixed(1)}px（可用寬度 ${Math.round(availW)}px ÷ ${READABLE_FONT_CHARS}）；仍單行、在頁面內`);
      }
      if (!inPage) env.warn(`「${item}」超出紙頁（墨跡右緣 ${Math.round(local.x + local.w)} > ${Math.round(pageX + stackW - padX * 0.5)}）`);
      cursorY += lineH;
    });
    if (g.items.length === 0) cursorY += lineH;
  }

  ctx.textBaseline = 'alphabetic';

  // 頁面邊緣淡淡描邊，增加紙感
  ctx.strokeStyle = 'rgba(40,15,12,0.35)';
  ctx.lineWidth = 1.5 * S;
  ctx.strokeRect(pageX, pageY, stackW, pageH);

  ctx.restore(); // rotate

  // ---- 暗角 ----
  const vig = ctx.createRadialGradient(W * 0.5, H * 0.5, H * 0.3, W * 0.5, H * 0.5, H * 0.95);
  vig.addColorStop(0, 'rgba(0,0,0,0)');
  vig.addColorStop(1, 'rgba(0,0,0,0.42)');
  ctx.fillStyle = vig;
  ctx.fillRect(0, 0, W, H);

  // 左側圖示欄壓暗
  const lGrad = ctx.createLinearGradient(0, 0, W * 0.1, 0);
  lGrad.addColorStop(0, 'rgba(6,2,2,0.72)');
  lGrad.addColorStop(0.6, 'rgba(6,2,2,0.30)');
  lGrad.addColorStop(1, 'rgba(6,2,2,0)');
  ctx.fillStyle = lGrad;
  ctx.fillRect(0, 0, W, H);

  // 右上（小工具區）與底部工作列壓暗
  const corner = ctx.createRadialGradient(W * 0.98, H * 0.03, H * 0.05, W * 0.98, H * 0.03, H * 1.05);
  corner.addColorStop(0, 'rgba(6,2,2,0.35)');
  corner.addColorStop(1, 'rgba(6,2,2,0)');
  ctx.fillStyle = corner;
  ctx.fillRect(0, 0, W, H);
  const bGrad = ctx.createLinearGradient(0, H * 0.93, 0, H);
  bGrad.addColorStop(0, 'rgba(5,2,2,0)');
  bGrad.addColorStop(1, 'rgba(5,2,2,0.55)');
  ctx.fillStyle = bGrad;
  ctx.fillRect(0, H * 0.93, W, H * 0.07);

  // ---- 極輕微雜訊，避免色帶 ----
  const tile = 160;
  const offN = document.createElement('canvas');
  offN.width = offN.height = tile;
  const octx = offN.getContext('2d');
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
  ctx.fillStyle = ctx.createPattern(offN, 'repeat');
  ctx.globalAlpha = 0.5;
  ctx.fillRect(0, 0, W, H);
  ctx.globalAlpha = 1;
  ctx.globalCompositeOperation = 'source-over';

  return lines;
}
