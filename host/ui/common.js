// host/ui/common.js
//
// 小工具頁面共用的日期／格式化工具（design.md D10）。內容原樣搬自
// finance-calendar.html（Lively 版，2026-09-27 對照 v6.2，該檔凍結只讀，不修改）的
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
 * 各自對不對，見 host/tests/compare/compare.mjs `buildOverrideScript` 的說明）。
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
  const base = todayBase();
  for (let i = 0; i <= 30; i++) {
    const d = addDays(base, i);
    if (isTradingDay(d, holidays)) return d;
  }
  return base; // 理論上不會走到（30 天內必有交易日）
}
