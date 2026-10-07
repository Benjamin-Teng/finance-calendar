// host/ui/wallpapers/lib/astrolabe-draw.mjs
//
// 星盤（task 3.3）的繪圖：把 `astrolabe-sessions.mjs` 算好的資料模型畫到 canvas。
// 畫風與構圖忠實移植樣稿 `assets/design-explore/05-astrolabe/bg-sessions.html` 的 full 構圖
// （整盤置中、「現在」指標固定在正上方、盤面隨時間轉動、只畫現在前 10 小時到後 12 小時），
// 只改資料來源與下列項目（理由見 task 3.3 報告）：
//   - 時段表與休市表來自 `env.config`（缺鍵由內建預設檔 `config/wallpaper-config.default.json` 補）。
//   - 休市日：整條環變暗（透明度 ×0.3、不發光），狀態文字「休市（假日名）」。
//   - 盤面半徑改為短邊的一半（樣稿是高的一半）：直式螢幕（1080×1920）整盤不被左右裁切；橫式與樣稿相同。
//   - 中文字型只用內建的 Noto Sans TC（樣稿堆疊 "Microsoft JhengHei UI","Noto Sans TC" 的前者是系統字型、
//     不是內建字型；spec「渲染不依賴網路」）。黑體風格與樣稿相同。
//   - 市場標籤與圖例過寬時縮小字級（不換行、不截斷）；24 小時內沒有開收盤時「接下來」下方寫一行說明。
//   - 每個文字元素以 `textBox` 登記到 `env.boxes`，供截圖稿斷言不出界、不相交。
// 不用計時器與 requestAnimationFrame（design.md D2）。

import { FONT_REQUIREMENTS, mulberry32, textBox, tzParts } from './core.mjs';
import { FUTURE_HOURS, PAST_HOURS, clipToWindow, computeAstrolabe } from './astrolabe-sessions.mjs';

/** 星盤實際用到的字型（`runWallpaper({ fonts })`）：Cinzel 600（時刻環數字）／700（現在時刻）、Noto Sans TC 500／600。 */
export const ASTROLABE_FONT_IDS = ['cinzel-600', 'cinzel-700', 'notosans-500', 'notosans-600'];
export const ASTROLABE_FONTS = FONT_REQUIREMENTS.filter((r) => ASTROLABE_FONT_IDS.includes(r.id));

/** 內建預設設定檔（與 4.1 編進宿主的是同一份）。 */
export const DEFAULT_CONFIG_URL = new URL('../config/wallpaper-config.default.json', import.meta.url).href;

const CJK = '"Noto Sans TC", sans-serif';
const LATIN = '"Cinzel", serif';
const SEED = 19700101;
/** 一條市場軌道分三個子軌：正規＝滿寬；現貨盤前／盤後＝外半；期貨夜盤＝內半（樣稿原值）。 */
const STYLE = {
  regular: { w: 0.82, off: 0, a: 0.62, dash: null },
  auction: { w: 0.82, off: 0, a: 0.85, dash: null },
  pre: { w: 0.34, off: 0.24, a: 0.42, dash: null },
  post: { w: 0.34, off: 0.24, a: 0.42, dash: null },
  night: { w: 0.34, off: -0.24, a: 0.55, dash: [0.55, 0.45] },
};
const DIM = 0.3;

/** 讀內建預設設定檔；讀不到就拋錯（整張失敗、宿主保留舊圖），不會悄悄畫成空盤。 */
export async function loadDefaultConfig(url = DEFAULT_CONFIG_URL) {
  let res;
  try {
    res = await fetch(url, { cache: 'no-store' });
  } catch (e) {
    throw new Error(`內建預設設定讀取失敗：${url}（${e?.message || e}）`);
  }
  if (!res.ok) {
    throw new Error(`內建預設設定讀取失敗：HTTP ${res.status} ${url}`);
  }
  return res.json();
}

/**
 * 畫一張星盤。`env` 是 runWallpaper 傳入的環境；回傳資料模型（測試與摘要用）。
 * @param {object} env
 * @param {{ defaults?: object }} [opts] `defaults` 省略時讀內建預設檔
 */
export async function drawAstrolabe(env, opts = {}) {
  const defaults = opts.defaults ?? (await loadDefaultConfig());
  const cfg = env.withDefaults(defaults);
  const model = computeAstrolabe(cfg, env.nowMs, env.tz);
  for (const w of model.warnings) env.warn(w);
  paint(env, model);
  return model;
}

/** 一行摘要（截圖稿記進證據檔）：現在、各市場狀態、接下來。 */
export function summarize(model) {
  const st = model.markets.map((m) => `${m.name}=${m.statusText}${m.dimmed ? '(暗)' : ''}`).join(' ');
  const up = model.upcoming.map((e) => `${e.timeText}${e.name}${e.what}`).join('、') || '（無）';
  return `現在 ${model.nowText} ${model.tzText}｜${st}｜接下來 ${up}`;
}

function paint(env, model) {
  const { ctx, W, H, S, boxes } = env;
  const cx = W * 0.5;
  const cy = H * 0.5;
  const Rmax = Math.min(W, H) * 0.5;
  const NOW_ANG = -Math.PI / 2;
  // 過去只畫到 −10h：+12h～−10h 之間留 2h 缺口放市場名
  const GAP_HOURS = 24 - PAST_HOURS - FUTURE_HOURS;
  const LABEL_ANG = NOW_ANG + ((FUTURE_HOURS + GAP_HOURS / 2) / 24) * Math.PI * 2;
  const angRel = (hRel) => NOW_ANG + (hRel / 24) * Math.PI * 2;
  const rng = mulberry32(SEED);

  function noiseOverlay(alpha) {
    const n = 96;
    const off = document.createElement('canvas');
    off.width = n;
    off.height = n;
    const octx = off.getContext('2d');
    const id = octx.createImageData(n, n);
    for (let i = 0; i < id.data.length; i += 4) {
      const v = 128 + (rng() - 0.5) * 2 * 14;
      id.data[i] = id.data[i + 1] = id.data[i + 2] = v;
      id.data[i + 3] = 255;
    }
    octx.putImageData(id, 0, 0);
    ctx.save();
    ctx.globalAlpha = alpha;
    ctx.globalCompositeOperation = 'overlay';
    ctx.fillStyle = ctx.createPattern(off, 'repeat');
    ctx.fillRect(0, 0, W, H);
    ctx.restore();
  }

  function drawBackground() {
    const g = ctx.createRadialGradient(cx, cy, Rmax * 0.05, cx, cy, Rmax * 1.35);
    g.addColorStop(0, '#0f1a3a');
    g.addColorStop(0.55, '#0b1330');
    g.addColorStop(1, '#070b1c');
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, W, H);
    const glow = (gx, gy, r, color, a) => {
      const gg = ctx.createRadialGradient(gx, gy, 0, gx, gy, r);
      gg.addColorStop(0, color.replace('ALPHA', a));
      gg.addColorStop(1, color.replace('ALPHA', 0));
      ctx.fillStyle = gg;
      ctx.fillRect(0, 0, W, H);
    };
    glow(W * 0.2, H * 0.16, W * 0.26, 'rgba(120,150,220,ALPHA)', 0.06);
    glow(W * 0.44, H * 0.44, W * 0.22, 'rgba(180,120,170,ALPHA)', 0.045);
    glow(W * 0.7, H * 0.3, W * 0.16, 'rgba(90,200,190,ALPHA)', 0.035);
    const starN = Math.round((W * H) / 6200);
    for (let i = 0; i < starN; i++) {
      const sx = rng() * W;
      const sy = rng() * H;
      const bright = rng() < 0.04;
      const r = (bright ? rng() * 1.6 + 1.6 : rng() * 1.1 + 0.22) * S * 1.78;
      const a = (bright ? rng() * 0.35 + 0.55 : rng() * 0.5 + 0.1) * 0.55;
      ctx.beginPath();
      ctx.arc(sx, sy, r, 0, Math.PI * 2);
      ctx.fillStyle = `rgba(216,223,240,${Math.min(1, a).toFixed(3)})`;
      ctx.fill();
    }
  }

  function ring(r, color, alpha, width) {
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.strokeStyle = color;
    ctx.globalAlpha = alpha;
    ctx.lineWidth = width;
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  /** 寫字並登記邊界框（同一組 font／align／baseline 量測與繪製）。 */
  function text(label, str, x, y) {
    ctx.fillText(str, x, y);
    boxes.push(textBox(ctx, label, str, x, y));
  }

  /** 字級 `px` 下寬度超過 `maxW` 時等比縮小字級（不換行、不截斷）。`fontOf(px)` 回傳 ctx.font 字串。 */
  function fitFont(fontOf, str, px, maxW) {
    ctx.font = fontOf(px);
    const w = ctx.measureText(str).width;
    if (w > maxW) {
      ctx.font = fontOf(Math.max(1, Math.floor((px * maxW) / w)));
    }
  }

  /* 時刻環：刻度跟著盤面轉，數字是顯示時區的鐘點 */
  function drawHourRing() {
    const R = Rmax * 0.855;
    ring(R, '#c8a15a', 0.16, 3.4 * S);
    const p = tzParts(model.tz, model.nowMs);
    const nowH = p.h + p.mi / 60;
    ctx.font = `600 ${Math.round(Rmax * 0.024)}px ${LATIN}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    for (let q = 0; q < 24 * 4; q++) {
      const hh = q / 4;
      const rel = ((((hh - nowH) % 24) + 36) % 24) - 12;
      const a = angRel(rel);
      const isHour = q % 4 === 0;
      const len = isHour ? Rmax * 0.032 : Rmax * 0.013;
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(a) * (R - len), cy + Math.sin(a) * (R - len));
      ctx.lineTo(cx + Math.cos(a) * (R + len * 0.35), cy + Math.sin(a) * (R + len * 0.35));
      ctx.strokeStyle = '#dcc084';
      ctx.globalAlpha = isHour ? 0.8 : 0.3;
      ctx.lineWidth = (isHour ? 3.4 : 1.7) * S;
      ctx.stroke();
      ctx.globalAlpha = 1;
      if (isHour && hh % 2 === 0) {
        const rr = R + Rmax * 0.052;
        ctx.fillStyle = 'rgba(220,192,132,.55)';
        text(`hour:${String(hh).padStart(2, '0')}`, String(hh).padStart(2, '0'), cx + Math.cos(a) * rr, cy + Math.sin(a) * rr);
      }
    }
    return R;
  }

  function arcSeg(r, laneW, sg, color, dim) {
    const st = STYLE[sg.kind];
    const clip = clipToWindow(sg.h0, sg.h1);
    if (!clip) return;
    ctx.save();
    ctx.beginPath();
    ctx.arc(cx, cy, r + st.off * laneW, angRel(clip[0]), angRel(clip[1]));
    ctx.strokeStyle = color;
    ctx.lineWidth = laneW * st.w;
    ctx.lineCap = st.dash ? 'butt' : 'round';
    if (st.dash) ctx.setLineDash(st.dash.map((x) => x * laneW));
    const glowing = sg.active && dim === 1;
    ctx.globalAlpha = st.a * (sg.past ? 0.32 : 1) * (glowing ? 1.35 : 1) * dim;
    if (glowing) {
      ctx.shadowColor = color;
      ctx.shadowBlur = laneW * 0.9;
    }
    ctx.stroke();
    ctx.restore();
  }

  function drawLanes() {
    // 六條環沿用樣稿尺寸；設定裡市場多於六個時等比縮窄，最內圈不跑進中央
    const k = Math.min(1, 6 / Math.max(1, model.markets.length));
    const rOuter = Rmax * 0.8;
    const laneW = Rmax * 0.027 * k;
    const gap = Rmax * 0.013 * k;
    model.markets.forEach((m, i) => {
      const r = rOuter - i * (laneW + gap);
      const dim = m.dimmed ? DIM : 1;
      ctx.beginPath();
      ctx.arc(cx, cy, r, angRel(-PAST_HOURS), angRel(FUTURE_HOURS));
      ctx.strokeStyle = m.color;
      ctx.globalAlpha = 0.08 * dim;
      ctx.lineWidth = laneW * 0.9;
      ctx.stroke();
      ctx.globalAlpha = 1;
      for (const sg of m.segments) arcSeg(r, laneW, sg, m.color, dim);

      /* 市場名＋狀態：置中在 2 小時缺口；過寬時縮字，不超出缺口 */
      const lx = cx + Math.cos(LABEL_ANG) * r;
      const ly = cy + Math.sin(LABEL_ANG) * r;
      const txt = `${m.name}　${m.statusText}`;
      const maxW = 2 * r * Math.sin(((GAP_HOURS / 2) / 24) * Math.PI * 2) * 0.92;
      ctx.save();
      fitFont((px) => `600 ${px}px ${CJK}`, txt, Math.round(laneW * 0.78), maxW);
      ctx.textBaseline = 'middle';
      ctx.textAlign = 'center';
      ctx.fillStyle = 'rgba(7,11,28,.7)';
      const tw = ctx.measureText(txt).width;
      ctx.fillRect(lx - tw / 2 - laneW * 0.3, ly - laneW * 0.5, tw + laneW * 0.6, laneW);
      ctx.fillStyle = m.color;
      ctx.globalAlpha = m.status === 'idle' || m.status === 'closed' ? 0.6 : 1;
      text(`market:${m.id}`, txt, lx, ly);
      ctx.restore();
    });
    return rOuter - (model.markets.length - 1) * (laneW + gap) - laneW;
  }

  /* 「現在」指標：固定不動，盤面在它底下轉 */
  function drawNowIndex(rHour, rInner) {
    const a = NOW_ANG;
    ctx.save();
    ctx.beginPath();
    ctx.moveTo(cx + Math.cos(a) * (rInner - Rmax * 0.02), cy + Math.sin(a) * (rInner - Rmax * 0.02));
    ctx.lineTo(cx + Math.cos(a) * (rHour + Rmax * 0.01), cy + Math.sin(a) * (rHour + Rmax * 0.01));
    ctx.strokeStyle = '#f4ecd8';
    ctx.globalAlpha = 0.75;
    ctx.lineWidth = 2.4 * S;
    ctx.stroke();
    const tr = rHour + Rmax * 0.1;
    ctx.globalAlpha = 0.95;
    ctx.fillStyle = '#f4ecd8';
    ctx.font = `700 ${Math.round(Rmax * 0.034)}px ${LATIN}`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    text('now', model.nowText, cx + Math.cos(a) * tr, cy + Math.sin(a) * tr);
    ctx.restore();
  }

  /* 接下來的開收盤（最多 4 筆） */
  function drawUpcoming() {
    const x = cx;
    const y = cy - Rmax * 0.16;
    const fs = Math.round(34 * S);
    const lh = fs * 1.65;
    ctx.save();
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.font = `600 ${Math.round(fs * 0.8)}px ${CJK}`;
    ctx.fillStyle = 'rgba(220,192,132,.7)';
    text('upcoming:heading', '接下來', x, y);
    ctx.font = `500 ${fs}px ${CJK}`;
    if (model.upcoming.length === 0) {
      ctx.fillStyle = 'rgba(200,210,235,.45)';
      text('upcoming:none', '24 小時內無開收盤', x, y + lh);
    }
    model.upcoming.forEach((e, i) => {
      ctx.fillStyle = e.color;
      ctx.globalAlpha = 0.92;
      text(`upcoming:${i}`, `${e.timeText}　${e.name}${e.what}`, x, y + lh * (i + 1));
    });
    ctx.restore();
  }

  function drawLegend() {
    const x = W - 40 * S;
    const y = H - 40 * S;
    const str = `粗線＝現貨正規交易　外側細線＝現貨盤前／盤後　內側虛線＝指數期貨夜盤／延長交易　·　本機時區 ${model.tzText}　·　每 15 分鐘更新`;
    ctx.save();
    fitFont((px) => `500 ${px}px ${CJK}`, str, Math.round(24 * S), W - 80 * S);
    ctx.textAlign = 'right';
    ctx.textBaseline = 'alphabetic';
    ctx.fillStyle = 'rgba(200,210,235,.45)';
    text('legend', str, x, y);
    ctx.restore();
  }

  drawBackground();
  const rHour = drawHourRing();
  const rInner = drawLanes();
  ring(rInner - Rmax * 0.04, '#3a4a72', 0.2, 1.9 * S);
  drawNowIndex(rHour, rInner);
  drawUpcoming();
  drawLegend();
  noiseOverlay(0.045);
}
