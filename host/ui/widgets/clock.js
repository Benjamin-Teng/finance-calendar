// host/ui/widgets/clock.js
//
// 時鐘小工具：六個交易所的當地時間（change clock-six-exchanges，design.md D2／D3）。
// 左三右三共六格，每格三項：地區簡寫（時間左上角）、當地時間（24 小時制）、當地日期（YY-MM-DD）；
// 現貨開盤中的那一格亮起、時間為白色，其餘灰色。
// 資料模型在 `../exchange-clock.mjs`（純函式，測試見 host/tests/exchange-clock.test.mjs）；交易時段與
// 休市表讀內建的 wallpaper-config.default.json（與動態桌布同一份），讀到之前與讀取失敗時一律顯示為收盤。
//
// `report_content` 由 widget.html 的 ResizeObserver 統一處理（觀察 `mount()` 拿到的 `container`），本檔
// 不需要自己呼叫。時鐘不吃 `tw-events` 快照，`ctx.snapshot`／`ctx.onData` 用不到。

import { EXCHANGES, exchangeRows } from '../exchange-clock.mjs';
import { loadDefaultConfig } from '../wallpapers/lib/default-config.mjs';

/**
 * @param {HTMLElement} container widget.html 建立的內容容器（已掛在 `#widget-root` 底下）。
 * @param {{ common: typeof import('../common.js'), settings: object,
 *   onSettings: (fn: (settings: object) => void) => void }} ctx widget.html 組好的執行環境。
 * @returns {() => void} 卸載函式（清掉計時器與事件）。
 */
export function mount(container, ctx) {
  const { common } = ctx;

  common.applyAppearance(ctx.settings);
  ctx.onSettings((settings) => common.applyAppearance(settings));

  container.classList.add('panel', 'clockcard');

  const cells = EXCHANGES.map(({ code }) => {
    const rowEl = document.createElement('div');
    rowEl.className = 'xrow';
    const codeEl = document.createElement('span');
    codeEl.className = 'xcode';
    codeEl.textContent = code;
    const timeEl = document.createElement('span');
    timeEl.className = 'xtime';
    timeEl.textContent = '--:--';
    const dateEl = document.createElement('span');
    dateEl.className = 'xdate';
    // 簡寫疊在時間數字的左上角：兩者包在同一個定位容器裡。
    const headEl = document.createElement('div');
    headEl.className = 'xhead';
    headEl.append(codeEl, timeEl);
    rowEl.append(headEl, dateEl);
    container.append(rowEl);
    return { rowEl, timeEl, dateEl };
  });

  let config = null;
  let disposed = false;

  function tick() {
    const rows = exchangeRows(config, Date.now());
    rows.forEach((row, i) => {
      const { rowEl, timeEl, dateEl } = cells[i];
      // 每秒呼叫一次，內容多半沒變：只在有變時寫 DOM，避免無謂的重排與 ResizeObserver 通知。
      if (timeEl.textContent !== row.time) timeEl.textContent = row.time;
      if (dateEl.textContent !== row.date) dateEl.textContent = row.date;
      rowEl.classList.toggle('open', row.open);
    });
  }

  tick();
  const timer = setInterval(tick, 1000);

  loadDefaultConfig().then(
    (loaded) => {
      if (disposed) return;
      config = loaded;
      tick();
    },
    (e) => console.warn('[clock] 交易時段表讀取失敗，開收盤一律顯示為收盤：', e?.message || e),
  );

  // 視窗被遮蔽時 `setInterval` 可能被節流，解除遮蔽當下立即補一次，不等下一個整秒。
  const onVisible = () => {
    if (!document.hidden) {
      tick();
    }
  };
  document.addEventListener('visibilitychange', onVisible);

  return () => {
    disposed = true;
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
