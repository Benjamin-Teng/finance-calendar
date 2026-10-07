// host/ui/widgets/macro.js
//
// 總經日曆小工具（design.md D6／D10，task 4.3）。原樣搬自 finance-calendar.html
// （Lively 版，2026-09-27 對照 v6.2；456–512 行 `renderMacro`；該檔凍結只讀，不修改）：
// 只改「資料從哪來」（Lively 版靠全域 `data` 參數，這裡改吃 `ctx.snapshot`／`ctx.onData`
// 推送的通道快照）與「DOM 容器」（Lively 版寫死 `$('macroList')` 等全域 id，這裡改成
// `mount()` 自己建立的區域變數），事件分組、時間換算、預測值／前值格式、頁腳文字這些顯示
// 判斷邏輯逐字相同，不做任何「順手改善」。
//
// ## 頁腳歸屬判斷（task-4.3-brief.md 要求說明）
// Lively 版原始碼有兩個獨立頁腳：`#macroFoot`（本檔搬移對象，只有「資料更新：{updated}」＋
// 「本機時區」＋「預=預測值 前=前值」，見 456–512 行）與 `#dynFoot`（`renderDyn`，
// finance-calendar.html 678–687 行，屬於「台股動態事件」小工具、task 4.4 範圍）。
// **「已 N 天未更新」過期警示只存在於 `#dynFoot`**（678–683 行用 `data.fetched` 算
// `age`），`#macroFoot` 全程只用 `data.updated`、完全沒有過期判斷（逐行核對過 456–512 行，
// 沒有任何 `fetched`／`age`／「未更新」字樣）。因此本檔（macro）**不**搬移過期警示邏輯——
// 搬了就是「順手改善」而非原樣搬移，會讓對照測試在 Lively 版本來就不顯示過期警示的地方
// 顯示出新內容，變成假差異。task-4.3-brief.md 的 pre-flight 裁決要求「以 fetched 為 3 天前
// 的 fixture 驗證」這件事，本檔用 Lively 版自比對（`--panel macro` 與 `--panel dynamic`
// 對照）驗證了「過期警示只出現在 dynamic、不出現在 macro」這個判斷本身沒有搬錯位置，證據見
// task-4.3-report.md；「dynamic 小工具要正確搬移這段邏輯」留給 task 4.4 執行者。
//
// ## 跨日／視窗被遮蔽後重算（spec「跨日更新」；design.md Risks 最後一條）
// Lively 版靠 `tickClock`（時鐘小工具兼任的心跳）偵測跨日或大幅時間跳躍，觸發
// `refreshAll()` → `loadData()` → `renderData()` → `renderMacro()`；design.md D12 明說這個
// 心跳凍結偵測隨 `loadData` 防重入一併移除，由核心的資料推送（`data` 事件）與各小工具自己
// 的重算取代（不是「移除跨日重算本身」，是移除「集中心跳」這個機制）。新架構下每個小工具是
// 獨立視窗、沒有其他小工具可以幫忙戳一下，因此本檔改成：①週期性重算（60 秒一次，沿用 Lively
// `CONFIG.freshCheckMin=1` 分鐘的量級）；②`visibilitychange` 立即重算（design.md Risks
// 「被完全遮住的 WebView2 可能節流計時器 → 解除遮蔽時頁面立即重算」——60 秒的週期計時器在
// 視窗被完全遮住時可能被節流延後，解除遮蔽的當下立即補一次，不等下一個週期）。
//
// **task 4.3 fix round 1（Codex finding，2026-09-28）**：這兩個觸發點原本直接呼叫完整的
// `render(lastData)`，而 `render()` 開頭 `list.innerHTML = ''` 清空整份清單、結尾又把
// `scrollTop` 拉回第一筆未發生事件——使用者只是在往下看後面的事件，每分鐘（或視窗一恢復
// 可見）就被強制拉回頂部。Lively 版沒有這個退化：`loadData(true)`（`freshCheckMin` 週期）
// 在 `data.updated` 沒變時直接跳過整段重繪，只有「資料真的變了」或「日期真的翻了」
// （`refreshAll()`）才會重繪＋跳頁——但即使是 Lively 版的 `refreshAll()`，也只在「今天」
// 真的變動時才觸發，不會每分鐘無條件發生。
//
// 修法：週期計時器與 `visibilitychange` 改呼叫 `updateTimeSensitiveState()`（見下方），
// 只原地更新「今天」標記（`.dayhead.today`／「　今天」字樣）與「已過去」灰階
// （`.ev.past`），不清空清單、不重建 DOM、不動 `scrollTop`——使用者的閱讀位置不受影響。
// 真正的「資料變了」（`ctx.onData` 推送新快照）才呼叫完整的 `render()`（含跳到第一筆未
// 發生事件），這與 Lively 版「只有資料或日期真的變動才重繪＋跳頁」的行為一致。
//
// ## 移除的 ▴▾ 按鈕（design.md D10；spec「原生捲動」）
// Lively 版結尾 `if (CONFIG.columnAutoScroll) setupAutoScroll(ul); else
// setupScrollButtons(ul);` 整段移除：新版一律原生捲動（`overflow-y:auto`，見
// widget.css `.scroll-area`），不呼叫任何按鈕/自動輪播邏輯，這是 D11 明列的刻意差異。

/**
 * @param {HTMLElement} container widget.html 建立的內容容器。
 * @param {{ common: typeof import('../common.js'), config: object, snapshot: object,
 *   settings: object, onData: (fn: (snap: object) => void) => void,
 *   onSettings: (fn: (settings: object) => void) => void }} ctx widget.html 組好的執行環境。
 * @returns {() => void} 卸載函式（清掉計時器與事件監聽器）。
 */
export function mount(container, ctx) {
  const { common } = ctx;

  // task 5.2：透明度／主題色即時套用（design.md D4「settings 事件」；
  // specs/widget-host-lifecycle「設定持久化」Scenario「修改透明度」）。
  common.applyAppearance(ctx.settings);
  ctx.onSettings((settings) => common.applyAppearance(settings));

  container.classList.add('panel');
  // task 7.3：.panel 改為填滿視窗高度（widget.css `#widget-root > .panel{flex:1 1 auto}`），
  // 不再有 task 7.2 暫時的 MAX_HEIGHT_PX 上限。中間的 .evlist（flex:1 1 auto +
  // overflow-y:auto，見 widget.css `.scroll-area`）吸收 header／footer 之外的剩餘高度並
  // 捲動，超出視窗的部分捲動可見，header／footer 維持原尺寸不被壓縮。

  const header = document.createElement('header');
  const ttl = document.createElement('div');
  ttl.className = 'ttl';
  ttl.textContent = '總經日曆';
  const sub = document.createElement('div');
  sub.className = 'sub';
  sub.id = 'macroSub';
  // Lively 版 HTML 259–263 行的初始靜態內容（JS 尚未跑或 data.macro_meta 缺失時的預設值）。
  sub.textContent = '美國・歐元區・日本｜中高重要性';
  header.append(ttl, sub);

  const list = document.createElement('ul');
  list.className = 'evlist scroll-area';
  list.id = 'macroList';

  const foot = document.createElement('div');
  foot.className = 'hint';
  foot.id = 'macroFoot';

  container.append(header, list, foot);

  /** 原樣搬自 finance-calendar.html 456–512 行 `renderMacro`。 */
  function render(data) {
    list.innerHTML = '';
    const rows = data && Array.isArray(data.macro) ? data.macro : null;
    if (data && data.macro_meta) {
      sub.textContent = `${data.macro_meta.countries}｜${data.macro_meta.importance}`;
    }
    if (!rows || !rows.length) {
      list.innerHTML =
        '<div class="empty">尚無總經資料 —— 請執行 update_tw_events.py（見 README 排程設定）</div>';
      foot.textContent = '';
      return;
    }
    const now = new Date();
    const todayStr = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
    let lastDate = '';
    for (const e of rows) {
      const hasTs = typeof e.ts === 'number';
      const localD = hasTs ? new Date(e.ts * 1000) : null;
      const groupKey = hasTs
        ? `${localD.getFullYear()}-${String(localD.getMonth() + 1).padStart(2, '0')}-${String(localD.getDate()).padStart(2, '0')}`
        : e.date;
      if (groupKey !== lastDate) {
        lastDate = groupKey;
        const headD = hasTs ? localD : new Date(e.date + 'T00:00:00');
        const label = common.mdw(headD);
        // data-group／data-label：task 4.3 fix round 1 新增，讓 updateTimeSensitiveState()
        // 之後只需要比對 data-group 就能原地切換「今天」標記，不必重建這個節點（見檔頭說明）。
        list.insertAdjacentHTML(
          'beforeend',
          `<div class="dayhead${groupKey === todayStr ? ' today' : ''}" data-group="${common.esc(groupKey)}" data-label="${common.esc(label)}">${label}${groupKey === todayStr ? '　今天' : ''}</div>`,
        );
      }
      const evTime = hasTs
        ? `${String(localD.getHours()).padStart(2, '0')}:${String(localD.getMinutes()).padStart(2, '0')}`
        : e.time;
      // pastAt：這筆事件從「未發生」變成「已過去」的門檻時刻（epoch ms）。原本
      // `past` 判斷（`hasTs ? localD < now : new Date(e.dt) < now`）逐字保留，只是抽出
      // 門檻值存成 data-past-at，供 updateTimeSensitiveState() 之後原地重算用（task 4.3
      // fix round 1）。
      const pastAt = hasTs ? localD.getTime() : new Date(e.dt).getTime();
      const past = now.getTime() > pastAt;
      const vals = [];
      if (e.forecast) vals.push(`預 ${common.esc(e.forecast)}`);
      if (e.previous) vals.push(`前 ${common.esc(e.previous)}`);
      list.insertAdjacentHTML(
        'beforeend',
        `<li class="ev macro ${e.impact}${past ? ' past' : ''}" data-past-at="${pastAt}">
           <span class="d">${common.esc(evTime)}</span>
           <span class="cchip ${common.esc(e.country)}">${common.esc(e.flag)}</span>
           <span class="imp ${e.impact === 'high' ? 'high' : 'medium'}"></span>
           <b>${common.esc(e.title)}</b>
           <span class="n">${vals.join('｜')}</span>
         </li>`,
      );
    }
    // 先跳到第一筆未發生的事件。
    const first = list.querySelector('.ev:not(.past)');
    if (first) {
      list.scrollTop = Math.max(0, first.offsetTop - list.offsetTop - 34);
    } else {
      list.insertAdjacentHTML(
        'beforeend',
        '<div class="empty">之後暫無資料 —— 排程執行 update_tw_events.py 會補上下週</div>',
      );
    }
    foot.innerHTML = `資料更新：${common.esc(data.updated || '—')}｜時間為本機時區（UTC${common.tzOffsetLabel()}）｜預=預測值 前=前值`;
  }

  /**
   * task 4.3 fix round 1（Codex finding：host/ui/widgets/macro.js:147-148 原本呼叫
   * `render(lastData)` 會清空清單、重設捲動位置）。週期計時器與 `visibilitychange` 改
   * 呼叫這個函式：只原地更新「今天」標記（`.dayhead` 的 `today` class 與「　今天」字樣）
   * 與「已過去」灰階（`.ev` 的 `past` class），不清空清單、不重建 DOM、不動
   * `list.scrollTop`——使用者的閱讀位置不受影響。若清單目前是「尚無資料」的空狀態
   * （render() 走了 97–101 行的早退分支），下面兩個 querySelectorAll 選不到任何節點，
   * 靜默無事可做，安全。
   */
  function updateTimeSensitiveState() {
    const now = new Date();
    const todayStr = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}-${String(now.getDate()).padStart(2, '0')}`;
    list.querySelectorAll('.dayhead[data-group]').forEach((el) => {
      const isToday = el.dataset.group === todayStr;
      el.classList.toggle('today', isToday);
      el.textContent = isToday ? `${el.dataset.label}　今天` : el.dataset.label;
    });
    list.querySelectorAll('.ev[data-past-at]').forEach((el) => {
      const pastAt = Number(el.dataset.pastAt);
      el.classList.toggle('past', now.getTime() > pastAt);
    });
  }

  let lastData = ctx.snapshot && ctx.snapshot.status === 'ok' ? ctx.snapshot.data : null;
  render(lastData);

  ctx.onData((snap) => {
    lastData = snap.status === 'ok' ? snap.data : null;
    render(lastData);
  });

  // 跨日／節流計時器恢復後：只更新今天標記／過期狀態，不重建清單、不動捲動位置
  // （task 4.3 fix round 1，見檔頭「fix round 1」說明與 updateTimeSensitiveState() 註解）。
  const timer = setInterval(updateTimeSensitiveState, 60 * 1000);
  const onVisible = () => {
    if (!document.hidden) updateTimeSensitiveState();
  };
  document.addEventListener('visibilitychange', onVisible);

  return () => {
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
