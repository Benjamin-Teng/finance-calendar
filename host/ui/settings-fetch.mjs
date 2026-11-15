// host/ui/settings-fetch.mjs
//
// 設定視窗底部「資料來源與抓取狀態」區塊的純邏輯（data-layer-rust task 5.5、design.md D12；spec
// market-data-fetch「標示資料來源與抓取狀態」）。ES module、不依賴 DOM，可在 Node 測試
// （host/tests/settings-fetch.test.mjs）；畫面與 IPC 在 settings.html。
//
// 狀態來自 `get_fetch_status`（host/src/fetch/status.rs `FetchStatusView`）：
//   { kind: 'active' | 'off_by_setting' | 'off_isolated',
//     last_start, last_finish: 本機時間 'YYYY-MM-DD HH:MM' 或 null,
//     source_failed: boolean, round_failed: boolean（整輪沒有任何來源成功，或寫檔失敗）, settings_path: string,
//     isolation: { env_value, registered } | null }
//
// 「立即更新資料」按鈕（fetch-progress-and-manual-refresh design.md D5）另外吃抓取活動狀態
// `fetch-activity`／`get_fetch_activity` 的 `{ running, enabled, last }`（bridge.js 已正規化，未知＝null），
// 與 `request_fetch_now` 的回覆 `{ outcome: 'started' | 'already_running' | 'disabled' }`。

/** 政府資料開放授權條款第 1 版的條款網址（設定視窗沒有開外部連結的作法，只顯示網址文字）。 */
export const LICENSE_URL = 'https://data.gov.tw/license';

/** 資料來源清單（順序照 spec）。`license`＝該來源另外標示的授權文字。 */
export const DATA_SOURCES = [
  { name: '臺灣證券交易所', license: '依政府資料開放授權條款第 1 版' },
  { name: '證券櫃檯買賣中心', license: '依政府資料開放授權條款第 1 版' },
  { name: '公開資訊觀測站' },
  { name: 'ForexFactory' },
  { name: 'Yahoo Finance' },
  { name: '紐約聯邦準備銀行' },
];

/** 停用時顯示的原因（spec：明確顯示「未抓取」與原因、隔離環境說明如何改為 "on"）。 */
export const NOT_FETCHED_OFF = '未抓取：已在設定關閉';
export const NOT_FETCHED_ISOLATED =
  '未抓取：偵測到隔離環境（LOCALAPPDATA 與系統登記不同），可在設定檔將 data_fetch 設為 "on"，改完後重新啟動宿主';

/** 查詢本身失敗（IPC 錯誤）時的文字。 */
export const STATUS_QUERY_FAILED = '抓取狀態：查詢失敗';

/** 未抓取狀態下補充「最後一次更新」（有完成時間才有）。 */
function lastUpdateDetail(status) {
  return status.last_finish ? [`最後一次更新：${status.last_finish}`] : [];
}

/**
 * 把 `get_fetch_status` 的回應轉成要顯示的內容。
 * @returns {{ text: string, detail: string[], tone: 'ok' | 'warn' | 'off' }}
 *   `text`＝主要一行；`detail`＝補充行（未抓取時依序為：最後一次更新（有完成時間才有）、隔離時的
 *   LOCALAPPDATA／系統登記兩個路徑與設定檔路徑）；`tone`＝顏色語意
 *   （`ok` 正常、`warn` 部分來源失敗或尚無紀錄、`off` 未抓取）。
 */
export function fetchStatusDisplay(status) {
  if (!status || typeof status !== 'object') {
    return { text: STATUS_QUERY_FAILED, detail: [], tone: 'warn' };
  }
  switch (status.kind) {
    case 'off_by_setting':
      return { text: NOT_FETCHED_OFF, detail: lastUpdateDetail(status), tone: 'off' };
    case 'off_isolated': {
      // 未抓取時仍顯示最後一次更新（若有），使用者才看得出資料已經舊了多久。
      const detail = lastUpdateDetail(status);
      if (status.isolation) {
        detail.push(`LOCALAPPDATA：${status.isolation.env_value}`);
        detail.push(`系統登記：${status.isolation.registered}`);
      }
      if (status.settings_path) {
        detail.push(`設定檔：${status.settings_path}`);
      }
      return { text: NOT_FETCHED_ISOLATED, detail, tone: 'off' };
    }
    case 'active': {
      const when = status.last_finish ?? null;
      if (when === null) {
        // 啟用中但沒有完成過的一輪（剛安裝、紀錄損毀、或上一輪被中止）。
        return {
          text: status.last_start
            ? `上次更新：尚未完成（上一輪開始於 ${status.last_start}）`
            : '上次更新：尚無紀錄（等待第一輪抓取）',
          detail: [],
          tone: 'warn',
        };
      }
      const failed = status.source_failed === true || status.round_failed === true;
      const note = status.round_failed === true ? '（本輪未成功更新）' : '（部分來源失敗）';
      return {
        text: `上次更新：${when}${failed ? note : ''}`,
        detail: [],
        tone: failed ? 'warn' : 'ok',
      };
    }
    default:
      return { text: STATUS_QUERY_FAILED, detail: [], tone: 'warn' };
  }
}

// ── 立即更新資料（fetch-progress-and-manual-refresh design.md D5） ─────────────────────────────

export const REFRESH_LABEL = '立即更新資料';
export const REFRESH_RUNNING_LABEL = '更新中…';
/** 抓取進行中時加在既有狀態文字前面（D5：「既有文字前加『更新中…』」）。 */
export const RUNNING_PREFIX = '更新中…';
/** `request_fetch_now` 回 `disabled`：設定或隔離判定在按下前一刻改了，頁面上的狀態還沒跟上。 */
export const REFRESH_DISABLED_NOTE = '抓取未啟用，無法立即更新';

const STATUS_KINDS = ['active', 'off_by_setting', 'off_isolated'];

/**
 * 抓取是否啟用（決定按鈕能不能按）。抓取狀態認得時以它的 `kind` 為準：它與 `request_fetch_now` 用同一個
 * 閘門、依當下設定現算，也正是「未抓取」原因文字的來源，按鈕與原因因此一致；活動狀態的 `enabled`
 * 要等排程下一次醒來才更新（最多約 60 秒）。抓取狀態查不到時才看活動狀態；兩者都未知就讓使用者按，
 * 由宿主的回覆決定（停用時回 `disabled`，頁面顯示說明）。
 */
function fetchEnabled(status, activity) {
  if (status && STATUS_KINDS.includes(status.kind)) {
    return status.kind === 'active';
  }
  return activity ? activity.enabled : true;
}

/**
 * 抓取狀態區塊與按鈕要顯示的內容。
 * @param {object} args
 * @param {object | null} args.status  `get_fetch_status` 的回覆（查詢失敗時為 null）
 * @param {string | null} [args.statusError]  查詢失敗的錯誤文字（有值時主文字為「查詢失敗：…」）
 * @param {{ running: boolean, enabled: boolean, last: string } | null} [args.activity]  null＝未知
 * @param {boolean} [args.requesting]  `request_fetch_now` 還在途（避免連按送出多次）
 * @returns {{ text: string, detail: string[], tone: 'ok' | 'warn' | 'off',
 *             button: { text: string, disabled: boolean } }}
 */
export function fetchControlsDisplay({ status, statusError = null, activity = null, requesting = false }) {
  const base =
    statusError != null
      ? { text: `${STATUS_QUERY_FAILED}：${statusError}`, detail: [], tone: 'warn' }
      : fetchStatusDisplay(status);
  const running = activity?.running === true;
  const enabled = fetchEnabled(statusError != null ? null : status, activity);
  return {
    text: running ? `${RUNNING_PREFIX}${base.text}` : base.text,
    detail: base.detail,
    tone: base.tone,
    button: {
      text: running ? REFRESH_RUNNING_LABEL : REFRESH_LABEL,
      disabled: running || !enabled || requesting,
    },
  };
}

/**
 * `request_fetch_now` 回覆後要在狀態區顯示的一行說明；`null`＝不需要說明。`started`／`already_running`
 * 都靠隨後的 `fetch-activity` 事件更新畫面（D5）。
 */
export function refreshReplyNote(reply) {
  switch (reply?.outcome) {
    case 'started':
    case 'already_running':
      return null;
    case 'disabled':
      return REFRESH_DISABLED_NOTE;
    default:
      return '無法立即更新：宿主回覆無法辨識';
  }
}

/** `request_fetch_now` 被拒（沒有排程、正在因更新結束等；Tauri 以字串 reject）時的說明。 */
export function refreshErrorNote(err) {
  return `無法立即更新：${err?.message ?? err}`;
}
