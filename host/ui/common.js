// host/ui/common.js
//
// 小工具頁面共用的日期／格式化工具（design.md D10）。內容原樣搬自
// finance-calendar.html（Lively 版 v6.2；該檔已於 2026-11-16 自 repo 移除，本目錄註解裡的行號都對應
// `git show v6.2:finance-calendar.html`）的
// 「共用工具」區塊（約 297–312 行）與零星散落的格式化片段（週日期時區標註見 507–510
// 行），僅補上這裡的模組匯出語法；判斷邏輯逐字相同，未來各小工具（4.2–4.7）從
// finance-calendar.html 搬移對應 render 函式時，直接改呼叫這裡即可，不必重寫一份。
//
// 之後任務會再擴充本檔（休市日判斷、fmtQuotePrice 等），本 task（4.1）只先放
// clock 小工具與其餘小工具共通會用到的最小集合。

/** 一天的毫秒數。 */
export const DAY = 86400000;

/** 星期文字，索引＝Date#getDay()（0＝日）。 */
export const WD = ['日', '一', '二', '三', '四', '五', '六'];

/** 去掉時分秒，只留年月日（本機時區）。 */
export const strip = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate());

/** 加減天數（以毫秒運算，跨夏令時邊界不在台灣時區適用範圍內，沿用原邏輯）。 */
export const addDays = (d, n) => new Date(d.getTime() + n * DAY);

/** `M/D` 格式。 */
export const md = (d) => `${d.getMonth() + 1}/${d.getDate()}`;

/** `M/D (週幾)` 格式。 */
export const mdw = (d) => `${md(d)} (${WD[d.getDay()]})`;

/**
 * 本週週日（AGENTS.md／spec「本週」＝週日至週六；v6.0 由週一起算改為週日起算，
 * 全檔僅這個函式決定「本週」邊界，各小工具一致呼叫此函式即不會各自算出不同結果）。
 */
export const weekStartOf = (d) => addDays(strip(d), -d.getDay());

/** HTML escape（沿用 Lively 版，供之後搬移含使用者可見文字的小工具使用）。 */
export const esc = (s) =>
  String(s ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');

/** 補零至兩位數（時鐘 `HH:MM` 用）。 */
export const pad2 = (n) => String(n).padStart(2, '0');

// ── 外觀套用（task 5.2；design.md D4「settings 事件」；specs/widget-host-lifecycle
// 「設定持久化」Scenario「修改透明度」：所有小工具立即套用；specs/widget-host-windows
// 「小工具外觀模式」Scenario「切換為純色模式」）───────────────────────────────────────

/**
 * 依設定套用外觀相關的 CSS 自訂屬性（`--panel-o`／`--accent`，widget.css `:root`）到
 * 指定目標。各小工具在 `mount()` 掛載時與 `ctx.onSettings` 收到新設定時都呼叫本函式，
 * 使透明度／主題色改動立即反映在畫面上，不需重新載入頁面。
 *
 * 只處理型別合法、範圍合理的欄位；缺漏或不合法的值保留目前的 CSS 值（不強制回退成
 * widget.css 的 `:root` 預設）——呼叫端可能只是還沒收到完整設定，這裡採取跟 bridge.js
 * 「拒收全空 payload、不覆蓋既有好值」一致的保守態度，不能讓一筆型別錯誤的設定把畫面
 * 洗成不合理的樣子。
 *
 * **外觀模式（毛玻璃／純色）不在這裡處理**：design.md D8 探針 1.2 定案「毛玻璃恆常降級為
 * 純色」是宿主端 `desktop::apply_appearance` 在視窗建立當下決定的原生層行為（DWM 系統
 * 背景材質），與本頁的 CSS 自訂屬性無關，這裡讀 `appearance_mode` 也不會有任何視覺效果。
 *
 * @param {{ opacity?: number, accent_color?: string }|null|undefined} settings
 * @param {{ setProperty: (name: string, value: string) => void }} [target] 預設
 *   `document.documentElement.style`；測試環境（無 `document`）可傳入假物件。
 */
export function applyAppearance(settings, target = document.documentElement.style) {
  if (!settings) {
    return;
  }
  if (typeof settings.opacity === 'number' && Number.isFinite(settings.opacity)) {
    const clamped = Math.min(1, Math.max(0, settings.opacity));
    target.setProperty('--panel-o', String(clamped));
  }
  if (typeof settings.accent_color === 'string' && /^#[0-9a-fA-F]{6}$/.test(settings.accent_color)) {
    target.setProperty('--accent', settings.accent_color);
  }
}

// ── 抓取進行中的顯示與無資料文字（fetch-progress-and-manual-refresh design.md D4；
// specs/finance-widgets「抓取進行中的顯示」、specs/widget-data-feed「尚無資料」）──────────────

/** 整塊載入區塊的文字。 */
export const FETCHING_TEXT = '資料抓取中…';

/** 資料目錄還不知道時（設定尚未取得）顯示的替代文字。 */
const UNKNOWN_DATA_DIR = '（尚未取得設定）';

/**
 * 活動狀態是否該以整塊載入取代無資料提示：抓取啟用，且一輪正在進行、或還沒跑過任何一輪
 *（宿主剛啟動、首抓還沒開始的十幾秒）。`activity` 為 `null`＝未知（查詢失敗、還沒收到事件），
 * 不顯示載入（design.md D8）。
 * @param {{ running: boolean, enabled: boolean, last: string }|null} activity
 */
export function isFetchPending(activity) {
  return !!activity && activity.enabled && (activity.running || activity.last === 'none');
}

/**
 * 通道 `tw-events` 沒有有效快照、且不在整塊載入時的提示文字。依抓取狀態區分：抓取已關閉、最近一輪
 * 失敗、其餘（成功過但目前資料目錄沒有資料，或狀態未知）。只有最近一輪真的失敗才提到失敗——規格
 * MUST NOT 在抓取尚未失敗時宣稱連線失敗。
 * @param {{ running: boolean, enabled: boolean, last: string }|null} activity
 * @param {string|null|undefined} dataDir 設定的 `data_dir`。
 */
export function noDataText(activity, dataDir) {
  const dir = dataDir ? String(dataDir) : UNKNOWN_DATA_DIR;
  if (activity && !activity.enabled) {
    return `尚無資料 —— 資料抓取已關閉；資料目錄：${dir}`;
  }
  if (activity && activity.last === 'failed') {
    return '尚無資料 —— 最近一次資料抓取失敗，可從系統匣「產生 log.txt」查看原因';
  }
  return `尚無資料 —— 資料目錄：${dir}`;
}

/**
 * 由（快照狀態、`needsData`、活動狀態）算出小工具該顯示什麼：
 *   - `content`：有有效快照，或小工具不需要抓取資料（時鐘、台股固定事件）——照常顯示內容。
 *   - `loading`：整塊載入（`text`＝「資料抓取中…」）。
 *   - `empty`：無資料提示（`text`＝`noDataText`）。
 * 純函式；外殼（widget.html）用它決定要不要整塊載入，各小工具模組用它取無資料文字。
 * @param {{ status: string|null|undefined, needsData: boolean,
 *   activity: { running: boolean, enabled: boolean, last: string }|null,
 *   dataDir?: string|null }} input
 * @returns {{ mode: 'content'|'loading'|'empty', text: string }}
 */
export function fetchActivityView({ status, needsData, activity, dataDir }) {
  if (status === 'ok' || !needsData) {
    return { mode: 'content', text: '' };
  }
  if (isFetchPending(activity)) {
    return { mode: 'loading', text: FETCHING_TEXT };
  }
  return { mode: 'empty', text: noDataText(activity, dataDir) };
}

/**
 * `UTC+N` 格式的本機時區標籤（沿用 finance-calendar.html 507–508 行的算法）。正時區不含正號以外
 * 的额外處理、負時區沿用 `Number` 原生的負號輸出。
 */
export function tzOffsetLabel() {
  const off = -new Date().getTimezoneOffset() / 60;
  return (off >= 0 ? '+' : '') + off;
}

/**
 * 每月第 n 個星期 wd（wd: 0=日…6=六）。台股固定事件計算用（4.4 搬移 `fixedOccurrences`
 * 時會用到），先放在共用工具供各小工具一致呼叫。
 */
export function nthWeekday(y, m, wd, n) {
  const first = new Date(y, m, 1);
  return new Date(y, m, 1 + ((wd - first.getDay() + 7) % 7) + 7 * (n - 1));
}

/**
 * `YYYY-MM-DD`（本機時區，無時區轉換；沿用 finance-calendar.html 586 行 `isoDate`，
 * 供台股動態事件的休市日／處置股期間比對使用，task 4.4）。
 */
export function isoDate(d) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/**
 * 台股「今天」（沿用 finance-calendar.html 591 行 `todayBase`）：一般情況跟隨系統
 * （觀看者）日期；`window.__TEST_TODAY`（'YYYY-MM-DD'）可覆寫，供 design.md D11 對照測試
 * 使用。**只影響台股動態事件**（`dynamic.js` 呼叫 `targetDay`／本函式的地方）——時鐘與台股
 * 固定事件維持零參數 `new Date()`，這個分工是 Lively 版原本就有的行為，原樣保留
 * （preflight.md 已記錄此分工，不可合併成同一種 Date 覆寫方式，否則會測不出兩條路徑
 * 各自對不對，見 host/tests/compare/capture-utils.mjs `buildOverrideScript` 的說明）。
 */
export function todayBase() {
  return strip(window.__TEST_TODAY ? new Date(`${window.__TEST_TODAY}T00:00:00`) : new Date());
}

/**
 * 交易日判斷：非週末且不在休市日集合內（沿用 finance-calendar.html 593 行
 * `isTradingDay`，task 4.4）。
 * @param {Date} d
 * @param {Set<string>} holidays `isoDate` 格式的休市日集合。
 */
export function isTradingDay(d, holidays) {
  const wd = d.getDay();
  return wd !== 0 && wd !== 6 && !holidays.has(isoDate(d));
}

/**
 * 目標交易日：今天是交易日則為今天，否則往後找下一個交易日（上限 30 天，理論上不會走到；
 * 沿用 finance-calendar.html 598 行 `targetDay`，task 4.4）。台股動態事件的處置股節維持
 * 「當日制」：非交易日（休市／週末）時順延到下一個交易日。
 * @param {Set<string>} holidays `isoDate` 格式的休市日集合。
 */
export function targetDay(holidays) {
  return nextTradingDay(todayBase(), holidays);
}

/**
 * `d` 是交易日就回傳 `d`，否則往後找第一個交易日（上限 30 天，找不到回傳 `d`，理論上不會
 * 走到）。台股動態事件的處置股順延（`targetDay`）與台股固定事件的休市順延共用（change
 * fixed-events-holiday-shift design.md D2）。
 * @param {Date} d 已 `strip` 的日期。
 * @param {Set<string>} holidays `isoDate` 格式的休市日集合。
 */
export function nextTradingDay(d, holidays) {
  for (let i = 0; i <= 30; i++) {
    const c = addDays(d, i);
    if (isTradingDay(c, holidays)) return c;
  }
  return d;
}

/** 順延徽章的說明（change fixed-events-holiday-shift design.md D1／D3）。 */
const SHIFT_NOTE_TAIFEX = '依期交所規則順延至次一交易日';
const SHIFT_NOTE_PRESUMED = '推定順延，以金管會公告為準';

/**
 * 台股固定事件（原搬自 finance-calendar.html 515–543 行 `fixedOccurrences`；change
 * fixed-events-holiday-shift design.md D1／D2 起加上休市順延）：從 `today` 的上個月起算
 * `monthsAhead` 個月（月底的財報截止可能順延進本月），依順延後日期排序。
 *
 * 每筆 `{ date, orig, tag, label, note }`：`date`＝顯示日期；`orig`＝依日曆規則的原日期；
 * `note`＝順延說明，未順延時為 `null`。台指期結算、財報截止、月營收截止遇非交易日順延；
 * 季結算（3/6/9/12 月第三個週五，美股指數期貨）不依台灣休市日調整。
 * @param {Date} today 已 `strip` 的今天。
 * @param {Set<string>} holidays `isoDate` 格式的休市日集合（缺年度時等同只看週末）。
 * @param {number} monthsAhead 本月之後再算幾個月。
 */
export function fixedOccurrences(today, holidays, monthsAhead) {
  const out = [];
  const push = (orig, tag, label, shiftNote) => {
    const date = shiftNote ? nextTradingDay(orig, holidays) : orig;
    const shifted = date.getTime() !== orig.getTime();
    out.push({ date, orig, tag, label, note: shifted ? shiftNote : null });
  };
  for (let k = -1; k <= monthsAhead; k++) {
    const y = today.getFullYear();
    const m = today.getMonth() + k;
    const yy = new Date(y, m, 1).getFullYear();
    const mm = new Date(y, m, 1).getMonth();

    // 台指期／選擇權結算：每月第三個週三。
    push(nthWeekday(yy, mm, 3, 3), '期權', '台指期／選擇權結算', SHIFT_NOTE_TAIFEX);

    // 那斯達克／道瓊期指季度結算：3/6/9/12 月第三個週五（台股季月結算在第三個週三，已含在上一筆）。
    if ([2, 5, 8, 11].includes(mm)) {
      push(nthWeekday(yy, mm, 5, 3), '季結算', '那指・道瓊期貨季度結算', null);
    }

    // 月營收公布截止：每月 10 日（公布上一個月）。
    const pm = new Date(yy, mm, 0).getMonth() + 1;
    push(new Date(yy, mm, 10), '營收', `${pm}月營收公布截止（10日前）`, SHIFT_NOTE_PRESUMED);

    // 財報公布截止。
    const fin = { 2: [31, 'Q4＋年報'], 4: [15, 'Q1'], 7: [14, 'Q2'], 10: [14, 'Q3'] }[mm];
    if (fin) {
      push(new Date(yy, mm, fin[0]), '財報', `${fin[1]} 財報公布截止`, SHIFT_NOTE_PRESUMED);
    }
  }
  return out.sort((a, b) => a.date - b.date);
}
