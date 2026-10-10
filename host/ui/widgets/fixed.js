// host/ui/widgets/fixed.js
//
// 台股固定事件小工具（design.md D6／D10，task 4.4）。原搬自 finance-calendar.html
// （Lively 版，2026-09-27 對照 v6.2；515–577 行 `fixedOccurrences`／`renderFixed`；該檔
// 凍結只讀，不修改）：本地計算台指期／選擇權結算、季度期貨結算、月營收公布截止、財報公布
// 截止（計算本體在 `common.fixedOccurrences`）。
//
// ## 與 Lively 版的刻意差異（change fixed-events-holiday-shift）
// 台指期結算、財報截止、月營收截止遇非交易日順延到下一個交易日，列上加「原 M/D」徽章
// （`.badge.shifted`，`title` 為順延說明）；季結算標籤改為「那指・道瓊期貨季度結算」、不依
// 台灣休市日調整。交易日判斷要用 `tw-events` 通道的 `holidays`，所以本小工具**會讀**
// `ctx.snapshot`／`ctx.onData`——但只取 `holidays`：沒有資料、或快照不是 'ok' 時以空集合計算
// （只依週末順延），不顯示「尚無資料」（固定事件不靠資料也算得出來，空著反而退步；
// design.md D4）。D11 的 Lively 對照因此不再涵蓋本小工具（`compare/panels.mjs` 標為分歧）。
//
// ## 移除的 ▴▾ 按鈕（design.md D10；spec「原生捲動」）
// Lively 版 `fixedList` 本身沒有 `.scroll` class（不預設捲動，`#panelFixed{flex:none}`
// 讓它在雙欄版面裡絕不被壓縮），但新架構每個小工具是獨立視窗、有自己的高度上限，內容超出時
// 仍需要能捲動，因此這裡改用共用的 `.scroll-area`（`overflow-y:auto`）——正常情況下固定事件
// 清單很短（通常 < 10 行），實務上幾乎不會觸發捲動。
//
// ## 跨日／跨週重算，不打斷使用者捲動位置（task 4.3 fix round 1 的教訓）
// 內容由「今天」與休市日集合決定。同一週內每天的差異只有「哪一筆變成已過去／今天」這兩個
// CSS 標記，不需要整份清單重建。因此仿照 `macro.js` 的作法拆成兩個函式：
//   - `fullRender()`：真正重建 DOM，只在①初次掛載、②「本週」邊界改變（`weekStartOf(now)`
//     與上次重建時不同）、③休市日集合改變（資料推送時比對簽章；其他鍵更新不重建）時呼叫。
//   - `updateTimeSensitiveState()`：週期計時器（60 秒）與 `visibilitychange` 呼叫，先判斷
//     本週邊界是否變了——變了就整份重建；沒變就只原地切換 `past`／「今天」徽章
//     （`.badge.today`；不可用裸 `.badge`，會抓到順延徽章），不清空清單、不動 `scrollTop`。

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
  // `.evlist.scroll-area` 吸收剩餘高度並捲動。

  const header = document.createElement('header');
  const ttl = document.createElement('div');
  ttl.className = 'ttl';
  ttl.textContent = '台股固定事件';
  const sub = document.createElement('div');
  sub.className = 'sub';
  sub.id = 'fixedSub';
  header.append(ttl, sub);

  const list = document.createElement('ul');
  list.className = 'evlist scroll-area';
  list.id = 'fixedList';

  container.append(header, list);

  /** 快照中的休市日集合；快照不是 'ok' 或 `holidays` 不是陣列時為空集合（只依週末判斷）。 */
  function holidaysOf(snap) {
    const raw = snap && snap.status === 'ok' && snap.data ? snap.data.holidays : null;
    return new Set(Array.isArray(raw) ? raw.filter((h) => typeof h === 'string') : []);
  }
  const signatureOf = (set) => [...set].sort().join(',');

  let holidays = holidaysOf(ctx.snapshot);
  let lastWeekStartMs = null; // fullRender() 最後一次重建時對應的「本週」週日（epoch ms）。

  /**
   * 順延徽章（未順延時為空字串）。放在標題 `<b>` 裡、跟著文字換行，不另占一個 flex 欄位——
   * 窄寬度時另占一欄會把標題擠成一字一行（headless 274px 實測）。
   */
  function shiftBadge(o) {
    if (!o.note) return '';
    return `<span class="badge shifted" title="${common.esc(o.note)}">原 ${common.md(o.orig)}</span>`;
  }

  /** 原搬自 finance-calendar.html 545–577 行 `renderFixed`：整份重建 DOM。 */
  function fullRender() {
    const today = common.strip(new Date());
    const ws = common.weekStartOf(today);
    const we = common.addDays(ws, 6);
    lastWeekStartMs = ws.getTime();
    sub.textContent = `本週 ${common.md(ws)} – ${common.md(we)}`;

    const occ = common.fixedOccurrences(today, holidays, 13);
    const inWeek = occ.filter((o) => o.date >= ws && o.date <= we);
    const nexts = [];
    for (const o of occ) {
      // 每一類事件的「下一次」。
      if (o.date <= we) {
        continue;
      }
      if (nexts.some((n) => n.tag === o.tag)) {
        continue;
      }
      nexts.push(o);
    }

    list.innerHTML = '';
    if (!inWeek.length) {
      list.insertAdjacentHTML('beforeend', '<div class="empty">本週無固定事件</div>');
    }
    for (const o of inWeek) {
      const isToday = o.date.getTime() === today.getTime();
      const past = o.date < today;
      // data-date：供 updateTimeSensitiveState() 只比對時間戳就能原地切換 past／今天徽章，
      // 不必重建節點。「下一次」清單的項目一律是靜態 `.ev.past` 樣式（代表「非本週」的視覺
      // 淡化，不是真的「已過去」），不隨時間變化，不需要 data-date。
      list.insertAdjacentHTML(
        'beforeend',
        `<li class="ev week${past ? ' past' : ''}" data-date="${o.date.getTime()}">
           <span class="d">${common.mdw(o.date)}</span><b>${o.label}${shiftBadge(o)}</b>
           ${isToday ? '<span class="badge today">今天</span>' : ''}
         </li>`,
      );
    }
    list.insertAdjacentHTML('beforeend', '<div class="divider">── 下一次 ──</div>');
    for (const o of nexts.sort((a, b) => a.date - b.date)) {
      list.insertAdjacentHTML(
        'beforeend',
        `<li class="ev past"><span class="d">${common.mdw(o.date)}</span><b>${o.label}${shiftBadge(o)}</b></li>`,
      );
    }
  }

  /**
   * 週期計時器與 `visibilitychange` 呼叫這個函式，不是直接呼叫 `fullRender()`。「本週」邊界
   * 沒有改變時，只原地切換 `.ev.week` 的 `past` class 與「今天」徽章，不清空清單、不動
   * `scrollTop`；邊界真的改變了（跨過週日 24:00）才整份重建。
   */
  function updateTimeSensitiveState() {
    const today = common.strip(new Date());
    const ws = common.weekStartOf(today);
    if (ws.getTime() !== lastWeekStartMs) {
      fullRender();
      return;
    }
    list.querySelectorAll('.ev.week[data-date]').forEach((el) => {
      const d = new Date(Number(el.dataset.date));
      const isToday = d.getTime() === today.getTime();
      const past = d.getTime() < today.getTime();
      el.classList.toggle('past', past);
      let badge = el.querySelector('.badge.today');
      if (isToday && !badge) {
        badge = document.createElement('span');
        badge.className = 'badge today';
        badge.textContent = '今天';
        el.appendChild(badge);
      } else if (!isToday && badge) {
        badge.remove();
      }
    });
  }

  fullRender();

  // 資料推送：只有休市日集合變了才重建（其他鍵更新不打斷捲動位置；design.md D4）。
  // 'empty' 也要採用（轉成空集合）：`data_dir` 切換後 widget.html 會推一筆新世代的 empty（舊世代
  // 與同世代多餘的 empty 已在 widget.html 濾掉），代表新目錄尚無資料，舊目錄的休市日必須丟掉、
  // 改回只依週末（Codex review [medium]，回歸測試 host/tests/fixed-data-generation.test.mjs）。
  ctx.onData((snap) => {
    const next = holidaysOf(snap);
    if (signatureOf(next) === signatureOf(holidays)) {
      return;
    }
    holidays = next;
    fullRender();
  });

  const timer = setInterval(updateTimeSensitiveState, 60 * 1000);
  const onVisible = () => {
    if (!document.hidden) {
      updateTimeSensitiveState();
    }
  };
  document.addEventListener('visibilitychange', onVisible);

  return () => {
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
