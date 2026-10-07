// host/ui/widgets/dynamic.js
//
// 台股動態事件小工具（design.md D6／D10，task 4.4）。原樣搬自 finance-calendar.html
// （Lively 版，2026-09-27 對照 v6.2；579–689 行 `TYPE_META`／`renderDyn`；`isTradingDay`／
// `targetDay`／`isoDate`／`todayBase` 已抽到 `common.js`，見該檔案；該檔凍結只讀，不修改）：
// 只改「資料從哪來」（Lively 版靠全域 `data` 參數，這裡改吃完整的 snapshot 物件——
// `ctx.snapshot`／`ctx.onData` 推送的形狀，理由見下方「尚無資料」一節）、「DOM 容器」
// （Lively 版寫死 `$('dynList')` 等全域 id，這裡改成 `mount()` 自己建立的區域變數）與
// 「顯示除權息」改由設定控制（design.md D11 明列的刻意差異，見下方），上節預告清單／下節
// 處置股（非交易日順延、當日制）判斷邏輯逐字相同，不做任何「順手改善」。
//
// ## 「顯示除權息」設定（task-4.4-brief.md；specs/finance-widgets「顯示除權息」）
// Lively 版 `CONFIG.dynTypes` 是寫死的常數（預設 `['earnings','conference','meeting',
// 'punish']`，不含 `'dividend'`），要顯示除權息得手動改檔案。新架構改由
// `host/src/settings.rs` 的 `Settings.show_dividend`（設定視窗尚未實作，task 5.2）控制：
// `true` 時把 `'dividend'` 併入本檔算出的 `dynTypes`（上節，`'punish'` 以外的類型）。
// `dividend` 在陣列中的排序（財報之後、法說會之前）只影響「同一天有多種類型事件」時的
// tie-break 順序，Lively 版原始碼只註明「把 'dividend' 加回陣列即可恢復」未明確規定位置——
// 這是本 task 的判斷，controller 可覆核。對照測試（D11）用的預設設定
// （`host/ui/fixtures/settings.json` 的 `show_dividend:false`）與 Lively 版預設值相同，
// 兩邊本來就不會出現除權息項目，不需要在 `panels.mjs` 額外排除；「設定打開後也與 Lively 版
// 手動加入 'dividend' 後的行為一致」另外用 `host/tests/compare/verify-dividend-setting.mjs`
// 驗證（見 task-4.4-report.md）。
//
// ## 「尚無資料」（widget-data-feed spec；task-4.4-brief.md 明確要求）
// widget-data-feed spec「尚無資料」Requirement：某通道從未取得有效快照時，宿主回報該通道為
// 空、訂閱的小工具 SHALL 顯示提示而非空白；Scenario「首次啟動且資料目錄為空」進一步要求
// 顯示提示與目前的資料目錄路徑。這是新架構特有的資料流狀態（Lively 版讀同資料夾的
// `tw_events.js`，沒有「這個通道從未取得快照」這種可回報的中介狀態，抓不到就是
// `renderDyn` 615–622 行 `!data || !Array.isArray(data.events)` 這個分支），因此本檔的
// `render()` 改吃**完整的 snapshot 物件**（`{channel, status, data, meta}`，與
// `ctx.snapshot`／`ctx.onData` 送來的形狀一致），先判斷 `status !== 'ok'`（對應
// `get_snapshot`／widget.html `data` 事件正規化後的「empty」狀態：通道在**目前 registry
// 世代**尚未取得任何有效快照。除了首次啟動，task 2.7 follow-up 起改 `data_dir` 重建註冊表
// 時，核心會對已訂閱、新世代尚無快照的通道推送一筆新世代的 empty，所以「曾有資料之後收到
// empty」是正常情況（資料目錄換了）；同一世代內則不會在 ok 之後送出 empty（widget.html 也會
// 丟棄同世代晚到的 empty，見 `isStaleSameGenerationEmpty`），見 `host/src/widgets.rs`
// `push_empty_snapshots_after_rebuild`／`host/ui/widget.html` 對 `data` 事件的正規化），顯示新訊息
// （含 `ctx.settings.data_dir`）；`status==='ok'` 之後才走 Lively 版原本的
// `!Array.isArray(data.events)` 防禦性 fallback（理論上通過後端驗證的 'ok' 快照不會走到
// 這裡，保留只是逐字沿用原始防呆，不是本 task 新增的行為）。
//
// ## 移除的 ▴▾ 按鈕（design.md D10；spec「原生捲動」）
// Lively 版結尾 `if (CONFIG.columnAutoScroll) setupAutoScroll(ul); else
// setupScrollButtons(ul);` 整段移除，改用共用的 `.scroll-area`（`overflow-y:auto`），
// design.md D11 明列的刻意差異。
//
// ## 跨日重算，不打斷使用者捲動位置（task 4.3 fix round 1 的教訓，task-4.4-brief.md
// 「補充」段落明確要求比照辦理）
// 本小工具顯示的內容完全由 `(data, todayIso)` 決定：上節「今天起」的過濾條件
// （`e.date >= todayIso`）與下節處置股的目標交易日（`targetDay()`，同樣以 `todayBase()`
// 為起點）都只在**日期真的改變**（跨過午夜，或 `window.__TEST_TODAY` 被對照測試改寫）時
// 才會產生不同的清單內容；同一天之內反覆呼叫必定得到完全相同的結果（不像 macro.js 的
// 「已過去」灰階會在同一天內隨每分鐘經過的事件逐步改變，見該檔說明）。因此週期計時器
// （60 秒）與 `visibilitychange` 不需要 macro.js 那種「原地切換 CSS class」機制：只需要
// 比較「這次的 todayIso」與「上次重建時的 todayIso」，沒變就什麼都不做（不清空清單、不動
// `scrollTop`），變了才呼叫 `render()` 整份重建（此時清單內容本來就不同，捲動位置沒有
// 「保留」的意義，等同 Lively 版跨日觸發 `refreshAll()` 的效果）。
//
// 「已 N 天未更新」頁腳警示（`data.fetched` 超過 3 天）理論上會在 fetched 時刻起算滿 24 小時
// 的整數倍時翻動（不是午夜），比 `todayIso` 的日期粒度更細；但 Lively 版本身也只在
// `dailyReloadAt`（04:00 整頁重載）或 `freshCheckMin`（偵測到 `data.updated` 真的改變）
// 時才重繪，同樣不會逐毫秒即時更新這個天數——本檔跟隨 `todayIso` 變化時一併重算頁腳，
// 更新粒度與 Lively 版原本的粗粒度一致甚至更頻繁，非本 task 新增的落差。
//
// ## fix round 1（task-4.4-codex.md [medium]）
// 上一段的推論有漏洞：計時器與 visibilitychange 原本只在 `todayIso` 改變時才呼叫
// `render()`，而 `render()` 是唯一會重算頁腳的地方——「同一天之內」資料來源停止更新、
// 稍晚才跨過 3 天門檻的情況下，`todayIso` 沒變，頁腳會一路停在「未過期」直到隔天
// `todayIso` 真的改變才補上警示，最多延遲近一天，比 Lively 版的粗粒度更粗（Lively 版的
// `dailyReloadAt`／`freshCheckMin` 至少不受「今天的日期」這個額外條件限制）。
// 對策：把頁腳的計算與寫入抽成獨立的 `renderFooter(snapshot)`，`render()` 一律呼叫它
// （涵蓋「尚無資料」／「讀不到資料」兩個早退分支，不用各自寫一次 `foot.textContent=''`），
// 計時器與 visibilitychange 則在 `todayIso` **沒有**改變時改呼叫 `renderFooter(lastSnapshot)`
// （`todayIso` 有變仍走整份 `render()`，其中已經含 `renderFooter`，不用重複呼叫）——
// 這樣頁腳每 60 秒（或每次恢復可見）都會用當下的 `Date.now()` 重算，清單內容與捲動位置
// 完全不受影響（沒有呼叫 `list.innerHTML=''`）。重現與回歸測試：
// `host/tests/dynamic-footer-age.test.mjs`（在 Node vm 內用可控制的假 `Date` 與可手動觸發
// 的計時器回呼，模擬「今天沒變、牆上時鐘跨過 3 天門檻」，修好前 FAIL、修好後 PASS）。

const TYPE_META = {
  earnings: { chip: '財報', cls: 'var(--c-earn)' },
  dividend: { chip: '除權息', cls: 'var(--c-div)' },
  meeting: { chip: '股東會', cls: 'var(--c-meet)' },
  conference: { chip: '法說會', cls: 'var(--c-conf)' },
  punish: { chip: '處置', cls: 'var(--c-punish)' },
};

/** Lively 版預設 `CONFIG.dynTypes`（finance-calendar.html v6.2）。 */
const DEFAULT_DYN_TYPES = ['earnings', 'conference', 'meeting', 'punish'];

/**
 * 「顯示除權息」設定 → 本次渲染要用的 `dynTypes`（見檔頭說明，排序為本 task 的判斷）。
 * @param {{ show_dividend?: boolean }|null|undefined} settings
 */
function dynTypesFor(settings) {
  if (settings && settings.show_dividend) {
    return ['earnings', 'dividend', 'conference', 'meeting', 'punish'];
  }
  return DEFAULT_DYN_TYPES;
}

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

  container.classList.add('panel');
  // task 7.3：.panel 改為填滿視窗高度（widget.css `#widget-root > .panel{flex:1 1 auto}`），
  // 不再有 task 7.2 暫時的 MAX_HEIGHT_PX 上限；`.evlist.scroll-area` 吸收剩餘高度並捲動。

  const header = document.createElement('header');
  const ttl = document.createElement('div');
  ttl.className = 'ttl';
  ttl.textContent = '台股動態事件';
  const sub = document.createElement('div');
  sub.className = 'sub';
  sub.id = 'dynSub';
  header.append(ttl, sub);

  const list = document.createElement('ul');
  list.className = 'evlist scroll-area';
  list.id = 'dynList';

  const foot = document.createElement('div');
  foot.className = 'hint';
  foot.id = 'dynFoot';

  container.append(header, list, foot);

  let currentSettings = ctx.settings || null;
  let lastSnapshot = null;
  let lastRenderedTodayIso = null;

  /** fix round 1（task-4.4-codex.md [medium]）：頁腳的計算與寫入抽成獨立函式，讓計時器／
   * visibilitychange 在 `todayIso` 沒變時也能單獨重算頁腳，不用整份 `render()`（見檔頭
   * 「fix round 1」說明）。自己重做 `render()` 開頭那兩個早退分支的判斷條件——沒資料／
   * 資料格式不對時回傳空字串，等同原本各分支手動寫的 `foot.textContent = ''`。 */
  function computeFooterHtml(snapshot) {
    if (!snapshot || snapshot.status !== 'ok' || !snapshot.data) {
      return '';
    }
    const data = snapshot.data;
    if (!Array.isArray(data.events)) {
      return '';
    }
    const fetchedTs = data.fetched || data.updated; // fetched＝最後一次真的抓到新資料；舊資料檔無此欄退回 updated
    let footHtml = `資料更新：${common.esc(fetchedTs || '—')}`;
    if (fetchedTs) {
      const age = (Date.now() - new Date(fetchedTs.replace(' ', 'T'))) / common.DAY;
      if (age > 3) {
        footHtml += `　<span class="warn">⚠ 已 ${Math.floor(age)} 天未更新，請檢查工作排程</span>`;
      }
    }
    if (data.errors && data.errors.length) {
      footHtml += `　<span class="warn">⚠ ${data.errors.length} 個來源異常</span>`;
    }
    return footHtml;
  }

  function renderFooter(snapshot) {
    foot.innerHTML = computeFooterHtml(snapshot);
  }

  /** 原樣搬自 finance-calendar.html 615–689 行 `renderDyn`，只把 `data` 參數換成完整
   * snapshot（見檔頭「尚無資料」說明）。 */
  function render(snapshot) {
    lastSnapshot = snapshot;
    lastRenderedTodayIso = common.isoDate(common.todayBase());
    list.innerHTML = '';
    renderFooter(snapshot); // fix round 1：獨立呼叫，涵蓋下面兩個早退分支，不用各自清空頁腳。

    if (!snapshot || snapshot.status !== 'ok' || !snapshot.data) {
      // widget-data-feed spec「尚無資料」：通道從未取得有效快照時顯示提示（含目前的資料
      // 目錄路徑），不是空白——這是新架構特有的資料流狀態，訊息文字刻意與下面
      // `!Array.isArray(data.events)` 那個沿用 Lively 版原文字的分支不同（見檔頭說明）。
      const dataDir =
        currentSettings && currentSettings.data_dir ? String(currentSettings.data_dir) : '（尚未取得設定）';
      list.innerHTML = `<div class="empty">尚無資料 —— 資料目錄：${common.esc(dataDir)}</div>`;
      return;
    }

    const data = snapshot.data;
    if (!Array.isArray(data.events)) {
      // Lively 版 615–622 行原始分支：理論上通過後端驗證的 'ok' 快照不會走到這裡，保留是
      // 防禦性 fallback、逐字沿用原文字。
      list.innerHTML = '<div class="empty">讀不到 tw_events 資料 —— 請先執行 update_tw_events.py</div>';
      return;
    }

    const holidays = new Set(Array.isArray(data.holidays) ? data.holidays : []);
    const today = common.todayBase();
    const todayIso = common.isoDate(today);
    const target = common.targetDay(holidays);
    const targetIso = common.isoDate(target);
    const isToday = target.getTime() === today.getTime();
    sub.textContent = isToday ? `今日 ${common.mdw(target)}` : `休市，順延 ${common.mdw(target)}`;

    // ── 上節：預告清單（dynTypes 中 punish 以外的類型；窗口內今天起全列）──
    const dynTypes = dynTypesFor(currentSettings);
    const upTypes = dynTypes.filter((t) => t !== 'punish');
    if (upTypes.length) {
      const label = upTypes.map((t) => TYPE_META[t].chip).join('・');
      list.insertAdjacentHTML('beforeend', `<div class="dayhead">${label}｜近兩週</div>`);
      const rows = data.events
        .filter((e) => upTypes.includes(e.type) && e.date >= todayIso) // ISO 字串比較即可
        .sort(
          (a, b) =>
            a.date.localeCompare(b.date) ||
            dynTypes.indexOf(a.type) - dynTypes.indexOf(b.type) ||
            (a.code || '').localeCompare(b.code || ''),
        );
      if (!rows.length) {
        list.insertAdjacentHTML('beforeend', `<div class="empty">近兩週無${label}</div>`);
      }
      for (const e of rows) {
        let note = (e.note || '').replace(/^除[權息]+\s*/, '');
        if (note === TYPE_META[e.type].chip) {
          note = ''; // 來源欄位與 chip 同字（如「法說會」）不重述
        }
        list.insertAdjacentHTML(
          'beforeend',
          `<li class="ev">
             <span class="d">${common.mdw(new Date(`${e.date}T00:00:00`))}</span>
             <span class="chip" style="--c:${TYPE_META[e.type].cls}">${TYPE_META[e.type].chip}</span>
             <b>${common.esc(e.code)} ${common.esc(e.name)}</b>
             ${e.date === todayIso ? '<span class="badge">今天</span>' : ''}
             <span class="n">${common.esc(note)}</span>
           </li>`,
        );
      }
    }

    // ── 下節：處置股（當日制：目標日落在處置期間內全列）──
    if (dynTypes.includes('punish')) {
      list.insertAdjacentHTML(
        'beforeend',
        `<div class="dayhead">處置股｜${isToday ? '今日' : `順延 ${common.md(target)}`}</div>`,
      );
      const rows = (Array.isArray(data.punish) ? data.punish : []) // 缺鍵（舊資料）視為空陣列
        .filter((p) => p.start <= targetIso && targetIso <= p.end)
        .sort((a, b) => (a.code || '').localeCompare(b.code || ''));
      if (!rows.length) {
        list.insertAdjacentHTML('beforeend', '<div class="empty">當日無處置股</div>');
      }
      for (const e of rows) {
        const timesPrefix = e.times >= 2 ? `第${e.times}次・` : '';
        const marketPrefix = e.market === '上櫃' ? '櫃・' : '';
        list.insertAdjacentHTML(
          'beforeend',
          `<li class="ev">
             <span class="chip" style="--c:${TYPE_META.punish.cls}">${TYPE_META.punish.chip}</span>
             <b>${common.esc(e.code)} ${common.esc(e.name)}</b>
             <span class="n">${common.esc(`${marketPrefix}${timesPrefix}至 ${common.md(new Date(`${e.end}T00:00:00`))}`)}</span>
           </li>`,
        );
      }
    }
    // 頁腳（更新時間＋過期／錯誤提醒）已在函式開頭由 renderFooter(snapshot) 算好寫入，
    // 這裡不用再重複一次（fix round 1）。
  }

  render(ctx.snapshot);

  ctx.onData((snap) => render(snap));
  ctx.onSettings((newSettings) => {
    currentSettings = newSettings;
    // task 5.2：透明度／主題色即時套用，與「顯示除權息」改動走同一筆 settings 事件。
    common.applyAppearance(newSettings);
    // 設定變更（例如「顯示除權息」開關）是使用者的明確動作，不是週期性雜訊，直接整份重建；
    // 與下面「週期重算不可打斷捲動位置」的顧慮（針對無條件的定時觸發）是兩回事。
    render(lastSnapshot);
  });

  // 跨日：見檔頭「跨日重算」說明——今天的日期沒變就不整份重建（不清空清單、不動
  // scrollTop），變了才整份重建（render() 內部已經含 renderFooter，不用另外呼叫）。
  //
  // fix round 1（task-4.4-codex.md [medium]）：today 沒變的分支改呼叫 renderFooter，讓
  // 「已 N 天未更新」門檻能在同一天之內被跨過時照樣更新，不必等到隔天 todayIso 改變。
  const timer = setInterval(() => {
    const todayIso = common.isoDate(common.todayBase());
    if (todayIso !== lastRenderedTodayIso) {
      render(lastSnapshot);
    } else {
      renderFooter(lastSnapshot);
    }
  }, 60 * 1000);
  const onVisible = () => {
    if (!document.hidden) {
      const todayIso = common.isoDate(common.todayBase());
      if (todayIso !== lastRenderedTodayIso) {
        render(lastSnapshot);
      } else {
        renderFooter(lastSnapshot);
      }
    }
  };
  document.addEventListener('visibilitychange', onVisible);

  return () => {
    clearInterval(timer);
    document.removeEventListener('visibilitychange', onVisible);
  };
}
