// host/ui/widgets/clock.js
//
// 時鐘小工具（design.md D6／D10）。task 4.1 建立最小可顯示版本，task 4.3 補上
// design.md Risks 最後一條的節流計時器復原（visibilitychange）＋與 Lively 版對照測試。
// 只搬移 finance-calendar.html（Lively 版，2026-09-27 對照 v6.2；297–312 行「共用工具」＋
// 847–862 行 `tickClock`；該檔凍結只讀，不修改）裡「時間／日期／本週範圍」這三行文字的渲染
// 邏輯，證明 widget.html 骨架＋bridge.js 的 fixture 模式能跑通（task 4.1 驗收：
// `widget.html?w=clock` 顯示時鐘內容）。
//
// Lively 版的 `weekRange` 其實是另一個渲染函式 `renderFixed()` 寫入同一個 DOM（Lively 版
// 時鐘卡與固定事件面板共用整份頁面）。新架構下每個小工具是獨立視窗，不能像 Lively 版一樣
// 共用別的小工具的 DOM／狀態，所以這裡改成時鐘小工具自己用 `common.weekStartOf`／
// `common.md` 算一次「本週」範圍——算法（週日起算）與 Lively 版完全相同，只是搬到不依賴
// `fixed` 小工具是否存在／是否已載入的地方，這是 design.md D10「搬移方式：只改資料來源與
// DOM 容器」在多視窗架構下必要的解耦，非行為變更。
//
// 心跳凍結偵測（v4.5，Lively 版 `tickClock` 原本兼任 loadData 補讀與跨日重載的心跳）刻意
// 不搬：design.md D12 說這個機制由核心的 `pause` 事件與排程取代，不屬於小工具內容層
// （brief「共通背景」：心跳凍結偵測與 loadData 防重入本次搬移移除）。
//
// `report_content`（task 7.3 取代 `report_size`）由 widget.html 的 ResizeObserver 統一處理
// （觀察 `mount()` 拿到的 `container`），本檔不需要自己呼叫。時鐘小工具沒有遠端資料（不吃
// `tw-events` 快照），
// `ctx.snapshot`／`ctx.onData` 用不到，故不在下面的簽章裡解構它們。

/**
 * @param {HTMLElement} container widget.html 建立的內容容器（已在 widget.html 掛在
 *   `#widget-root` 底下）。
 * @param {{ common: typeof import('../common.js'), settings: object,
 *   onSettings: (fn: (settings: object) => void) => void }} ctx widget.html 組好的執行環境。
 * @returns {() => void} 卸載函式（清掉 `setInterval`）；widget.html 目前不主動呼叫，頁面
 *   關閉時由瀏覽器自然回收，先提供給未來需要熱替換小工具內容時使用。
 */
export function mount(container, ctx) {
  const { common } = ctx;

  // task 5.2：透明度／主題色即時套用（design.md D4「settings 事件」；
  // specs/widget-host-lifecycle「設定持久化」Scenario「修改透明度」）。
  common.applyAppearance(ctx.settings);
  ctx.onSettings((settings) => common.applyAppearance(settings));

  container.classList.add('panel', 'clockcard');

  const timeEl = document.createElement('div');
  timeEl.id = 'clockTime';
  timeEl.textContent = '--:--';

  const dateEl = document.createElement('div');
  dateEl.id = 'clockDate';

  const weekEl = document.createElement('div');
  weekEl.id = 'weekRange';

  container.append(timeEl, dateEl, weekEl);

  function tick() {
    const n = new Date();
    timeEl.textContent = `${common.pad2(n.getHours())}:${common.pad2(n.getMinutes())}`;
    dateEl.textContent = `${n.getFullYear()}/${n.getMonth() + 1}/${n.getDate()}　週${common.WD[n.getDay()]}`;
    const ws = common.weekStartOf(n);
    const we = common.addDays(ws, 6);
    weekEl.textContent = `本週 ${common.md(ws)} – ${common.md(we)}`;
  }

  tick();
  const timer = setInterval(tick, 1000);

  // 節流計時器恢復後立即重算（design.md Risks 最後一條：「被完全遮住的 WebView2
  // 可能節流計時器 → 解除遮蔽時頁面立即重算」）。時鐘本身每秒重新讀一次 `new Date()`，
  // 跨日／跨週不需要額外的日期比對邏輯（Lively 版 `tickClock` 那段 `dateChanged`／
  // `jumpMs` 心跳偵測是拿來觸發*其他*小工具重讀資料，design.md D12 已移除，不屬於
  // 時鐘小工具本身的顯示邏輯）；但如果視窗被遮蔽時 `setInterval` 被節流延後，解除遮蔽的
  // 當下應立即補一次，不等下一個整秒 tick，避免顯示卡在遮蔽前的舊時間。
  const onVisible = () => {
    if (!document.hidden) {
      tick();
    }
  };
  document.addEventListener('visibilitychange', onVisible);

  return () => {
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
