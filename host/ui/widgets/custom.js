// host/ui/widgets/custom.js
//
// 擴充插槽小工具（design.md D6，task 4.6）。custom1–custom5 五個插槽共用這一份實作：
// registry.js 讓每個插槽的 `channel` 與自己的 id 同名（host/src/widgets.rs
// `custom_spec`／host/src/data.rs `CUSTOM_CHANNELS`），本模組只依 `ctx.config.id`／
// `ctx.config.channel` 顯示內容，不寫死任何一個
// 插槽的 id。`widgets/customN.js`（N=1..5）是薄包裝，`export { mount } from './custom.js'`，
// 讓 widget.html 的動態 `import('./widgets/${id}.js')`（依 id 找檔名，design.md D10）能找到
// 五個各自的模組檔，內容邏輯集中在本檔一份——這是 task-4.6-brief.md「五個插槽可共用一個實作檔
// ＋薄包裝，或 registry 指向同一模組並以 id 區分——選最簡單的」兩個選項裡較簡單的一個：後者要
// 讓 registry.js 每個 custom 項目多帶一個「模組檔名」欄位、還要改 widget.html 的通用載入邏輯
// 去讀那個欄位（widget.html 是十個小工具共用的核心骨架，design.md D10 明訂「依 registry.js
// 載入 widgets/<id>.js」，id 就是檔名，插入一層間接會讓這條規則多一個例外）；五個一行的薄包裝
// 檔完全不改動任何共用邏輯，純粹是轉發，符合 AGENTS.md 鐵則 8「KISS」。日後要幫某個插槽接上
// 專屬資料源時（finance-widgets spec「擴充插槽小工具」：「替換為專屬顯示模組後，SHALL 不需
// 修改宿主核心、視窗管理或設定結構」），只需要把對應的 customN.js 內容換成真正的實作，其餘四個
// 與本檔都不受影響。
//
// ## 顯示規則的分工（design.md D6；widget-data-feed spec「拒收無效資料並保留上一份好資料」／
// 「尚無資料」；finance-widgets spec「擴充插槽小工具」）
// 「無資料時顯示尚未設定」「刪檔後仍保留上一份」「有資料時顯示更新時間與內容摘要」這三條規則，
// 前兩條完全由核心決定——本檔只負責「拿到什麼 snapshot 就照樣畫」，不自己做保留/拒收判斷：
//   - `JsonFileSource`（host/src/data.rs）刪檔後找不到來源檔時保留上一份快照、不會把 status
//     改回 `"empty"`，故「刪除 customN.json 後小工具仍顯示上一份摘要」不需要本檔特別處理，
//     刪檔不會產生「變成空」的推播——本檔收到的最後一筆 `data` 事件／`get_snapshot` 結果就是
//     那份舊資料。唯一會在曾有資料之後送出 empty 的情況是使用者改了 `data_dir`：核心重建註冊表
//     後，對已訂閱、新世代尚無快照的通道推送一筆新世代的 empty（host/src/widgets.rs
//     `push_empty_snapshots_after_rebuild`），代表「新資料目錄尚無此檔」，照樣顯示「尚未設定」。
//   - `snapshot.status !== 'ok'`（`get_snapshot`／`data` 事件正規化後的「empty」狀態：通道在
//     目前 registry 世代尚未取得有效快照，即 widget-data-feed spec「尚無資料」）時顯示「尚未設定」。
//   - 有資料時顯示的「更新時間」用 `snapshot.meta.loadedAt`（宿主端 `now_epoch_ms()` 記錄的
//     推播/查詢時刻，host/src/data.rs `SnapshotMeta`），不是資料檔本身的 `updated`／`fetched`
//     ——customN.json 是任意合法 JSON（design.md D5），不保證存在這兩個欄位；`tw_events.json`
//     才有、且由 macro.js／dynamic.js 各自的頁腳處理，不屬於本檔的職責。

/** 內容摘要：陣列顯示筆數；物件列出頂層鍵，陣列值的鍵附上該陣列的筆數（design.md D6「內容
 * 摘要（頂層鍵與筆數）」）；其餘型別（字串／數字／布林／null）直接顯示其 JSON 表示，容錯
 * 「合法 JSON 但頂層不是物件或陣列」這種 widget-data-feed spec 允許的情況（validator 只要求
 * 為合法 JSON，design.md D5）。 */
function summarize(data) {
  if (Array.isArray(data)) {
    return `陣列，共 ${data.length} 筆`;
  }
  if (data && typeof data === 'object') {
    const keys = Object.keys(data);
    if (!keys.length) {
      return '空物件（無頂層鍵）';
    }
    const parts = keys.map((k) => (Array.isArray(data[k]) ? `${k}（${data[k].length} 筆）` : k));
    return `${keys.length} 個頂層鍵：${parts.join('、')}`;
  }
  return JSON.stringify(data);
}

/** `snapshot.meta.loadedAt`（epoch ms）→ 本機時區的可讀時間；理論上有資料時一定會有這個欄位
 * （`get_snapshot`／`data` 事件皆附，見 host/src/widgets.rs `SnapshotResponse`），型別不符時
 * 防禦性顯示 `—`，不讓整個小工具因為意外形狀而掛掉。 */
function fmtLoadedAt(meta) {
  if (!meta || typeof meta.loadedAt !== 'number') {
    return '—';
  }
  return new Date(meta.loadedAt).toLocaleString('zh-TW', { hour12: false });
}

// widget-adaptive-zoom-and-grid task 4.1：容器不再寫死 CSS 寬度（舊版 `WIDTH_PX = 360`）。
// 倍率改由 `content_zoom`（最小框＋舒適框＋字級）決定，CSS viewport 寬不再固定等於某個設計
// 寬度，寫死寬度會讓內容偏離視窗；寬度由 `#widget-root` 決定、填滿小工具（與 macro／fixed／
// dynamic／quotes 一致）。高度（原 `maxHeight`）task 7.3 起已填滿視窗（widget.css
// `#widget-root > .panel{flex:1 1 auto}`）。

/**
 * @param {HTMLElement} container widget.html 建立的內容容器。
 * @param {{ common: typeof import('../common.js'), config: object, snapshot: object,
 *   settings: object, onData: (fn: (snap: object) => void) => void,
 *   onSettings: (fn: (settings: object) => void) => void }} ctx widget.html 組好的執行環境。
 *   `config` 是 registry.js 裡「這一個插槽自己」的項目（含 `id`／`channel`），五個插槽
 *   各自呼叫本檔時拿到的 `config.id` 不同，這是本檔不必寫死 id 就能
 *   分辨「自己是哪一個插槽」的依據（標題文字用得到）。
 * @returns {() => void} 卸載函式（本模組沒有計時器／額外事件監聽器，回傳 no-op 供未來擴充，
 *   與 clock.js 的說明一致：widget.html 目前不主動呼叫卸載函式）。
 */
export function mount(container, ctx) {
  const { common, config } = ctx;

  // task 5.2：透明度／主題色即時套用（design.md D4「settings 事件」；
  // specs/widget-host-lifecycle「設定持久化」Scenario「修改透明度」）。
  common.applyAppearance(ctx.settings);
  ctx.onSettings((settings) => common.applyAppearance(settings));

  container.classList.add('panel');

  const header = document.createElement('header');
  const ttl = document.createElement('div');
  ttl.className = 'ttl';
  ttl.textContent = `擴充插槽 ${config.id}`;
  const sub = document.createElement('div');
  sub.className = 'sub';
  header.append(ttl, sub);

  const body = document.createElement('div');
  body.className = 'scroll-area';
  body.style.padding = '14px 18px';

  container.append(header, body);

  function render(snapshot) {
    // fix round 1（Codex review .superpowers/sdd/tasks/reviews/task-4.6-codex.md [medium]）：
    // 「有沒有資料」只看 `snapshot.status`（widget-data-feed spec「尚無資料」：某通道從未取得
    // 有效快照才是「尚無資料」）。之前這裡多加了 `snapshot.data == null` 一個條件，把「已經
    // 成功載入、但檔案內容合法地就是 JSON `null`」的快照也誤判成「尚未設定」——
    // customN.json 允許任意合法 JSON（widget-data-feed spec「擴充通道讀取任意 JSON」），
    // `null` 是合法值，此時 `status` 是 `"ok"`，不該被這條額外檢查攔截。status 為 `"ok"`
    // 時一律進入下面的 summarize()／loadedAt 顯示路徑，`summarize(null)` 落到最後一個分支
    // （非陣列非物件）回傳 `JSON.stringify(null)`＝字串 `"null"`，如實呈現內容，不再清空
    // 更新時間。
    if (!snapshot || snapshot.status !== 'ok') {
      sub.textContent = '';
      body.innerHTML = '<div class="empty">尚未設定</div>';
      return;
    }
    sub.textContent = `更新於 ${fmtLoadedAt(snapshot.meta)}`;
    body.innerHTML = `<div class="summary">${common.esc(summarize(snapshot.data))}</div>`;
  }

  render(ctx.snapshot);
  ctx.onData((snap) => render(snap));

  return () => {};
}
