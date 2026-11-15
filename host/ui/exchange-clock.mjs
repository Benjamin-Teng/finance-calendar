// host/ui/exchange-clock.mjs
//
// 時鐘小工具的資料模型（change clock-six-exchanges，design.md D1）：六個交易所的當地時間、日期與
// 「現貨是否開盤」。純函式、不碰 DOM；開收盤判斷沿用動態桌布的 `marketState`（時區、休市日、半日市同源）。

import { marketState } from './wallpapers/lib/astrolabe-sessions.mjs';
import { tzParts } from './wallpapers/lib/core.mjs';

/**
 * 顯示順序（由東到西）、地區簡寫與時區。`id` 對應 wallpaper-config 的 `markets[].id`；這裡的 `tz` 讓
 * 設定檔還沒讀到（或缺該市場）時仍算得出時間。
 */
export const EXCHANGES = Object.freeze([
  { id: 'TYO', code: 'JP', tz: 'Asia/Tokyo' },
  { id: 'SEL', code: 'KR', tz: 'Asia/Seoul' },
  { id: 'TPE', code: 'TW', tz: 'Asia/Taipei' },
  { id: 'HKG', code: 'HK', tz: 'Asia/Hong_Kong' },
  { id: 'LON', code: 'UK', tz: 'Europe/London' },
  { id: 'NYC', code: 'US', tz: 'America/New_York' },
]);

/** 算作「開盤」的狀態：現貨正規盤與其開收盤集合競價（午休、盤前盤後、期貨夜盤、休市都不算）。 */
const OPEN_STATUSES = new Set(['regular', 'auction']);

const pad2 = (n) => String(n).padStart(2, '0');

function isOpen(config, exchange, nowMs) {
  const markets = Array.isArray(config?.markets) ? config.markets : [];
  const market = markets.find((m) => m?.id === exchange.id);
  if (!market) return false;
  try {
    // 時區以本模組為準：設定檔的 tz 打錯時，開收盤與顯示的時間仍一致。
    return OPEN_STATUSES.has(marketState({ ...market, tz: exchange.tz }, nowMs, config).status);
  } catch {
    return false;
  }
}

/**
 * 六個交易所在 `nowMs`（UTC 毫秒）的顯示資料。
 * @param {object | null} config 內建的 wallpaper-config；`null`＝還沒讀到，`open` 一律為 `false`。
 * @param {number} nowMs
 * @returns {{ code: string, time: string, date: string, open: boolean }[]}
 *   `time`＝當地 `HH:MM`（24 小時制），`date`＝當地 `YY-MM-DD`。
 */
export function exchangeRows(config, nowMs) {
  return EXCHANGES.map((exchange) => {
    const p = tzParts(exchange.tz, nowMs);
    return {
      code: exchange.code,
      time: `${pad2(p.h)}:${pad2(p.mi)}`,
      date: `${pad2(p.y % 100)}-${pad2(p.mo)}-${pad2(p.d)}`,
      open: config ? isOpen(config, exchange, nowMs) : false,
    };
  });
}
