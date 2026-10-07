// host/ui/widgets/fixed.js
//
// 台股固定事件小工具（design.md D6／D10，task 4.4）。原樣搬自 finance-calendar.html
// （Lively 版，2026-09-27 對照 v6.2；515–577 行 `fixedOccurrences`／`renderFixed`；該檔
// 凍結只讀，不修改）：純本地計算（台指期／選擇權結算、季度期貨結算、月營收公布截止、財報
// 公布截止），不依賴任何遠端資料——`registry.js` 讓本小工具訂閱 `tw-events` 通道只是為了與
// 其餘四個財經小工具一致（design.md D6：id → 通道對照表），內容本身完全不吃 `ctx.snapshot`／
// `ctx.onData`，這點與 Lively 版 `renderFixed()` 不接收任何 `data` 參數的行為一致，因此本檔
// 不需要處理 widget-data-feed spec「尚無資料」（該狀態描述「通道從未取得有效快照」，但本
// 小工具的內容從不從該通道取值，沒有「有資料／沒資料」的區別）——與 task 4.3 的 `clock.js`
// 同一個判斷（時鐘同樣訂閱 `tw-events` 卻不使用其內容）。
//
// ## 移除的 ▴▾ 按鈕（design.md D10；spec「原生捲動」）
// Lively 版 `fixedList` 本身沒有 `.scroll` class（不預設捲動，`#panelFixed{flex:none}`
// 讓它在雙欄版面裡絕不被壓縮），但新架構每個小工具是獨立視窗、有自己的高度上限
// （`MAX_HEIGHT_PX`），內容超出時仍需要能捲動（spec「原生捲動」對「內容超出小工具高度的清單」
// 一律要求），因此這裡改用共用的 `.scroll-area`（`overflow-y:auto`）——正常情況下固定事件
// 清單很短（通常 < 10 行），實務上幾乎不會觸發捲動，這個差異不影響 D11 對照測試（比對的是
// 文字內容，不比對是否有捲軸），不需要在 `panels.mjs` 額外列排除規則。
//
// ## 跨日／跨週重算，不打斷使用者捲動位置（task 4.3 fix round 1 的教訓，task-4.4-brief.md
// 「補充」段落明確要求比照辦理）
// `renderFixed()` 的內容（本週有哪些事件、哪些已過去、是否為今天）完全由「今天」這個日期
// 決定，且**只有在跨過一週邊界（週日 24:00）時，「本週」實際列出的事件集合才會改變**——
// 同一週內每天的差異只有「哪一筆變成已過去／今天」這兩個 CSS 標記，不需要整份清單重建。
// 因此仿照 `macro.js` 的作法拆成兩個函式：
//   - `fullRender()`：真正重建 DOM（含週次標題、清單、divider、下一次清單），只在①初次掛載、
//     ②偵測到「本週」邊界已改變（`weekStartOf(now)` 與上次重建時不同）時呼叫。
//   - `updateTimeSensitiveState()`：週期計時器（60 秒，沿用 Lively `freshCheckMin` 量級）與
//     `visibilitychange`（design.md Risks 最後一條：節流計時器解除遮蔽後立即補算）呼叫，
//     先判斷本週邊界是否變了——變了就整份重建（等同 Lively 版跨日觸發 `refreshAll()` 之後
//     `renderFixed()` 整份重寫 `fixedList.innerHTML` 的效果，這種情況下清單內容本來就不同、
//     捲動位置沒有「保留」的意義）；沒變就只原地切換 `past`／「今天」徽章，不清空清單、不動
//     `scrollTop`。

/**
 * @param {HTMLElement} container widget.html 建立的內容容器。
 * @param {{ common: typeof import('../common.js'), config: object, settings: object,
 *   onSettings: (fn: (settings: object) => void) => void }} ctx widget.html 組好的
 *   執行環境（本小工具不需要 `ctx.snapshot`／`ctx.onData`，見檔頭說明）。
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
  // 不再有 task 7.2 暫時的 MAX_HEIGHT_PX 上限；`.evlist.scroll-area` 吸收剩餘高度並捲動
  // （固定事件清單通常很短，實務上幾乎不會觸發，見上方「移除的 ▴▾ 按鈕」一節）。

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

  /** 原樣搬自 finance-calendar.html 515–543 行 `fixedOccurrences`。 */
  function fixedOccurrences(monthsAhead) {
    const today = common.strip(new Date());
    const out = [];
    for (let k = 0; k <= monthsAhead; k++) {
      const y = today.getFullYear();
      const m = today.getMonth() + k;
      const yy = new Date(y, m, 1).getFullYear();
      const mm = new Date(y, m, 1).getMonth();

      // 台指期／選擇權結算：每月第三個週三。
      out.push({ date: common.nthWeekday(yy, mm, 3, 3), tag: '期權', label: '台指期／選擇權結算' });

      // 台股＋那斯達克／道瓊期指季度結算：3/6/9/12 月第三個週五。
      if ([2, 5, 8, 11].includes(mm)) {
        out.push({
          date: common.nthWeekday(yy, mm, 5, 3),
          tag: '季結算',
          label: '台股・那指・道瓊期貨季度結算',
        });
      }

      // 月營收公布截止：每月 10 日（公布上一個月）。
      const rev = new Date(yy, mm, 10);
      const pm = new Date(yy, mm, 0).getMonth() + 1;
      out.push({ date: rev, tag: '營收', label: `${pm}月營收公布截止（10日前）` });

      // 財報公布截止。
      const fin = { 2: [31, 'Q4＋年報'], 4: [15, 'Q1'], 7: [14, 'Q2'], 10: [14, 'Q3'] }[mm];
      if (fin) {
        out.push({ date: new Date(yy, mm, fin[0]), tag: '財報', label: `${fin[1]} 財報公布截止` });
      }
    }
    return out.sort((a, b) => a.date - b.date);
  }

  let lastWeekStartMs = null; // fullRender() 最後一次重建時對應的「本週」週日（epoch ms）。

  /** 原樣搬自 finance-calendar.html 545–577 行 `renderFixed`：整份重建 DOM。 */
  function fullRender() {
    const today = common.strip(new Date());
    const ws = common.weekStartOf(today);
    const we = common.addDays(ws, 6);
    lastWeekStartMs = ws.getTime();
    sub.textContent = `本週 ${common.md(ws)} – ${common.md(we)}`;

    const occ = fixedOccurrences(13);
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
      // data-date：task 4.4 沿用 macro.js（task 4.3 fix round 1）的作法，供
      // updateTimeSensitiveState() 之後只需要比對這個時間戳就能原地切換 past／今天徽章，
      // 不必重建這個節點（見檔頭「跨日／跨週重算」說明）。「下一次」清單（nexts）的項目一律
      // 是靜態 `.ev.past` 樣式（代表「非本週」的視覺淡化，不是真的「已過去」），本來就不隨
      // 時間變化，不需要 data-date。
      list.insertAdjacentHTML(
        'beforeend',
        `<li class="ev week${past ? ' past' : ''}" data-date="${o.date.getTime()}">
           <span class="d">${common.mdw(o.date)}</span><b>${o.label}</b>
           ${isToday ? '<span class="badge">今天</span>' : ''}
         </li>`,
      );
    }
    list.insertAdjacentHTML('beforeend', '<div class="divider">── 下一次 ──</div>');
    for (const o of nexts.sort((a, b) => a.date - b.date)) {
      list.insertAdjacentHTML(
        'beforeend',
        `<li class="ev past"><span class="d">${common.mdw(o.date)}</span><b>${o.label}</b></li>`,
      );
    }
  }

  /**
   * task 4.4（比照 macro.js task 4.3 fix round 1）：週期計時器與 `visibilitychange` 呼叫
   * 這個函式，不是直接呼叫 `fullRender()`。「本週」邊界沒有改變時，只原地切換 `.ev.week`
   * 的 `past` class 與「今天」徽章，不清空清單、不動 `scrollTop`；邊界真的改變了（跨過週日
   * 24:00）才整份重建（此時清單內容本來就不同，捲動位置沒有「保留」的意義，等同 Lively 版
   * 跨日觸發 `refreshAll()` 的效果）。
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
      let badge = el.querySelector('.badge');
      if (isToday && !badge) {
        badge = document.createElement('span');
        badge.className = 'badge';
        badge.textContent = '今天';
        el.appendChild(badge);
      } else if (!isToday && badge) {
        badge.remove();
      }
    });
  }

  fullRender();

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
