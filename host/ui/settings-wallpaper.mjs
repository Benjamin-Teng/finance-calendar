// host/ui/settings-wallpaper.mjs
//
// 設定視窗「動態桌布」區塊的純邏輯（dynamic-wallpaper task 4.8）。ES module、不依賴 DOM，可在
// Node 測試（host/tests/settings-wallpaper.test.mjs）；畫面與 IPC 在 settings.html。
//
// - 主題選單（spec desktop-wallpaper「桌布主題選擇」）：「不接管」與五套主題，id 與
//   host/src/settings.rs `WallpaperTheme` 的字串一致。
// - 休市表快過期（spec wallpaper-themes「休市表快過期」）：以繪圖頁同一套規則判定——先
//   `mergeThemeConfig(內建預設, 宿主送來的設定)` 逐層合併（宿主 Rust 只做頂層合併），再
//   `holidayExpiryReminder`（host/ui/wallpapers/lib/config-holidays.mjs：本機日期距 12/31 不足
//   30 天，且任一市場最晚的 through 早於明年 12/31）。不在 Rust 另寫一份，避免兩套規則分岔
//   （例如使用者檔只寫了部分市場時，頂層合併會讓其他市場「消失」）。
// - 狀態提示：協調迴圈狀態快照（host/src/wallpaper_settings.rs `WallpaperStatusView`）轉成要顯示
//   的提示（等待資料中等）。

import { holidayExpiryReminder, mergeThemeConfig } from './wallpapers/lib/config-holidays.mjs';

/** 主題選單（順序照 spec：不接管、脊線、撕日曆、星盤、等高線、天際線）。 */
export const THEME_OPTIONS = [
  { id: 'none', label: '不接管', note: '不改變桌布' },
  { id: 'ridgeline', label: '脊線', note: '每日 06:00 更新' },
  { id: 'tearoff', label: '撕日曆', note: '換日與融資更新時' },
  { id: 'astrolabe', label: '星盤', note: '每 15 分鐘更新' },
  { id: 'contour', label: '等高線', note: '每日 06:00 更新' },
  { id: 'skyline', label: '天際線', note: '日 K 有新交易日時' },
];

/** Windows 備份提示（spec「提示 Windows 備份會帶走桌布」；常駐在主題選單處）。 */
export const BACKUP_HINT =
  'Windows 備份的「記住我的喜好設定」會備份桌布，接管期間產生的圖可能在新裝置設定時被還原；' +
  '若不希望如此，可到「設定 > 帳戶 > Windows 備份 > 記住我的喜好設定」關閉個人化備份。';

/** 原桌布為 Windows 焦點時的提醒（spec「原桌布為 Windows 焦點時先提醒」原文）。 */
export const SPOTLIGHT_WARNING = '停止接管時無法自動切回 Windows 焦點，需要到 Windows 設定手動切回';

const pad2 = (n) => String(n).padStart(2, '0');

/** 本機日期 'YYYY-MM-DD'。 */
export function localDateString(date) {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

/**
 * 休市表快過期判定。`defaults`＝內建預設（wallpaper-config.default.json），`config`＝宿主送來的設定
 * （Rust 頂層合併後；`null`／非物件時視為空設定）。回傳 `null`（不用提示）或
 * `{ year, markets: [{ id, name, through }] }`（`year`＝缺的年度；`through`＝該市場休市表最晚涵蓋到的
 * 日期，沒有任何年度時為 `null`——task 6.4：下一年度只有部分月份時提示要寫「只到 …」，不是「沒有資料」）。
 */
export function holidayReminder(defaults, config, todayLocal) {
  const plain = config !== null && typeof config === 'object' && !Array.isArray(config) ? config : {};
  const { value } = mergeThemeConfig(defaults, plain);
  const ids = holidayExpiryReminder(value, todayLocal);
  if (ids.length === 0) return null;
  const names = new Map(
    (Array.isArray(value.markets) ? value.markets : [])
      .filter((m) => m && typeof m.id === 'string')
      .map((m) => [m.id, typeof m.name === 'string' ? m.name : m.id]),
  );
  const isObj = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);
  const holidays = isObj(value.holidays) ? value.holidays : {};
  const latestThrough = (id) =>
    Object.values(isObj(holidays[id]) ? holidays[id] : {})
      .map((y) => (isObj(y) && typeof y.through === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(y.through) ? y.through : null))
      .filter(Boolean)
      .sort()
      .at(-1) ?? null;
  return {
    year: Number(todayLocal.slice(0, 4)) + 1,
    markets: ids.map((id) => ({ id, name: names.get(id) ?? id, through: latestThrough(id) })),
  };
}

/**
 * 休市表快過期的提示文字（spec「請更新明年的休市表」，附市場與設定檔路徑）。每個市場依實際涵蓋範圍
 * 描述：下一年度已有部分月份＝「只到 YYYY-MM-DD」，完全沒有＝「沒有 YYYY 年的資料」（task 6.4）。
 */
export function holidayNoticeText(reminder, configPath) {
  const firstDay = `${reminder.year}-01-01`;
  const markets = reminder.markets
    .map((m) => {
      const label = m.name === m.id ? m.id : `${m.name}（${m.id}）`;
      return m.through && m.through >= firstDay
        ? `${label}只到 ${m.through}`
        : `${label}沒有 ${reminder.year} 年的資料`;
    })
    .join('、');
  const where = configPath ? `設定檔：${configPath}` : '';
  return `請更新明年的休市表：${markets}，還沒有涵蓋到 ${reminder.year} 年底，桌布會把未知日期當成一般交易日。${where}`;
}

/**
 * 協調迴圈狀態快照 → 要顯示的提示 `[{ kind: 'info'|'warn'|'err', text }]`。協調迴圈沒有在跑時不顯示。
 * 焦點確認不在這裡（另以對話處理）。
 */
export function statusNotices(status) {
  if (!status || !status.coordinator_running) return [];
  switch (status.state) {
    case 'waiting_for_data':
      return [
        {
          kind: 'info',
          text: `等待資料中：所選主題需要的資料（${status.waiting_for ?? '市場資料'}）還沒出現，桌布維持原狀；資料出現後會自動開始接管桌布。`,
        },
      ];
    case 'hold_without_data':
      return [
        {
          kind: 'warn',
          text: `目前讀不到${status.holding_without ?? '市場資料'}：保留目前的桌布，資料恢復後繼續更新。`,
        },
      ];
    case 'blocked':
      return [
        {
          kind: 'err',
          text: `桌布狀態檔無法使用（${status.blocked ?? '未知原因'}）：不接管也不還原桌布，詳見記錄檔。`,
        },
      ];
    case 'resuming_restore':
      return [{ kind: 'info', text: '正在還原上次沒有還原完成的桌布，完成前不接管。' }];
    case 'waiting_for_state_file':
      return [{ kind: 'warn', text: '暫時讀不到桌布狀態檔，稍後自動重試；期間不接管。' }];
    default:
      return [];
  }
}
