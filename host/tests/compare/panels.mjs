// host/tests/compare/panels.mjs
//
// Lively 版面板 id → 新版小工具 id 的對照表（design.md D11／task-4.2-brief.md）。
// 兩邊 DOM 結構、選取器、排除清單都集中在這裡，4.3–4.5 搬移對應小工具時不需要改
// compare.mjs 本體，只要這裡的條目仍然對得上（新版尚未實作的 widget 用 `/new/widget.html?w=<id>`
// 打開時走 widget.html 的「小工具尚未實作」佔位訊息，屬正常現象，不是本檔要處理的差異）。
//
// 選取器來源（finance-calendar.html，2026-09-27 對照 v6.2，該檔凍結只讀，見
// .superpowers/sdd/tasks/preflight.md 第 0 節的行號速查）：
//   - clockCard（253–258 行）／panelMacro（259–266）／panelFixed（271–277）／
//     panelDyn（279–286）／panelQuotes（290–292）。

// task 7.3：widget.html 無條件在 `#widget-root` 底下多掛一個 `.edit-placeholder`
// 節點（編輯版面時、小工具無內容才顯示的佔位外框，design.md D7），其文字內容（小工具名稱）
// 是新版特有、Lively 版沒有對應節點，故每個小工具都要排除，否則會被
// `capture-utils.mjs` 的文字節點走訪誤判成多出一行差異（walker 直接走訪 clone 出來的
// DOM，不看 computed style，`display:none` 不影響它被收進 `lines`）。
const EDIT_PLACEHOLDER_EXCLUDE = '.edit-placeholder';

export const PANELS = {
  clock: {
    livelySelector: '#clockCard',
    // 新版 widget.html 一律把小工具內容掛在 #widget-root 底下（host/ui/widget.html）。
    newSelector: '#widget-root',
    exclude: [
      // Lively 版時鐘卡右下角疊了版本字串（VERSION，finance-calendar.html 頂部
      // `const VERSION`），純粹是「桌布有沒有套用成功」的除錯標記，不是時鐘小工具的內容
      // ——task 4.1 的 widgets/clock.js 就沒有搬這個節點（見 task-4.1-report.md）。
      // 兩邊本來就不該相同，故排除，不算差異。
      '#verTag',
      EDIT_PLACEHOLDER_EXCLUDE,
    ],
  },
  macro: {
    livelySelector: '#panelMacro',
    newSelector: '#widget-root',
    exclude: [
      // ▴▾ 捲動按鈕（setupScrollButtons() 動態插入 `.scrollctl.up`／`.scrollctl.dn`，
      // finance-calendar.html 418–453 行）：design.md D10「▴▾ 按鈕改原生捲動」，新版
      // 本來就不會有這兩顆按鈕，屬 D11 明列的刻意差異，排除。
      '.scrollctl',
      EDIT_PLACEHOLDER_EXCLUDE,
    ],
  },
  fixed: {
    livelySelector: '#panelFixed',
    newSelector: '#widget-root',
    exclude: ['.scrollctl', EDIT_PLACEHOLDER_EXCLUDE],
  },
  dynamic: {
    livelySelector: '#panelDyn',
    newSelector: '#widget-root',
    exclude: [
      '.scrollctl',
      EDIT_PLACEHOLDER_EXCLUDE,
      // 除權息開關差異（D11 明列的刻意差異）：CONFIG.dynTypes 預設不含 'dividend'，
      // 兩邊預設設定相同時本來就不會出現除權息項目，這裡不需要額外的選取器排除；
      // 記錄在這裡是給之後改設定比對時的提醒，見 README「已知限制」。
    ],
  },
  quotes: {
    livelySelector: '#panelQuotes',
    newSelector: '#widget-root',
    exclude: [EDIT_PLACEHOLDER_EXCLUDE],
  },
};

export const PANEL_IDS = Object.keys(PANELS);

export function getPanel(id) {
  const p = PANELS[id];
  if (!p) {
    throw new Error(`未知的面板／小工具 id：「${id}」（可用：${PANEL_IDS.join('、')}）`);
  }
  return p;
}
