// host/ui/wallpapers/lib/wallpaper.mjs
//
// 繪圖頁共用模組的 DOM 部分（task 3.1，design.md D2／D7／D8）。純邏輯（尺寸、亂數、時區、
// query 解析、payload 與設定合併、字型清單、版面檢查）在 `core.mjs`，可在 Node 測試；本檔需要瀏覽器。
//
// ## 頁面骨架與使用範例
//
//   <!-- host/ui/wallpapers/<主題>.html -->
//   <link rel="stylesheet" href="fonts/fonts.css">   <!-- 字型 @font-face，相對路徑、全部內建 -->
//   <canvas id="c"></canvas>
//   <script type="module">
//     import { runWallpaper } from './lib/wallpaper.mjs';
//     import { mulberry32, tzParts, textBox } from './lib/core.mjs';
//
//     const DEFAULTS = { thresholds: { shortRatio: 2 } };   // 頁面自己的內建預設（真正的預設檔見 task 3.2）
//
//     runWallpaper({
//       canvas: '#c',
//       draw(env) {                       // 可回傳 Promise；字型與資料都已就緒才會被呼叫
//         const { ctx, W, H, S, nowMs, tz, data, boxes } = env;
//         const cfg = env.withDefaults(DEFAULTS);     // config 缺鍵補預設、型別錯補預設＋警告
//         const rng = mulberry32(20261002);           // 決定性亂數
//         ctx.font = `600 ${Math.round(48 * S)}px "Cinzel"`;
//         ctx.fillText('NOW', W / 2, H / 2);
//         boxes.push(textBox(ctx, 'now', 'NOW', W / 2, H / 2));  // 版面檢查用：每個文字元素都要登記
//       },
//     });
//   </script>
//
// ## `runWallpaper(options)` 的流程（D2：全程只用 Promise 鏈，不用計時器與 requestAnimationFrame）
//   解析 query → 設定 canvas 尺寸 → `document.fonts.load` 全部字型（任一失敗＝整張不畫）→
//   讀資料與設定 → `draw(env)` → （宿主內）匯出 PNG 並回報宿主 → 設「畫完」訊號。
//
// ## 資料與設定（design.md D7）
//   通道 payload 形狀 `{ data, config }`：`data`＝從 tw_events 擷取的鍵，`config`＝主題設定
//   （內容由 task 3.2 定義，本模組不規定鍵）。細節與判別規則見 `core.mjs` 契約第 7 點。
//   - 宿主內：`getSnapshot('wallpaper')`（bridge.js），`snapshot.data` 就是上面的 payload。
//   - 獨立模式（無 Tauri）：讀 `fixture=` 指的檔；沒帶就讀 `host/ui/fixtures/tw-events.json`。
//     fixture 可以是 `{data, config}` 信封，或裸 tw_events JSON（視為 `{data, config:{}}`）。
//     讀不到、不是 JSON、不是物件 → 整張失敗（phase 'error'）。這個路徑不經 bridge.js 的 fixture
//     目錄機制（那個依頁面網址解析相對路徑，在 /wallpapers/ 底下會落空）。
//   - 宿主通道讀取失敗或尚無資料：不致命，`data=null`、`config={}`，主題畫缺資料畫面。
//
// ## env（傳給 draw 的唯一參數）
//   canvas, ctx   2D context，`{ alpha: false }`；canvas.width/height 已設為 W/H
//   W, H, S       輸出像素與尺寸基準 `S = min(W,H)/2160`
//   nowMs, tz     「現在」（UTC 毫秒；有 `t` 用 `t`，否則 Date.now()）與顯示時區（IANA）。
//                 主題要用台北日期（台股休市、法說會）時，自己呼叫 `tzParts('Asia/Taipei', nowMs)`，
//                 不要用顯示時區 `tz`。
//   params        parseQuery 的完整結果（含 warnings）
//   snapshot      `{ channel, status, data, config, meta, source }`；status 'ok' | 'empty' | 'error'
//   data, config  payload 的兩半（沒資料時 data 為 null、config 為 {}）
//   withDefaults(defaults)  `mergeThemeConfig(defaults, config)`（config-holidays.mjs）的包裝：`mergeConfig` 後再跑
//                 `validateThemeConfig`，回傳合併後設定；合併與巢狀驗證的警告經 `warn` 併入 state.warnings
//                 （同一則不重複）。task 4.1：宿主只做頂層鍵合併，巢狀設定錯誤只有這裡會發現並回報宿主
//   warn(message)  主題自己發現的設定問題（例：時段表的星期拼錯）併入 state.warnings；照常畫完、
//                 宿主內隨 meta.warnings 回報（task 3.3 加入）
//   boxes         陣列；主題把每個文字元素的 `{label,x,y,w,h}`（`textBox()` 產生）push 進來
//
// ## 「畫完」訊號（頁面內）
//   `window.__wallpaper`＝狀態物件，`phase` 只會從 'loading' 變成 'done' 或 'error'（一次）：
//     { phase, w, h, tz, nowMs, params, rid, warnings, fonts:[{id,ok,error,faces}], fontSources:[{url,local}],
//       dataStatus, dataSource, dataShape, dataKeys, configKeys, fixtureUrl, boxes, hostReport, error }
//   `warnings`＝query 與 payload／config 的全部警告（字串陣列）。
//   同時 `document.documentElement.dataset.wallpaper = phase`，並在 window 上派發
//   `CustomEvent('wallpaper-done', { detail: state })`（done 與 error 都會派發）。
//   截圖稿（CDP）讀這個狀態，並自己從 canvas 匯出（`canvas.toDataURL`），不要截整個視窗。
//
// ## 宿主接法（design.md D2；Rust 端由 task 4.5 實作，本模組只負責呼叫）
//   Tauri 的 `eval` 沒有回傳值，宿主讀不到頁面內的狀態，所以**頁面自己主動回報**（只在有 Tauri 時）：
//   - 成功（phase 'done'）：`invoke('wallpaper_render_done', pngBytes, { headers })`
//       請求本體（raw body）＝PNG 位元組（Uint8Array，canvas 匯出）；
//       header `x-wallpaper-meta`＝`encodeURIComponent(JSON.stringify(meta))`，
//       meta＝`{ w, h, tz, nowMs, warnings: string[], dataStatus, dataSource, rid }`。
//       `rid`＝網址 query 的 render id 原樣帶回（task 4.5：宿主丟棄 id 不符的回報，例如逾時後才晚到的 PNG）。
//   - 失敗（phase 'error'：字型失敗、fixture 讀不到、draw 拋錯、匯出失敗）：
//       `invoke('wallpaper_render_failed', { error: string, meta })`（meta 同上，w／h 為已解析的值）。
//   - **done 帶 warnings 也算成功**：照常送 PNG，警告放在 meta.warnings 讓宿主記錄，不視為失敗。
//     （例：`w` 不合法時頁面改畫預設 3840×2160，meta.w／h 會是實際畫的尺寸，宿主可自行核對。）
//   - 回報用的指令不存在（4.5 尚未實作）或 invoke 失敗：只 `console.warn`，phase 照常，
//     失敗原因記在 `state.hostReport.error`。
//   - 模組根本沒執行（import 404、語法錯誤）或頁面卡住：頁面沒有機會回報，宿主靠自己的渲染
//     逾時（design.md D2，30 秒）處理並保留舊圖。
//   回報發生在設定 `phase` 之前，所以看到 phase 最終值時回報已嘗試過。

import {
  FONT_REQUIREMENTS,
  cssFont,
  normalizePayload,
  parseQuery,
  readRenderId,
  sizeScale,
} from './core.mjs';
import { mergeThemeConfig } from './config-holidays.mjs';
import { getSnapshot, hasTauri } from '../../bridge.js';

/** 宿主通道名（design.md D7；Rust 端 `data.rs` 的 `WALLPAPER_CHANNEL`，task 4.6：只有桌布渲染視窗查得到）。 */
export const WALLPAPER_CHANNEL = 'wallpaper';

/** 獨立模式沒帶 `fixture=` 時讀的檔（與小工具頁共用的 host/ui/fixtures/tw-events.json）。 */
export const DEFAULT_FIXTURE_URL = new URL('../../fixtures/tw-events.json', import.meta.url).href;

/** 對外狀態物件（見檔頭「畫完訊號」）。 */
function initState() {
  const state = {
    phase: 'loading',
    w: 0,
    h: 0,
    tz: '',
    nowMs: 0,
    params: null,
    rid: null,
    warnings: [],
    fonts: [],
    fontSources: [],
    dataStatus: 'none',
    dataSource: 'none',
    dataShape: null,
    dataKeys: [],
    configKeys: [],
    fixtureUrl: null,
    boxes: [],
    hostReport: { sent: null, error: null },
    error: null,
  };
  window.__wallpaper = state;
  document.documentElement.dataset.wallpaper = 'loading';
  return state;
}

function finish(state, phase, error) {
  state.phase = phase;
  state.error = error ?? null;
  document.documentElement.dataset.wallpaper = phase;
  window.dispatchEvent(new CustomEvent('wallpaper-done', { detail: state }));
}

/**
 * 載入全部必要字型並逐項回報。`document.fonts.load` 在「家族沒有任何 @font-face」時 resolve 成
 * 空陣列（不會 reject），所以空陣列也算失敗；有 @font-face 但檔案載不到時它會 reject。
 * @returns {Promise<Array<{id:string, ok:boolean, error:string|null, faces:Array<{family:string,weight:string,status:string}>}>>}
 */
export async function loadFonts(requirements = FONT_REQUIREMENTS) {
  return Promise.all(
    requirements.map(async (req) => {
      const out = { id: req.id, ok: false, error: null, faces: [] };
      try {
        const faces = await document.fonts.load(cssFont(req));
        out.faces = faces.map((f) => ({ family: f.family, weight: f.weight, status: f.status }));
        if (faces.length === 0) {
          out.error = '沒有符合的 @font-face（fonts.css 沒載入或家族名不符）';
        } else if (!faces.every((f) => f.status === 'loaded')) {
          out.error = `字型狀態不是 loaded：${out.faces.map((f) => f.status).join(',')}`;
        } else {
          out.ok = true;
        }
      } catch (e) {
        out.error = `載入失敗：${e?.message || e}`;
      }
      return out;
    }),
  );
}

/** 本頁實際載入的字型檔來源（Resource Timing）；`local`＝與頁面同源（斷網時仍可用）。 */
export function fontSources() {
  return performance
    .getEntriesByType('resource')
    .filter((e) => /\.(?:ttf|otf|woff2?)(?:[?#]|$)/i.test(e.name))
    .map((e) => ({ url: e.name, local: e.name.startsWith(window.location.origin + '/') }));
}

async function fetchFixture(url) {
  let res;
  try {
    res = await fetch(url, { cache: 'no-store' });
  } catch (e) {
    throw new Error(`fixture 讀取失敗：${url}（${e?.message || e}）`);
  }
  if (!res.ok) {
    throw new Error(`fixture 讀取失敗：HTTP ${res.status} ${url}`);
  }
  try {
    return await res.json();
  } catch (e) {
    throw new Error(`fixture 不是合法 JSON：${url}（${e?.message || e}）`);
  }
}

/**
 * 讀資料與設定。fixture 路徑（有 `fixture=`，或無 Tauri 時的預設檔）：讀不到、不是 JSON、不是物件
 * 一律拋錯。宿主路徑：沿用 bridge.js 的 `getSnapshot('wallpaper')`；通道失敗不拋錯，回傳
 * status 'error'（主題畫缺資料畫面）。回傳 `{channel,status,data,config,shape,warnings,meta,source}`。
 */
export async function loadSnapshot(params) {
  const fixtureUrl = params.fixture
    ? new URL(params.fixture, window.location.href).href
    : hasTauri
      ? null
      : DEFAULT_FIXTURE_URL;

  if (fixtureUrl) {
    const raw = await fetchFixture(fixtureUrl);
    const p = normalizePayload(raw); // 不是物件 → 拋錯
    return {
      channel: WALLPAPER_CHANNEL,
      status: 'ok',
      data: p.data,
      config: p.config,
      shape: p.shape,
      warnings: p.warnings,
      meta: { fetched: p.data?.fetched ?? null, updated: p.data?.updated ?? null, fixture: fixtureUrl },
      source: 'fixture',
      fixtureUrl,
    };
  }

  const none = { channel: WALLPAPER_CHANNEL, data: null, config: {}, shape: null, warnings: [], meta: null, source: 'bridge', fixtureUrl: null };
  try {
    const snap = await getSnapshot(WALLPAPER_CHANNEL);
    if (snap?.status === 'ok' && snap.data != null) {
      const p = normalizePayload(snap.data);
      return { ...none, status: 'ok', data: p.data, config: p.config, shape: p.shape, warnings: p.warnings, meta: snap.meta ?? null };
    }
    return { ...none, status: snap?.status ?? 'empty', meta: snap?.meta ?? null };
  } catch (e) {
    const msg = String(e?.message || e);
    return { ...none, status: 'error', error: msg, warnings: [`宿主通道讀取失敗：${msg}`] };
  }
}

/** canvas → PNG 位元組（Uint8Array）。`toBlob` 是回呼式 API，不是計時器。 */
export function exportPng(canvas) {
  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (!blob) {
        reject(new Error('canvas.toBlob 回傳 null（尺寸過大或 canvas 已污染）'));
        return;
      }
      blob.arrayBuffer().then((buf) => resolve(new Uint8Array(buf)), reject);
    }, 'image/png');
  });
}

/**
 * 預設回報：只在有 Tauri 時呼叫宿主指令（見檔頭「宿主接法」）。指令不存在或失敗只 console.warn，
 * 不拋錯。回傳 `{ sent: 'done'|'failed'|null, error: string|null }`。
 * @param {{ ok: true, png: Uint8Array, meta: object } | { ok: false, error: string, meta: object }} outcome
 */
export async function reportToHost(outcome) {
  if (!hasTauri) {
    return { sent: null, error: null };
  }
  const invoke = window.__TAURI__?.core?.invoke;
  const sent = outcome.ok ? 'done' : 'failed';
  try {
    if (outcome.ok) {
      await invoke('wallpaper_render_done', outcome.png, {
        headers: { 'x-wallpaper-meta': encodeURIComponent(JSON.stringify(outcome.meta)) },
      });
    } else {
      await invoke('wallpaper_render_failed', { error: outcome.error, meta: outcome.meta });
    }
    return { sent, error: null };
  } catch (e) {
    const msg = String(e?.message || e);
    console.warn(`wallpaper：回報宿主（${sent}）失敗：${msg}`);
    return { sent: null, error: msg };
  }
}

function reportMeta(state) {
  return {
    w: state.w,
    h: state.h,
    tz: state.tz,
    nowMs: state.nowMs,
    rid: state.rid,
    warnings: [...state.warnings],
    dataStatus: state.dataStatus,
    dataSource: state.dataSource,
  };
}

/**
 * 跑一次完整繪製流程。回傳的 Promise 在 done／error 後 resolve 成狀態物件（不 reject：
 * 失敗一律走 `phase:'error'` ＋ `state.error`，呼叫端只看狀態）。
 * @param {{ canvas: string|HTMLCanvasElement, draw: (env: object) => (void|Promise<void>),
 *           fonts?: typeof FONT_REQUIREMENTS, search?: string,
 *           report?: typeof reportToHost }} options `report` 供測試替換宿主回報。
 */
export async function runWallpaper({ canvas, draw, fonts = FONT_REQUIREMENTS, search, report = reportToHost }) {
  const state = initState();
  // 回報宿主一定要帶回 render id（失敗路徑也要），所以在任何可能拋錯的步驟之前取出。
  state.rid = readRenderId(search ?? window.location.search);
  let outcome;
  try {
    const params = parseQuery(search ?? window.location.search);
    state.params = params;
    state.warnings.push(...params.warnings);
    state.w = params.w;
    state.h = params.h;
    state.tz = params.tz;
    state.nowMs = params.nowMs;

    const cv = typeof canvas === 'string' ? document.querySelector(canvas) : canvas;
    if (!cv) {
      throw new Error(`找不到 canvas：${canvas}`);
    }
    cv.width = params.w;
    cv.height = params.h;

    // 1) 字型：全部載入成功才往下；失敗的清單帶在 error 裡。
    state.fonts = await loadFonts(fonts);
    state.fontSources = fontSources();
    const bad = state.fonts.filter((f) => !f.ok);
    if (bad.length > 0) {
      throw new Error(`字型載入失敗：${bad.map((f) => `${f.id}（${f.error}）`).join('；')}`);
    }
    const external = state.fontSources.filter((s) => !s.local);
    if (external.length > 0) {
      throw new Error(`字型來源非本機：${external.map((s) => s.url).join('、')}`);
    }

    // 2) 資料與設定
    const snapshot = await loadSnapshot(params);
    state.dataStatus = snapshot.status;
    state.dataSource = snapshot.source;
    state.dataShape = snapshot.shape;
    state.dataKeys = snapshot.data ? Object.keys(snapshot.data) : [];
    state.configKeys = Object.keys(snapshot.config ?? {});
    state.fixtureUrl = snapshot.fixtureUrl ?? null;
    state.warnings.push(...snapshot.warnings);

    // 3) 畫
    const ctx = cv.getContext('2d', { alpha: false });
    const warn = (message) => {
      state.warnings.push(String(message));
    };
    const env = {
      canvas: cv,
      ctx,
      W: params.w,
      H: params.h,
      S: sizeScale(params.w, params.h),
      nowMs: params.nowMs,
      tz: params.tz,
      params,
      snapshot,
      data: snapshot.data ?? null,
      config: snapshot.config ?? {},
      withDefaults(defaults) {
        const merged = mergeThemeConfig(defaults, snapshot.config);
        for (const w of merged.warnings) {
          if (!state.warnings.includes(w)) warn(w);
        }
        return merged.value;
      },
      warn,
      boxes: state.boxes,
    };
    await draw(env);

    // 4) 宿主內：匯出 PNG（失敗＝整張失敗）
    outcome = { ok: true, png: hasTauri ? await exportPng(cv) : null, meta: null };
  } catch (e) {
    outcome = { ok: false, error: String(e?.message || e), meta: null };
  }

  // 5) 回報宿主，再設畫完訊號（看到最終 phase 時回報已嘗試過）
  outcome.meta = reportMeta(state);
  state.hostReport = await report(outcome);
  finish(state, outcome.ok ? 'done' : 'error', outcome.ok ? null : outcome.error);
  return state;
}
