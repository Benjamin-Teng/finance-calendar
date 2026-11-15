// host/tests/compare/scenarios.mjs
//
// 基準檔比對的情境清單（compare.mjs 用）。一個情境＝一個小工具在固定時間、固定資料下的畫面文字，
// 對應 `golden/<name>.txt` 一份基準檔。新增情境：在這裡加一筆，跑
// `node host/tests/compare/compare.mjs --scenario <name> --update` 產生基準檔，人工讀過內容再提交。
//
// 欄位：
//   name       基準檔檔名（不含副檔名），只用英數與連字號
//   widget     小工具 id（`widget.html?w=<id>`）
//   today      固定的「現在」（ISO 8601，含時區偏移）；省略＝DEFAULT_TODAY
//   fixture    tw-events fixture，相對於本目錄；省略＝host/ui/fixtures/tw-events.json
//   advanceTo  有值時頁面只在 `today` 導覽一次，之後不重新載入、把時間換成這個值並送
//              visibilitychange 再擷取（驗證「恢復後立即重算」，spec「跨日更新」）
//   settings   覆寫 fixture `settings.json` 的頂層鍵（例如 `{ show_dividend: true }`）
//   note       這個情境在驗什麼

// 週一、平日下午、非跨日邊界；同時是 host/ui/fixtures/tw-events.json 的休市日之一，
// 所以 dynamic 的預設情境順帶涵蓋「今天休市，順延下一交易日」。
export const DEFAULT_TODAY = '2026-09-28T14:05:00+08:00';

// widget.html 無條件在 `#widget-root` 底下掛 `.edit-placeholder`（編輯版面時、無內容才顯示的佔位
// 外框），擷取文字時走訪的是 clone 出來的 DOM、不看 computed style，所以要排除。
export const ROOT_SELECTOR = '#widget-root';
export const EXCLUDE_SELECTORS = ['.edit-placeholder'];

export const SCENARIOS = [
  { name: 'clock', widget: 'clock', note: '六個交易所的簡寫、時間、日期（開收盤顏色由 exchange-clock.test.mjs 驗）' },
  { name: 'macro', widget: 'macro', note: '總經日曆預設畫面' },
  { name: 'fixed', widget: 'fixed', note: '台股固定事件預設畫面（含休市順延）' },
  { name: 'dynamic', widget: 'dynamic', note: '台股動態事件；今天是休市日，處置股節順延' },
  { name: 'quotes', widget: 'quotes', note: '行情條（超寬時內容複製一份做跑馬燈）' },
  {
    name: 'macro-advance-0400',
    widget: 'macro',
    today: '2026-09-28T03:59:00+08:00',
    advanceTo: '2026-09-28T04:01:00+08:00',
    note: '不重新載入頁面，時間推進後重算',
  },
  {
    name: 'macro-advance-midnight',
    widget: 'macro',
    today: '2026-09-28T23:59:00+08:00',
    advanceTo: '2026-09-29T00:01:00+08:00',
    note: '跨日後重算',
  },
  {
    name: 'dynamic-advance-midnight',
    widget: 'dynamic',
    today: '2026-09-28T23:59:00+08:00',
    advanceTo: '2026-09-29T00:01:00+08:00',
    note: '跨日後「今天」翻日',
  },
  {
    name: 'fixed-advance-week',
    widget: 'fixed',
    today: '2026-09-26T23:59:00+08:00',
    advanceTo: '2026-09-27T00:01:00+08:00',
    note: '週六跨到週日，「本週」範圍換成下一週',
  },
  {
    name: 'dynamic-holiday-multi-skip',
    widget: 'dynamic',
    today: '2026-09-25T09:00:00+08:00',
    note: '週五休市＋週末＋週一休市，處置股節順延到下週二',
  },
  {
    name: 'dynamic-stale-3d',
    widget: 'dynamic',
    fixture: 'fixtures/tw-events-stale-3d.json',
    note: '資料 3 天未更新的頁尾警示',
  },
  {
    name: 'macro-stale-3d',
    widget: 'macro',
    fixture: 'fixtures/tw-events-stale-3d.json',
    note: '總經日曆頁尾不顯示過期警示',
  },
  {
    name: 'dynamic-dividend',
    widget: 'dynamic',
    settings: { show_dividend: true },
    note: '設定打開「顯示除權息」後的預告清單與同日排序',
  },
  {
    name: 'quotes-empty',
    widget: 'quotes',
    fixture: 'fixtures/tw-events-empty-quotes.json',
    note: '沒有行情資料時不輸出任何文字',
  },
];

export const WIDGET_IDS = [...new Set(SCENARIOS.map((s) => s.widget))];

export function getScenario(name) {
  const s = SCENARIOS.find((x) => x.name === name);
  if (!s) {
    throw new Error(`未知的情境：「${name}」（可用：${SCENARIOS.map((x) => x.name).join('、')}）`);
  }
  return s;
}
