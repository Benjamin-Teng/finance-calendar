// host/ui/bridge.js
//
// 核心↔小工具頁面 IPC 封裝（design.md D4，介面本體見 host/src/widgets.rs 與
// .superpowers/sdd/tasks/task-2.7-report.md）。有 Tauri（`window.__TAURI__` 存在）時走真正的
// invoke／事件；沒有 Tauri（本機 HTTP 伺服器＋瀏覽器直接開頁，D11 對照測試與本 task 的驗收方式）
// 時改讀 fixture，讓頁面在瀏覽器裡也能顯示東西。
//
// ## `data` 不是 Tauri 事件（fix round 1，Codex 2.7；見 host/src/widgets.rs `DataSubscribers`）
// Tauri 事件的過濾依「監聽者自己宣告的 target」，裸 `event.listen` 的 target＝Any 會被一律
// 放行，核心無法保證「只送給訂閱該通道的視窗」。因此資料推播改走 `tauri::ipc::Channel`：
// `listen('data', handler)` 在內部建立 `new window.__TAURI__.core.Channel(...)`，呼叫
// `subscribe_data({ onData })`；核心依**呼叫端 webview 的 label** 決定通道（頁面不傳、也不能
// 選通道），之後只對這個 webview 投遞。handler 收到的仍是 `{ payload: { channel, generation, snapshot } }`，
// 與舊事件的 `event.payload` 同形，呼叫端（widget.html）不需改。其他頁面即使自己用裸
// `window.__TAURI__.event.listen('data', ...)` 也收不到任何東西（核心不再 emit `data` 事件）。
// `settings`／`edit-mode`／`pause` 仍是廣播事件，走 `getCurrentWebviewWindow().listen(...)`。
//
// ## 頁面啟動順序
// design.md D4：小工具開啟或重新載入時 SHALL 能立即取得當前快照，且不能漏接事件——順序是
// 「先掛好全部監聽器，才呼叫 get_snapshot／get_settings」（掛監聽器與 emit 之間有空窗會漏接，
// Tauri 事件不緩衝）。這個順序由呼叫端（widget.html）負責遵守；本檔只提供各自獨立的函式。

/** 是否在 Tauri webview 內執行；`window.__TAURI__` 只有 Tauri 的 webview 才會注入
 *（tauri.conf.json 的 `app.withGlobalTauri: true`）。 */
export const hasTauri = typeof window !== 'undefined' && !!window.__TAURI__;

let cachedWindow = null;

/** 無 Tauri（fixture）環境下的事件監聽者：event 名稱 → handler 集合（task 4.5，見
 * `listen()` 與檔尾 `window.__bridgeTest`）。 */
const fixtureListeners = new Map();

/** 目前視窗的 `WebviewWindow` 控制代碼（僅 Tauri 環境下有意義）。 */
function currentWindow() {
  if (!hasTauri) {
    return null;
  }
  if (!cachedWindow) {
    cachedWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
  }
  return cachedWindow;
}

/**
 * 呼叫 Rust 端指令（`get_snapshot`／`get_settings`／`update_settings`／`set_edit_mode`／
 * `report_content`，design.md D4）。無 Tauri 環境時呼叫端不應直接用這個函式——請改用下面的
 * 高階函式（`getSnapshot` 等），它們各自知道 fixture 模式該怎麼退回。
 */
async function invoke(cmd, args) {
  return window.__TAURI__.core.invoke(cmd, args);
}

/**
 * 掛監聽器（`data`／`settings`／`edit-mode`／`pause`，design.md D4）。`data` 走
 * `subscribe_data` 的 Channel（見檔頭註解），其餘走 `getCurrentWebviewWindow().listen(...)`。
 * 回傳的 Promise resolve 時登記已完成，之後的推播不會漏接（D4「先 listen 再 get」）。
 *
 * 無 Tauri 環境時沒有真正的事件來源，改登記進下面的 `fixtureListeners`（task 4.5：`pause`
 * 事件——行情條跑馬燈暫停/恢復——沒有對應的 fixture 檔可讀，必須靠測試腳本主動模擬事件，
 * 見檔尾 `window.__bridgeTest.emit()`）；handler 一律收到 `{ event, payload }`，與 Tauri
 * 環境下 `getCurrentWebviewWindow().listen()` 回呼的形狀一致，呼叫端（widget.html）不需要
 * 為兩種環境分別處理。
 *
 * @returns {Promise<() => void>} 取消訂閱函式。
 */
export async function listen(event, handler) {
  if (!hasTauri) {
    if (!fixtureListeners.has(event)) {
      fixtureListeners.set(event, new Set());
    }
    fixtureListeners.get(event).add(handler);
    return () => {
      fixtureListeners.get(event)?.delete(handler);
    };
  }
  if (event === 'data') {
    return subscribeData(handler);
  }
  return currentWindow().listen(event, handler);
}

/** `subscribe_data(onData)`：核心依本 webview 的 label 決定通道；本視窗不是訂閱通道的小工具
 * 時 invoke 會 reject（呼叫端的 `await listen('data', ...)` 隨之拋錯）。Channel 無法從前端
 * 取消登記，取消訂閱函式只停止轉交給 handler；頁面重新載入時會以新 Channel 重新訂閱、取代舊的。 */
async function subscribeData(handler) {
  let active = true;
  const onData = new window.__TAURI__.core.Channel((payload) => {
    if (active) {
      handler({ event: 'data', payload });
    }
  });
  await invoke('subscribe_data', { onData });
  return () => {
    active = false;
  };
}

// ── fixture 模式（design.md D11；無 Tauri 時） ──────────────────────────────────────
//
// fixture 檔預設放在 widget.html 同目錄下的 `fixtures/`（host/ui/fixtures/，本 task 已放
// `tw-events.json`＋`settings.json`＋`custom1.json` 範例；`tw-events.json` 檔名刻意用連字號
// 不是底線——repo 根目錄 `.gitignore` 排除了 `tw_events.json`／`tw_events.js`（Lively 版排程
// 自動產生的資料檔，不追蹤），若沿用同名會讓這份「刻意提交的固定測試資料」被一併忽略）；
// 可用 `?fixtures=<相對或絕對路徑>`
// query 參數覆寫成別的根目錄（例如 4.2 對照測試若改把比對用 fixture 放在
// host/tests/compare/fixtures/，不需要改這個檔，開頁時帶參數即可）。
//
// host/ui/ 會被 tauri.conf.json 的 `build.frontendDist` 整個嵌入 exe——`fixtures/` 這幾個
// 小檔案（tw_events.json 約 17KB）因此也會跟著出貨，這是本 task 評估過的取捨：fixture 只在
// `hasTauri === false` 時才會被讀取，而正式安裝的宿主必定跑在 Tauri webview 內
// （`window.__TAURI__` 恆為真），讀 fixture 這條路徑在正式版是永遠不會被執行的死碼，多出的
// 體積是純資料（無金鑰、無 PII，本來就是要公開顯示的財經行事曆），可接受。若之後 fixture
// 數量或體積明顯增加，可把預設 fixture 根目錄改指到 build 時不會被打包的位置（例如宿主改用
// 真正的 bundler 才擋得掉「整個目錄照抄」），或要求呼叫端一律帶 `?fixtures=` 指到
// `host/tests/` 下、把 `host/ui/fixtures/` 整個移除；目前檔案量小，先不做這個變更。

const DEFAULT_FIXTURE_BASE = 'fixtures/';

/** 通道名 → fixture 檔名。財經五個小工具共用 `tw-events` 通道（見 host/src/widgets.rs
 * `WIDGET_CHANNELS`），`customN` 通道各自一個檔。`tw-events` 的檔名用連字號（不是官方資料檔
 * 慣用的底線 `tw_events.json`）：見上方 fixture 根目錄註解，底線檔名會被 repo 根 `.gitignore`
 * 忽略。 */
const CHANNEL_FIXTURE_FILES = {
  'tw-events': 'tw-events.json',
  custom1: 'custom1.json',
  custom2: 'custom2.json',
  custom3: 'custom3.json',
  custom4: 'custom4.json',
  custom5: 'custom5.json',
};

function fixtureBase() {
  const params = new URLSearchParams(window.location.search);
  const override = params.get('fixtures');
  if (!override) {
    return DEFAULT_FIXTURE_BASE;
  }
  return override.endsWith('/') ? override : `${override}/`;
}

function emptySnapshot(channel) {
  return { channel, status: 'empty', data: null, meta: null };
}

async function fetchFixtureSnapshot(channel) {
  const file = CHANNEL_FIXTURE_FILES[channel];
  if (!file) {
    return emptySnapshot(channel);
  }
  try {
    const res = await fetch(`${fixtureBase()}${file}`, { cache: 'no-store' });
    if (!res.ok) {
      return emptySnapshot(channel);
    }
    const data = await res.json();
    return {
      channel,
      status: 'ok',
      data,
      // fixture 沒有真正的 loadedAt（不是排程輪詢寫入的），用讀取當下時間；fetched／updated
      // 直接沿用 fixture 內容本身的欄位（tw_events.json 頂層就有這兩個鍵，D4 SnapshotMeta 的
      // camelCase 命名一致）。
      meta: {
        loadedAt: Date.now(),
        fetched: data?.fetched ?? null,
        updated: data?.updated ?? null,
      },
    };
  } catch {
    return emptySnapshot(channel);
  }
}

let cachedFixtureSettings = null;

async function fetchFixtureSettings() {
  if (cachedFixtureSettings) {
    return cachedFixtureSettings;
  }
  try {
    const res = await fetch(`${fixtureBase()}settings.json`, { cache: 'no-store' });
    if (res.ok) {
      // task 4.8：`__bridgeTest.settingsOverride` 覆寫部分欄位（例如截圖時預先選好某個桌布主題）。
      cachedFixtureSettings = { ...(await res.json()), ...(window.__bridgeTest?.settingsOverride ?? {}) };
      return cachedFixtureSettings;
    }
  } catch {
    // 忽略，退回下面的最小預設值。
  }
  // fixtures/settings.json 讀不到時的最小退回值——欄位形狀對照 host/src/settings.rs
  // 的 `Settings`（snake_case，該 struct 沒有 `rename_all`），只給小工具渲染會用到的欄位。
  cachedFixtureSettings = {
    show_dividend: false,
    opacity: 0.55,
    accent_color: '#e0aa54',
  };
  return cachedFixtureSettings;
}

// ── 高階 API（design.md D4 五個指令＋四個事件的封裝） ────────────────────────────────

/** `get_snapshot(channel)`（design.md D4）。 */
export async function getSnapshot(channel) {
  if (hasTauri) {
    return invoke('get_snapshot', { channel });
  }
  return fetchFixtureSnapshot(channel);
}

/** `get_settings()`（design.md D4）。 */
export async function getSettings() {
  if (hasTauri) {
    return invoke('get_settings');
  }
  return fetchFixtureSettings();
}

/** `get_pause() -> { paused, reason }`（design.md D4；task 5.6 fix round 1）：頁面啟動／重新
 * 載入時查詢宿主目前的暫停狀態（`pause` 事件只在狀態變化時廣播，重新載入的頁面不查就不知道
 * 目前暫停中）。呼叫端須先 `listen('pause', ...)` 再查，且查詢期間收到的事件優先（見
 * widget.html）。查詢失敗回傳 `null`（呼叫端視為「沒有初始狀態」，不影響掛載）。fixture 模式
 * 沒有宿主，回傳未暫停；測試要模擬暫停改用 `window.__bridgeTest.emit('pause', ...)`。 */
export async function getPause() {
  if (hasTauri) {
    try {
      return await invoke('get_pause');
    } catch {
      return null;
    }
  }
  return { paused: false, reason: null };
}

/** `get_edit_mode() -> bool`（fix F1，review 7.3 M3；比照 `getPause`）：頁面啟動／重新載入時
 * 查詢宿主目前是否在編輯版面（`edit-mode` 事件只在切換時廣播一次）。呼叫端須先
 * `listen('edit-mode', ...)` 再查，且查詢期間收到的事件優先（見 widget.html）。查詢失敗回傳
 * `null`（視為沒有初始狀態）。fixture 模式沒有宿主，回傳 `false`。 */
export async function getEditMode() {
  if (hasTauri) {
    try {
      return await invoke('get_edit_mode');
    } catch {
      return null;
    }
  }
  return false;
}

/** `update_settings(patch)`（design.md D4）。fixture 模式沒有宿主可寫回，只回傳合併後的
 * 記憶體副本，讓呼叫端（例如未來的設定視窗）在瀏覽器裡也能看到即時效果，但不持久化。
 *
 * task 7.4：`window.__bridgeTest.forceUpdateSettingsError`（僅 fixture 模式）——設定視窗
 * 「開啟小工具找空位找不到空位」的失敗路徑（`Err("空間不足，請先調整版面")`）需要真的讓
 * `update_settings` 回絕才能驗證 settings.html 顯示錯誤訊息並把開關還原，但 fixture 模式
 * 沒有 Rust 端可以真的判斷空間夠不夠。設這個掛鉤讓對照測試腳本（`host/tests/compare/`）
 * 能在不改動 settings.html 本身邏輯的前提下，讓下一次（且僅下一次）`updateSettings()` 拒絕
 * 並拋出指定的錯誤字串——與 `emit`／`lastReportContent` 同一套「只在 `!hasTauri` 時存在」的
 * 測試旁路慣例（見下方「測試掛鉤」一節）。用後即清除，避免影響同一頁面後續的其他操作。 */
export async function updateSettings(patch) {
  if (hasTauri) {
    return invoke('update_settings', { patch });
  }
  if (typeof window !== 'undefined' && window.__bridgeTest?.forceUpdateSettingsError) {
    const message = window.__bridgeTest.forceUpdateSettingsError;
    window.__bridgeTest.forceUpdateSettingsError = null;
    throw message;
  }
  const current = await fetchFixtureSettings();
  cachedFixtureSettings = { ...current, ...patch };
  return cachedFixtureSettings;
}

// ── 動態桌布（dynamic-wallpaper task 4.8；host/src/wallpaper_settings.rs，只給設定視窗） ──────────

/** fixture 模式的預設桌布狀態（協調迴圈在跑、主題「不接管」、沒有任何提示）。測試以
 * `window.__bridgeTestInit.wallpaperStatus`（載入前注入）或 `window.__bridgeTest.wallpaperStatus`
 * 覆寫部分欄位。欄位形狀對照 `WallpaperStatusView`。 */
const FIXTURE_WALLPAPER_STATUS = {
  coordinator_running: true,
  state: 'idle',
  waiting_for: null,
  holding_without: null,
  blocked: null,
  awaiting_spotlight_confirmation: false,
  spotlight_check_needed: false,
  taken_over: false,
  config_version: 0,
};

/** `get_wallpaper_status()`：協調迴圈狀態快照（不等協調迴圈）。 */
export async function getWallpaperStatus() {
  if (hasTauri) {
    return invoke('get_wallpaper_status');
  }
  return { ...FIXTURE_WALLPAPER_STATUS, ...(window.__bridgeTest?.wallpaperStatus ?? {}) };
}

/** `get_wallpaper_config()`：主題設定檔（Rust 頂層合併後）與路徑。fixture 模式沒有宿主，
 * `config` 為 `null`（頁面改用內建預設），可由 `__bridgeTest.wallpaperConfig` 覆寫。 */
export async function getWallpaperConfig() {
  if (hasTauri) {
    return invoke('get_wallpaper_config');
  }
  return (
    window.__bridgeTest?.wallpaperConfig ?? {
      path: 'C:/Users/example/AppData/Roaming/tw.fintools.fc-host/wallpaper-config.json',
      config: null,
    }
  );
}

// ── 抓取狀態（data-layer-rust task 5.5；host/src/fetch/status.rs，只給設定視窗） ───────────────────

/** fixture 模式的預設抓取狀態（啟用中、上一輪順利完成）。測試以 `window.__bridgeTestInit.fetchStatus`
 * （載入前注入）覆寫整份；`fetchStatusError`（字串）讓查詢拋錯。欄位形狀對照 `FetchStatusView`。 */
const FIXTURE_FETCH_STATUS = {
  kind: 'active',
  last_start: '2026-10-05 15:02',
  last_finish: '2026-10-05 15:03',
  source_failed: false,
  isolation: null,
};

/** `get_fetch_status()`：抓取開關判定（啟用／設定關閉／隔離環境）、上一輪時間與是否有來源失敗。
 * 設定視窗開啟時查一次。查詢失敗時 reject（呼叫端顯示「查詢失敗」）。 */
export async function getFetchStatus() {
  if (hasTauri) {
    return invoke('get_fetch_status');
  }
  if (window.__bridgeTest?.fetchStatusError) {
    throw new Error(window.__bridgeTest.fetchStatusError);
  }
  return window.__bridgeTest?.fetchStatus ?? FIXTURE_FETCH_STATUS;
}

/** `select_wallpaper_theme(theme, spotlightAnswer)` → `{ outcome, settings }`。`spotlightAnswer`：
 * `null`＝還沒問、`true`＝確認、`false`＝取消。fixture 模式照 Rust 端同一條規則模擬（要問時不存檔），
 * 並把收到的答案記在 `__bridgeTest.lastThemeSelection` 供測試檢查。 */
export async function selectWallpaperTheme(theme, spotlightAnswer) {
  if (hasTauri) {
    return invoke('select_wallpaper_theme', { theme, spotlightAnswer });
  }
  if (window.__bridgeTest) {
    window.__bridgeTest.lastThemeSelection = { theme, spotlightAnswer };
  }
  const status = await getWallpaperStatus();
  if (theme !== 'none') {
    if (spotlightAnswer === false) {
      // 修正輪 1：尚未接管且主題不是「不接管」時存成「不接管」（同 Rust 端 select_theme_core）。
      const settings = await fetchFixtureSettings();
      if (!status.taken_over && (settings.wallpaper_theme ?? 'none') !== 'none') {
        return { outcome: 'cancelled', settings: await updateSettings({ wallpaper_theme: 'none' }) };
      }
      return { outcome: 'cancelled', settings };
    }
    if (spotlightAnswer == null && status.spotlight_check_needed) {
      return { outcome: 'needs_spotlight_confirmation', settings: await fetchFixtureSettings() };
    }
  }
  const settings = await updateSettings({ wallpaper_theme: theme });
  return { outcome: 'applied', settings };
}

/** `respond_spotlight_confirmation(confirmed)`：協調迴圈等焦點確認時的答案。fixture 模式只記錄在
 * `__bridgeTest.lastSpotlightResponse`。 */
export async function respondSpotlightConfirmation(confirmed) {
  if (hasTauri) {
    return invoke('respond_spotlight_confirmation', { confirmed });
  }
  if (window.__bridgeTest) {
    window.__bridgeTest.lastSpotlightResponse = confirmed;
  }
  return undefined;
}

/** `set_edit_mode(enabled)`（design.md D4）。fixture 模式下無事可做（沒有其他視窗可廣播）。 */
export async function setEditMode(enabled) {
  if (hasTauri) {
    return invoke('set_edit_mode', { enabled });
  }
  return undefined;
}

/** task 7.6：編輯版面調整大小把手按下時呼叫 `getCurrentWebviewWindow().startResizeDragging(方向)`
 *（`core:window:allow-start-resize-dragging`，design.md D3／D7；方向字串是 tauri-runtime 2.12
 * `ResizeDirection` 的變體名：East、North、NorthEast、NorthWest、South、SouthEast、SouthWest、
 * West）。fixture 模式下沒有視窗可調整，靜默略過。 */
export async function startResizeDragging(direction) {
  if (hasTauri) {
    return currentWindow().startResizeDragging(direction);
  }
  return undefined;
}

/** `report_content(hasContent)`（design.md D4／D7；task 7.3 取代 `report_size(width, height)`：
 * 頁面自己判斷「內容高度為 0」的舊條件後只送一個布林值，核心不再需要自己解讀尺寸）。fixture
 * 模式下無宿主可回報，靜默略過，但仍記錄在 `window.__bridgeTest.lastReportContent`（task 4.5：
 * 驗證「無資料時內容高度為 0」需要一個不靠猜測 `getBoundingClientRect()` 的直接證據點，見檔尾
 * 說明；task 7.3 把記錄的形狀從 `{width,height}` 改成布林值本身）。 */
export async function reportContent(hasContent) {
  if (hasTauri) {
    return invoke('report_content', { hasContent });
  }
  if (typeof window !== 'undefined' && window.__bridgeTest) {
    window.__bridgeTest.lastReportContent = hasContent;
  }
  return undefined;
}

// ── 測試掛鉤（design.md D11；task-4.5-brief.md）───────────────────────────────────
//
// 無 Tauri 的 fixture 模式沒有真正的宿主可以送 `pause`／`settings`／`edit-mode` 事件，也沒有
// fixture 檔能表示「宿主通知暫停」這種瞬間動作（不像 `data`／`get_settings` 有對應的 JSON
// 檔可讀）。task 4.5（行情條跑馬燈暫停/恢復）需要對照測試腳本能在瀏覽器裡主動模擬這類事件，
// 因此提供 `window.__bridgeTest.emit(event, payload)`：直接呼叫 `listen()` 登記在
// `fixtureListeners` 裡的 handler，payload 形狀與 Tauri 環境的事件回呼一致
// （`{ event, payload }`，見上面 `listen()` 的說明）。
//
// **只在 `!hasTauri` 時建立**：正式安裝的宿主一定跑在 Tauri webview 內（`hasTauri` 恆為
// 真），`window.__bridgeTest` 不會出現在生產環境，純粹是測試用的旁路。
if (!hasTauri && typeof window !== 'undefined') {
  window.__bridgeTest = {
    /** 觸發一個 fixture 模式的事件；沒有任何 handler 在聽時靜默略過。 */
    emit(event, payload) {
      const handlers = fixtureListeners.get(event);
      if (!handlers) {
        return;
      }
      for (const handler of handlers) {
        handler({ event, payload });
      }
    },
    // `reportContent()` 每次呼叫都會覆寫這個欄位，供測試腳本讀取最後一次回報的有無內容
    // （task 7.3：`null`＝尚未回報過，`true`／`false`＝最後一次回報的值，不能用 truthy 判斷
    // 「有沒有回報過」）。
    lastReportContent: null,
    // task 7.4：測試腳本設成非空字串時，下一次（且僅下一次）`updateSettings()` 會拋出這個
    // 字串並清空自己（見 `updateSettings` 文件）；`null`＝正常走 fixture 合併路徑。
    forceUpdateSettingsError: null,
    // task 4.8（動態桌布）：`wallpaperStatus`／`wallpaperConfig` 覆寫 fixture 的桌布狀態與設定檔，
    // `today`（'YYYY-MM-DD'）覆寫設定視窗判定休市表用的本機日期；`lastThemeSelection`／
    // `lastSpotlightResponse` 記錄頁面送出的選擇與答案。
    wallpaperStatus: null,
    wallpaperConfig: null,
    // task 5.5：覆寫 fixture 的抓取狀態整份／讓查詢拋錯。
    fetchStatus: null,
    fetchStatusError: null,
    settingsOverride: null,
    today: null,
    lastThemeSelection: null,
    lastSpotlightResponse: null,
    // 頁面載入前以 CDP `Page.addScriptToEvaluateOnNewDocument` 設定的初始值（截圖腳本用）。
    ...(window.__bridgeTestInit ?? {}),
  };
}
