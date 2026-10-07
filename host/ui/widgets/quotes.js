// host/ui/widgets/quotes.js
//
// 行情條小工具（design.md D6／D10／D11，task 4.5）。原樣搬自 finance-calendar.html
// （Lively 版，2026-09-27 對照 v6.2；692–792 行 `fmtQuotePrice`／`fmtQuoteChg`／
// `quoteRowHtml`／`setupAutoScrollX`／`renderQuotes`；該檔凍結只讀，不修改）：只改
// 「資料從哪來」（Lively 版靠 `renderData()` 傳入的全域 `data` 參數，這裡改吃
// `ctx.snapshot`／`ctx.onData` 送來的 snapshot）、「DOM 容器」（Lively 版寫死
// `$('panelQuotes')`／`$('quoteList')` 全域 id，這裡改成 `mount()` 自建的區域變數）與
// 「跑馬燈的暫停/恢復」（新增，見下方「暫停/恢復」一節）。價格／漲跌幅格式化、頭尾相接
// 跑馬燈的位置推進與端點判斷邏輯逐字相同，不做任何「順手改善」。
//
// ## 無資料時內容高度為 0（design.md D7；task-4.5-brief.md 明確要求）
// Lively 版 `renderQuotes()` 開頭：`if (!CONFIG.showQuotes || !rows.length){
// wrap.style.display = 'none'; ...}`——沒有資料時整個面板隱藏，不像 `dynamic.js` 那樣顯示
// 「尚無資料」提示文字（quotes 是五個財經小工具裡唯一「沒資料就整個消失」的，這是 Lively
// 版原本就有的行為，不是本 task 新發明的差異）。這剛好符合新架構「內容高度 0 → 宿主隱藏
// 視窗」的機制（design.md D7；host 端套用邏輯是 task 3.2／7.3，本檔只需要正確切換
// display）：`render()` 用 `container.style.display` 整個切換，無資料時 display:none 讓
// widget.html 的 `ResizeObserver` 量到 0×0、進而呼叫 `report_content(false)`（task 7.3 取代
// `report_size(0, 0)`）。
//
// 「顯示行情條」開關本身（specs/widget-host-windows「小工具開關」）是
// `Settings.widgets.quotes.enabled`，屬於宿主端「要不要建立/顯示這個小工具視窗」的職責
// （design.md D6：「設定與視窗管理只依 id 運作，不認得小工具內容」），不是本檔要處理的事，
// 因此本檔不讀取任何對應的設定欄位（Lively 版 `CONFIG.showQuotes` 沒有對應的 Settings 欄位，
// 這個判斷整個搬到宿主端）。
//
// ## 寬度（design.md D7；host/src/widgets.rs `WIDGET_SPECS` 的設計寬度出處說明）
// Lively 版 `.ticker` 的實際寬度＝所在 CSS Grid 兩欄寬＋欄距（500+470+22=992px）；格線模型下
// 宿主以倍率（矩形邏輯寬 ÷ 設計寬度 992）縮放頁面，未夾住時 CSS viewport 寬＝992。
// fix F6（review monitorid-visual low）：container 不再寫死 CSS 寬度，由 `#widget-root`
// （flex 欄、交叉軸預設 stretch，四周留 `--widget-gap`）決定，面板寬＝視窗寬扣左右 gap。
// 原本固定 `992px − 2×gap`：倍率被夾在上限 3.0 時（例如 4K＠100% 拉滿 48 欄，viewport
// 寬 1280）面板只有 976 寬並靠左，右側留白遠大於 gap。「內容是否超寬需要跑馬燈」的判斷
// （`scrollWidth > clientWidth`）因此跟著實際面板寬度走；13 檔行情的內容寬度遠超過任何
// 合理的面板寬度，對照測試不受分頁寬度影響（見 host/tests/compare/README.md）。
//
// ## 移除的「橫貫兩欄」版面耦合
// Lively 版 `renderQuotes()` 結尾會設 `document.documentElement.style.setProperty('--qh',
// '66px')`，讓同一頁面另外兩欄（`.col-right`）的高度上限跟著讓出行情條的位置——這是 Lively
// 版「一張桌布、多面板共用一個 `.layout` grid」特有的耦合，新架構每個小工具是獨立視窗，沒有
// 「讓出空間給隔壁小工具」這回事，因此不搬（同 `clock.js`／`macro.js` 不搬 `--z` 縮放變數，
// design.md D7 明訂的必要差異）。
//
// ## 暫停/恢復（新增；widget-host-lifecycle spec「自動暫停」／finance-widgets spec
// 「行情條跑馬燈」；task-4.5-brief.md 明確要求）
// Lively 版沒有「暫停」這個概念——它整個 WebView2／JS 執行緒被 Windows 凍結時動畫自然跟著
// 停（v4.5 心跳偵測就是拿來偵測「解凍後要不要補一次重算」，design.md D12 已移除，不影響本檔）。
// 新架構每個小工具是獨立視窗，仍然可能持續在跑（renderer 沒被系統凍結，只是宿主判定「使用者
// 現在不該被動畫打擾」，例如全螢幕簡報、螢幕鎖定），因此需要宿主明確用 `pause` 事件
// （`{ paused, reason }`，design.md D4）通知。停止與恢復都只操作 `requestAnimationFrame`／
// `setTimeout` 的排程，**不動** `state.pos`／`state.dir`（跑馬燈目前的位置與方向）：
//   - 暫停：取消排定中的 `raf`／`tm`，`state` 物件本身保留。
//   - 恢復：用同一個 `state` 物件重新排程；重設 `state.last = 0`，讓 `step()` 下一幀重新起算
//     `dt`（否則 `dt = t - state.last` 會把「暫停期間經過的真實時間」全部當成一次要追趕的
//     位移，畫面上會像瞬間跳動一大段，不是「從停止處繼續」該有的效果）。
// 資料更新（`ctx.onData`）與暫停是兩件獨立的事：`render()` 一律先停止舊動畫再重建 DOM
// （逐字沿用 Lively 版「每次 renderQuotes 都重設 `scrollLeft=0`、重新判斷要不要跑馬燈」），
// 若目前正處於暫停狀態，新的動畫狀態一樣先準備好（`state` 物件），但不會真的排程
// `requestAnimationFrame`，等下一次收到 `pause:{paused:false}` 才會開始跑——沒有這一步的話，
// 暫停期間收到的資料更新會讓跑馬燈「偷跑」，違反「暫停時 SHALL 停止捲動」。
//
// ## fix round 1（task-4.5-codex.md [medium]）
// `ctx.onPause` 原本每次收到事件都無條件依 `payload.paused` 呼叫 `stopAnimation()`／
// `scheduleStep()`，沒有檢查「狀態其實沒變」。design.md D4 沒有保證 `pause` 事件不會重送
// （例如宿主初始狀態通知、之後可能的輪詢重送），重複收到 `{paused:false}` 會讓
// `scheduleStep()` 又排一筆新的 raf／timeout、覆寫 `state.raf`／`state.tm`，先前那一筆的
// 參照就此遺失——`stopAnimation()` 只能取消「最後一筆」，較舊的殘留鏈會繼續呼叫
// `step()`、繼續推進 `state.pos`、繼續重新排程自己，暫停也停不掉它，直接違反「暫停時
// SHALL 停止捲動」。
// 對策：① `onPause` handler 冪等化，只在 `paused` 狀態真的改變時才啟停一次；②
// `scheduleStep()` 加一道「已有排定中的動畫就不再新增」的防線（belt-and-suspenders，就算
// 未來又有呼叫端沒走 idempotent 檢查，也不會疊出第二條鏈）；③ `step()` 一開始檢查
// `paused`，暫停時直接回傳、不重新排程——就算佇列裡曾經留下殘留的一筆，觸發時也只會停在
// 原地、不會再排下一幀。重現與回歸測試：`host/tests/quotes-pause-idempotent.test.mjs`
// （用可控制的假排程器精確數「排定中筆數」，模擬「重複恢復後再暫停」，修好前 FAIL、
// 修好後 PASS）。

const AUTO_SCROLL_PX_PER_SEC = 24; // Lively 版 CONFIG.autoScrollPxPerSec 預設值。
const AUTO_SCROLL_PAUSE_SEC = 4; // Lively 版 CONFIG.autoScrollPauseSec 預設值。
const QUOTES_LOOP = true; // Lively 版 CONFIG.quotesLoop 預設值（頭尾相接連續捲動）。
// 這三個常數在 host/src/settings.rs 沒有對應的 Settings 欄位（2026-09-28 核對），故沿用
// Lively 版預設值當固定常數；如果之後要做成可調設定，這裡是加欄位/改讀 ctx.settings 的地方。

function fmtQuotePrice(q) {
  switch (q.kind) {
    case 'yield':
      return q.price.toFixed(2) + '%';
    case 'fx':
    case 'cmdty':
      return q.price.toFixed(2);
    case 'index':
      return Math.round(q.price).toLocaleString('zh-Hant');
    case 'stock':
      return q.price >= 1000 ? Math.round(q.price).toLocaleString('zh-Hant') : q.price.toFixed(1);
    default:
      return String(q.price);
  }
}

/* 回傳 {dir, text}；dir ∈ up/dn/flat 供上色（紅漲綠跌，台股慣例）。yield（美債殖利率／SOFR）
   顯示絕對值、不加 %；其餘顯示 chg_pct。四捨五入後才算「是否為 0」，避免顯示 ▲ 0.00% 這種
   箭頭與數字矛盾的畫面（逐字沿用 finance-calendar.html 708–717 行）。 */
function fmtQuoteChg(q) {
  if (q.kind === 'yield') {
    const v = Math.abs(q.chg_abs).toFixed(2);
    if (Number(v) === 0) return { dir: 'flat', text: '—' };
    return { dir: q.chg_abs > 0 ? 'up' : 'dn', text: `${q.chg_abs > 0 ? '▲' : '▼'} ${v}` };
  }
  const v = Math.abs(q.chg_pct).toFixed(2);
  if (Number(v) === 0) return { dir: 'flat', text: '—' };
  return { dir: q.chg_pct > 0 ? 'up' : 'dn', text: `${q.chg_pct > 0 ? '▲' : '▼'} ${v}%` };
}

function quoteRowHtml(common, q) {
  const { dir, text } = fmtQuoteChg(q);
  return `<li>
    <span class="tname">${common.esc(q.name)}</span>
    <span class="tprice">${fmtQuotePrice(q)}</span>
    <span class="tchg ${dir}">${text}</span>
  </li>`;
}

/** 通道從未取得有效快照，或該次快照沒有 `quotes` 陣列時，一律視為「無資料」（見檔頭「無資料
 * 時內容高度為 0」）——與 Lively 版 `renderQuotes()` 用同一句 `(data &&
 * Array.isArray(data.quotes)) ? data.quotes : []` 的效果相同,只是多包一層 snapshot.status
 * 判斷（widget-data-feed spec「尚無資料」的通道初始狀態）。 */
function extractRows(snapshot) {
  if (!snapshot || snapshot.status !== 'ok' || !snapshot.data) {
    return [];
  }
  return Array.isArray(snapshot.data.quotes) ? snapshot.data.quotes : [];
}

/**
 * @param {HTMLElement} container widget.html 建立的內容容器。
 * @param {{ common: typeof import('../common.js'), config: object, snapshot: object,
 *   settings: object, onData: (fn: (snap: object) => void) => void,
 *   onSettings: (fn: (settings: object) => void) => void,
 *   onPause: (fn: (payload: { paused: boolean, reason?: string }) => void) => void }} ctx
 *   widget.html 組好的執行環境。
 * @returns {() => void} 卸載函式（清掉排定中的動畫）。
 */
export function mount(container, ctx) {
  const { common } = ctx;

  // task 5.2：透明度／主題色即時套用（design.md D4「settings 事件」；
  // specs/widget-host-lifecycle「設定持久化」Scenario「修改透明度」）。
  common.applyAppearance(ctx.settings);
  ctx.onSettings((settings) => common.applyAppearance(settings));

  let ul = null;
  // 跑馬燈狀態；沿用 Lively 版 setupAutoScrollX 的欄位命名（dir/pos/raf/tm/last），另加
  // loopAt（>0＝頭尾相接模式的一份內容寬度，0＝來回模式）。render() 重建內容時整個換新。
  let state = null;
  let paused = false;

  function stopAnimation() {
    if (!state) return;
    cancelAnimationFrame(state.raf);
    clearTimeout(state.tm);
    state.raf = 0;
    state.tm = 0;
  }

  /* 逐字搬自 finance-calendar.html 737–759 行 setupAutoScrollX 的 step()，只把外層作用域的
     el/loopAt/speed 換成本檔的 ul/state.loopAt/AUTO_SCROLL_PX_PER_SEC。
     fix round 1（task-4.5-codex.md [medium]）：多加 `paused` 檢查——就算佇列裡殘留了一筆
     暫停前排定的呼叫，觸發時也直接停在原地、不再重新排程自己（belt-and-suspenders，見
     檔頭「fix round 1」說明；正常情況下 idempotent 的 onPause 已經不會製造殘留排程）。 */
  function step(t) {
    if (!state || !ul || paused) return;
    if (!state.last) state.last = t;
    const dt = Math.min(100, t - state.last);
    state.last = t;
    if (state.loopAt > 0) {
      state.pos += (AUTO_SCROLL_PX_PER_SEC * dt) / 1000;
      while (state.pos >= state.loopAt) state.pos -= state.loopAt; // while 非 if：見原檔註解
      ul.scrollLeft = state.pos;
      state.raf = requestAnimationFrame(step);
      return;
    }
    const max = ul.scrollWidth - ul.clientWidth;
    if (max > 4) {
      state.pos = Math.max(0, Math.min(max, state.pos + (state.dir * AUTO_SCROLL_PX_PER_SEC * dt) / 1000));
      ul.scrollLeft = state.pos;
      if ((state.dir > 0 && state.pos >= max) || (state.dir < 0 && state.pos <= 0)) {
        state.dir *= -1;
        state.last = 0;
        state.tm = setTimeout(() => {
          state.raf = requestAnimationFrame(step);
        }, AUTO_SCROLL_PAUSE_SEC * 1000);
        return;
      }
    } else {
      state.pos = 0;
    }
    state.raf = requestAnimationFrame(step);
  }

  /** 排定下一次 step()：loopAt>0 立即起步（頭尾相接不需要停頓），否則先停頓
   * AUTO_SCROLL_PAUSE_SEC 秒再起步（來回模式在端點也是這個節奏）。暫停中不排程
   * （見檔頭「暫停/恢復」）。fix round 1：已有排定中的動畫（`state.raf`／`state.tm` 非 0）
   * 就不再新增第二筆，避免呼叫端沒做到冪等時疊出多條鏈（belt-and-suspenders，主要防線是
   * `onPause` 的冪等檢查，見檔頭「fix round 1」說明）。 */
  function scheduleStep() {
    if (paused || !state || state.raf || state.tm) return;
    if (state.loopAt > 0) {
      state.raf = requestAnimationFrame(step);
    } else {
      state.tm = setTimeout(() => {
        state.raf = requestAnimationFrame(step);
      }, AUTO_SCROLL_PAUSE_SEC * 1000);
    }
  }

  function render(snapshot) {
    const rows = extractRows(snapshot);

    if (!rows.length) {
      stopAnimation();
      state = null;
      ul = null;
      container.style.display = 'none';
      container.className = '';
      container.innerHTML = '';
      return;
    }

    container.style.display = '';
    container.className = 'panel ticker';
    container.innerHTML = '';
    ul = document.createElement('ul');
    ul.className = 'tlist';
    container.appendChild(ul);

    ul.innerHTML = rows.map((q) => quoteRowHtml(common, q)).join('');
    ul.scrollLeft = 0;
    stopAnimation();
    state = null;

    if (ul.scrollWidth > ul.clientWidth) {
      // 頭尾相接：先確認單份內容確實超寬才複製第二份（否則複製本身會造成超寬），一份的
      // 精確寬度＝第二份首項與第一份首項的 offsetLeft 差（含項間邊框與間距）——逐字沿用
      // finance-calendar.html 783–787 行。
      if (QUOTES_LOOP) {
        const n = ul.children.length;
        ul.insertAdjacentHTML('beforeend', rows.map((q) => quoteRowHtml(common, q)).join(''));
        state = { dir: 1, pos: ul.scrollLeft, raf: 0, tm: 0, last: 0, loopAt: ul.children[n].offsetLeft - ul.children[0].offsetLeft };
      } else {
        state = { dir: 1, pos: ul.scrollLeft, raf: 0, tm: 0, last: 0, loopAt: 0 };
      }
      scheduleStep();
    }
  }

  render(ctx.snapshot);
  ctx.onData((snap) => render(snap));
  ctx.onPause((payload) => {
    const next = !!(payload && payload.paused);
    // fix round 1（task-4.5-codex.md [medium]）：冪等化——只在暫停狀態真的改變時才啟停一次。
    // design.md D4 沒有保證 pause 事件不重複，重複的 {paused:false} 若每次都呼叫
    // scheduleStep()，會疊出多條無法取消的動畫鏈（見檔頭「fix round 1」說明）。
    if (next === paused) return;
    paused = next;
    if (paused) {
      stopAnimation();
    } else {
      scheduleStep();
    }
  });

  return () => {
    stopAnimation();
  };
}
