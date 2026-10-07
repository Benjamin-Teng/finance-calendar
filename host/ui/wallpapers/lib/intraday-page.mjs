// host/ui/wallpapers/lib/intraday-page.mjs
//
// 脊線與等高線（task 3.5）兩頁共用的頁面層：讀設定與資料、算過期標示、畫標示文字並登記邊界框。
// 純邏輯在 intraday.mjs（盤中走勢）與 tw-trading-days.mjs（交易日、落後數）；本檔只組裝與寫字。
//
// - 缺資料（twii_intraday 不存在、序列為空…）由 readIntraday 拋錯 → runWallpaper 走 phase 'error'，
//   宿主內呼叫 wallpaper_render_failed（3.1 契約）；宿主保留舊圖。頁面不畫假資料。
// - 交易日判定用合併內建預設後的設定（config.holidays.TPE）＋ data.holidays ＋週末。
// - 標示文字：交易日（IBM Plex Mono 600）與「資料停在 M/D」（Noto Sans TC 500），兩頁字型相同。

import { FONT_REQUIREMENTS, textBox } from './core.mjs';
import { loadDefaultConfig } from './default-config.mjs';
import { dayCaption, formatPrice, normalizeIntraday, readIntraday, tradeDateSeed } from './intraday.mjs';
import { staleMarker } from './tw-trading-days.mjs';

/** 兩頁實際用到的字型（`runWallpaper({ fonts })`）：交易日與標高用 Plex Mono 600、過期標示用 Noto Sans TC 500。 */
export const INTRADAY_FONT_IDS = ['plexmono-600', 'notosans-500'];
export const INTRADAY_FONTS = FONT_REQUIREMENTS.filter((r) => INTRADAY_FONT_IDS.includes(r.id));

export const MONO = '"IBM Plex Mono"';
export const SANS = '"Noto Sans TC"';

/**
 * 讀設定（內建預設＋通道 config）與盤中走勢，算正規化序列、過期標示、裝飾層 seed、交易日標示。
 * @param {object} env runWallpaper 傳入的環境
 * @param {{ defaults?: object }} [opts] `defaults` 省略時讀內建預設檔
 */
export async function prepareIntraday(env, opts = {}) {
  const defaults = opts.defaults ?? (await loadDefaultConfig());
  const cfg = env.withDefaults(defaults);
  const series = readIntraday(env.data); // 缺資料 → 拋錯（整張失敗）
  for (const w of series.warnings) env.warn(w);
  const norm = normalizeIntraday(series.points);
  const stale = staleMarker(series.date, env.nowMs, cfg, env.data);
  return {
    cfg,
    series,
    norm,
    stale,
    seed: tradeDateSeed(series.date),
    caption: dayCaption(series.date, series.points),
  };
}

/**
 * 寫一段字並登記邊界框（`textBox` 量實際墨跡）。`o`：{ label, text, x, y, font, color, align, baseline, halo }。
 * `halo`＝與底色同色的柔光（{ color, blur }），線條穿過文字時保持可讀；不影響邊界框。
 * 回傳登記的框。
 */
export function drawLabel(env, o) {
  const { ctx, boxes } = env;
  ctx.save();
  ctx.font = o.font;
  ctx.fillStyle = o.color;
  ctx.textAlign = o.align ?? 'left';
  ctx.textBaseline = o.baseline ?? 'alphabetic';
  if (o.halo) {
    ctx.shadowColor = o.halo.color;
    ctx.shadowBlur = o.halo.blur;
    ctx.fillText(o.text, o.x, o.y); // 先畫一次帶柔光，再畫一次清晰的字
    ctx.shadowBlur = 0;
  }
  ctx.fillText(o.text, o.x, o.y);
  const box = textBox(ctx, o.label, o.text, o.x, o.y);
  boxes.push(box);
  ctx.restore();
  return box;
}

/** 一行摘要（截圖稿記進證據檔）。 */
export function summarizeIntraday(m, extra = '') {
  const s = m.stale;
  return (
    `${m.caption}｜${m.series.points.length} 點｜高 ${formatPrice(m.norm.hi)} 低 ${formatPrice(m.norm.lo)}｜` +
    `${m.norm.flat ? '平盤（以最小範圍置中）' : '依高低點正規化'}｜seed ${m.seed}｜` +
    `最近已收盤 ${s.lastClosed}，落後 ${s.behind} 個交易日｜過期標示：${s.text ?? '不顯示'}` +
    (extra ? `｜${extra}` : '')
  );
}
