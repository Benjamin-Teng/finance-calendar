// host/tools/wallpaper-shots.mjs
//
// 桌布繪圖頁截圖與契約驗證指令稿（task 3.1；design.md D2／D7／D8）。以 headless Edge 載入
// host/ui/wallpapers/<頁面>，等頁面的「畫完」訊號（`window.__wallpaper.phase`），
// **從頁面 canvas 匯出 PNG**（不截整個視窗；headless viewport 比視窗小約 30px），
// 對五種尺寸各輸出一張，並在稿內斷言：
//   - PNG 的 IHDR 寬高精確等於要求尺寸
//   - 畫完狀態為 done、warnings 為空（含未知 query 參數）、所有字型 loaded 且來源與頁面同源
//   - 頁面「真的收到」fixture 資料：dataStatus ok、dataSource fixture，且頁面回報的資料鍵
//     與 fixture 檔實際的鍵完全一致（不只是「有畫出東西」）
//   - 頁面回報的文字邊界框全在畫面內且兩兩不相交（checkLayout）
//   - 頁面整段過程沒有任何對外（非本機）請求
// `--dsf <n>`：以 CDP `Emulation.setDeviceMetricsOverride` 把 devicePixelRatio 設為 n（模擬 Windows 縮放比例；
// viewport 取 w/n × h/n，即「同一台螢幕在 n 倍縮放下」）。`--dsf-compare <n>`：每個尺寸另以 dsf n 再畫一次，
// 斷言頁面真的看到該 devicePixelRatio，且兩張逐像素相同（spec「縮放比例不影響構圖」）。
// `--expect-layout-fail`：反例模式（給故意畫出重疊／出界文字的測試頁用）——版面檢查必須同時抓到
// 「超出畫面」與「相交」、且沒有其他種類的問題才算通過，證明斷言真的會失敗。
// 頁面若設了 `window.__wallpaperSummary`（字串，例：星盤各市場狀態），會記進輸出。
// `--offline-check`：每個尺寸另以「攔截所有非本機請求」的離線狀態再畫一次，比對兩次的
// 像素 SHA-256 與 PNG 位元組是否完全相同，並用一個對外 fetch 證明攔截確實生效（負向對照）。
// `--contract-checks`：驗證頁面契約（見 wallpaper.mjs 檔頭）：預設 fixture、`{data,config}` 信封、
// 未知 query 參數警告、fixture 讀不到／不是 JSON 的明確失敗、宿主回報
// （以假的 `window.__TAURI__` 驗 `wallpaper_render_done`／`wallpaper_render_failed` 的呼叫與參數形狀，
// 以及回報指令不存在時的容錯）。
//
// 用法（repo 根目錄）：
//   node host/tools/wallpaper-shots.mjs                       # 五種尺寸，輸出到 host/tools/evidence/
//   node host/tools/wallpaper-shots.mjs --offline-check       # 加做離線／連線逐像素比對
//   node host/tools/wallpaper-shots.mjs --contract-checks     # 加做頁面契約驗證
//        （頁面需要預設 fixture 沒有的鍵時加 --contract-fixture <裸 tw_events 網址> --contract-envelope <信封網址>）
//   node host/tools/wallpaper-shots.mjs --page _demo.html --prefix dw-3.1-demo \
//        --out host/tools/evidence --sizes 1920x1080,1080x1920 --t 2026-10-02T21:00 \
//        --tz Asia/Taipei --fixture /test-fixtures/tw_events_sample.json
//   node host/tools/wallpaper-shots.mjs --page astrolabe.html --prefix dw-3.3-x --sizes 3840x2160 --dsf 1 --dsf-compare 1.5
// 每次都會寫 `<prefix>-shots.log`（完整輸出）。
// `--fixture none` ＝不帶 `fixture=`（獨立模式讀 host/ui/fixtures/tw-events.json）。
//
// 零安裝：只用 Node 22 內建模組；Edge 啟動／CDP 沿用 host/tests/compare/cdp.mjs
// （每次擷取都是全新的 --user-data-dir、headless=new）。本機 HTTP 伺服器提供頁面，
// 因為 file:// 下 ES module 會被擋：`/` → host/ui/；`/test-fixtures/` → repo 的 tests/fixtures/
// 與 host/tests/wallpapers/fixtures/（依序找，先找到的為準）。

import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { CDPTarget, closeTarget, launchEdge, newTarget, waitForPageCondition } from '../tests/compare/cdp.mjs';
import { FONT_REQUIREMENTS, checkLayout, cssFont, normalizePayload } from '../ui/wallpapers/lib/core.mjs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..');
const UI_DIR = path.join(REPO_ROOT, 'host', 'ui');
const TEST_FIXTURE_DIRS = [path.join(REPO_ROOT, 'tests', 'fixtures'), path.join(REPO_ROOT, 'host', 'tests', 'wallpapers', 'fixtures')];
const DEFAULT_FIXTURE_FILE = path.join(UI_DIR, 'fixtures', 'tw-events.json');

export const FIVE_SIZES = [
  [3840, 2160],
  [2560, 1600],
  [1920, 1080],
  [2560, 1080],
  [1080, 1920],
];

/**
 * 各頁面「至少必須」載入的字型 id（獨立於頁面自己的清單，刻意寫死在這裡）：頁面漏載某個字型時，canvas 會
 * 悄悄退回系統字型、畫面照樣出得來，必須由這裡讓它明確失敗。未列出的頁面只檢查「頁面要求的每個字型都載入成功」。
 */
const ASTROLABE_REQUIRED_FONTS = ['cinzel-600', 'cinzel-700', 'notosans-500', 'notosans-600'];
/** 撕日曆（task 3.4）：spec「離線繪製」的三種內建字型——Noto Serif TC 900、霞鶩文楷 TC 400、700。 */
const TEAROFF_REQUIRED_FONTS = ['notoserif-900', 'wenkai-400', 'wenkai-700'];
/** 脊線與等高線（task 3.5）：交易日與標高用 IBM Plex Mono 600（等高線樣稿字型）、「資料停在 M/D」用 Noto Sans TC 500。 */
const INTRADAY_REQUIRED_FONTS = ['plexmono-600', 'notosans-500'];
/** 天際線（task 3.6）：日期範圍與收盤／20MA 標示用 IBM Plex Mono 600、「資料停在 M/D」用 Noto Sans TC 500。 */
const SKYLINE_REQUIRED_FONTS = ['plexmono-600', 'notosans-500'];
export const PAGE_REQUIRED_FONTS = {
  '_demo.html': FONT_REQUIREMENTS.map((r) => r.id),
  'astrolabe.html': ASTROLABE_REQUIRED_FONTS,
  '../test-fixtures/astrolabe-layout-negative.html': ASTROLABE_REQUIRED_FONTS,
  'tearoff.html': TEAROFF_REQUIRED_FONTS,
  '../test-fixtures/tearoff-layout-negative.html': TEAROFF_REQUIRED_FONTS,
  '../test-fixtures/tearoff-ganzhi-edge.html': TEAROFF_REQUIRED_FONTS,
  'ridgeline.html': INTRADAY_REQUIRED_FONTS,
  '../test-fixtures/ridgeline-layout-negative.html': INTRADAY_REQUIRED_FONTS,
  'contour.html': INTRADAY_REQUIRED_FONTS,
  '../test-fixtures/contour-layout-negative.html': INTRADAY_REQUIRED_FONTS,
  'skyline.html': SKYLINE_REQUIRED_FONTS,
  '../test-fixtures/skyline-layout-negative.html': SKYLINE_REQUIRED_FONTS,
  '../test-fixtures/skyline-stale-oldpos-negative.html': SKYLINE_REQUIRED_FONTS,
};

/**
 * `--page` 的正規化寫法（相對 host/ui/wallpapers/）：反斜線改斜線、去掉 `./`、摺疊 `a/../`。
 * 3.3 審查 low：原本以原字串查 PAGE_REQUIRED_FONTS，`./tearoff.html` 這類變體會靜默跳過必要字型檢查。
 * 頁面網址也改用正規化後的寫法，所以查表與實際載入的一定是同一頁。
 */
export function normalizePagePath(page) {
  const p = path.posix.normalize(String(page).replaceAll('\\', '/'));
  return p.startsWith('/') ? p.slice(1) : p;
}

/** 某頁「至少必須」載入的字型 id（未列出的頁面回傳 null＝只檢查頁面自己要求的字型）。 */
export function requiredFontsFor(page) {
  return PAGE_REQUIRED_FONTS[normalizePagePath(page)] ?? null;
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
  '.ttf': 'font/ttf',
  '.txt': 'text/plain; charset=utf-8',
};

/** 把頁面用的 fixture 網址（`/test-fixtures/x.json`、`/fixtures/x.json`…）對應到本機檔案。 */
function fixtureUrlToFile(url) {
  if (url === 'none') {
    return DEFAULT_FIXTURE_FILE;
  }
  if (url.startsWith('/test-fixtures/')) {
    const rel = url.slice('/test-fixtures/'.length);
    return TEST_FIXTURE_DIRS.map((d) => path.join(d, rel)).find((f) => existsSync(f)) ?? null;
  }
  if (url.startsWith('/')) {
    const f = path.join(UI_DIR, url.slice(1));
    return existsSync(f) ? f : null;
  }
  return null;
}

/** 極簡靜態伺服器（只綁 127.0.0.1、隨機埠、防路徑穿越）。 */
async function startServer() {
  const routes = [
    { prefix: '/test-fixtures/', dirs: TEST_FIXTURE_DIRS },
    { prefix: '/', dirs: [UI_DIR] },
  ];
  const server = createServer(async (req, res) => {
    try {
      const urlPath = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      const route = routes.find((r) => urlPath.startsWith(r.prefix));
      let file = null;
      for (const dir of route?.dirs ?? []) {
        const f = path.normalize(path.join(dir, urlPath.slice(route.prefix.length)));
        if (f.startsWith(path.normalize(dir)) && existsSync(f)) {
          file = f;
          break;
        }
      }
      if (!file) {
        res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
        res.end(`404 ${urlPath}`);
        return;
      }
      const body = await readFile(file);
      res.writeHead(200, { 'content-type': MIME[path.extname(file).toLowerCase()] || 'application/octet-stream' });
      res.end(body);
    } catch (e) {
      res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`500 ${e.message}`);
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  const { port } = server.address();
  return {
    origin: `http://127.0.0.1:${port}`,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

function pngSize(bytes) {
  const sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  if (bytes.length < 24 || !sig.every((b, i) => bytes[i] === b)) {
    throw new Error('輸出不是 PNG（簽章不符）');
  }
  return { w: bytes.readUInt32BE(16), h: bytes.readUInt32BE(20) };
}

const sha256 = (buf) => createHash('sha256').update(buf).digest('hex');

/**
 * 開一個全新的 headless Edge 分頁、載入頁面、等畫完，把控制權交給 `fn`，結束後清理。
 * @param {{ origin: string, pagePath: string, query: string, offline?: boolean,
 *           init?: string|null, blockUrlPart?: string|null }} o
 *   `offline`：攔截所有非本機請求；`init`：每次文件建立前先跑的腳本（假宿主用）；
 *   `blockUrlPart`：網址含此字串的請求一律讓它失敗（模擬字型載入失敗）。
 * @param {(c: { ev: (expr: string) => Promise<any>, state: object, requests: string[], blocked: string[] }) => Promise<any>} fn
 */
async function withPage(o, fn) {
  const { origin, pagePath, query, offline = false, init = null, blockUrlPart = null, dsf = null, w = null, h = null } = o;
  const edge = await launchEdge();
  let target = null;
  let tab = null;
  try {
    tab = await newTarget(edge.port, 'about:blank');
    target = new CDPTarget(tab.webSocketDebuggerUrl, { commandTimeoutMs: 60000 });
    await target.connect();

    const requests = [];
    const blocked = [];
    target.on('Network.requestWillBeSent', (p) => requests.push(p.request.url));
    await target.send('Network.enable');
    await target.send('Page.enable');
    await target.send('Runtime.enable');
    if (init) {
      await target.send('Page.addScriptToEvaluateOnNewDocument', { source: init });
    }
    if (dsf != null) {
      // 模擬「同一台 w×h 螢幕在 dsf 倍縮放下」：CSS viewport 縮小、devicePixelRatio＝dsf
      await target.send('Emulation.setDeviceMetricsOverride', {
        width: Math.round((w ?? 1920) / dsf),
        height: Math.round((h ?? 1080) / dsf),
        deviceScaleFactor: dsf,
        mobile: false,
      });
    }

    if (offline || blockUrlPart) {
      target.on('Fetch.requestPaused', (p) => {
        const url = p.request.url;
        const isLocal = url.startsWith(`${origin}/`) || url.startsWith('data:');
        let cmd;
        if (blockUrlPart && url.includes(blockUrlPart)) {
          cmd = target.send('Fetch.failRequest', { requestId: p.requestId, errorReason: 'Failed' });
        } else if (offline && !isLocal) {
          blocked.push(url);
          cmd = target.send('Fetch.failRequest', { requestId: p.requestId, errorReason: 'InternetDisconnected' });
        } else {
          cmd = target.send('Fetch.continueRequest', { requestId: p.requestId });
        }
        cmd.catch(() => {});
      });
      await target.send('Fetch.enable', { patterns: [{ urlPattern: '*' }] });
    }

    await target.send('Page.navigate', { url: `${origin}/wallpapers/${pagePath}?${query}` });
    await waitForPageCondition(target, "window.__wallpaper && window.__wallpaper.phase !== 'loading'", {
      timeoutMs: 90000,
      intervalMs: 200,
    });

    const ev = async (expression) => {
      const r = await target.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (r.exceptionDetails) {
        throw new Error(`頁面端例外：${r.exceptionDetails.exception?.description || JSON.stringify(r.exceptionDetails)}`);
      }
      return r.result?.value;
    };
    const state = await ev('JSON.parse(JSON.stringify(window.__wallpaper))');
    return await fn({ ev, state, requests, blocked });
  } finally {
    target?.close();
    if (tab) {
      await closeTarget(edge.port, tab.id);
    }
    await edge.close();
  }
}

const sameSet = (a, b) => a.length === b.length && [...a].sort().join('\u0000') === [...b].sort().join('\u0000');

/** fixture 檔實際內容的資料鍵與設定鍵（與頁面用同一個 normalizePayload），讀不到回傳 null。 */
async function expectedFixtureShape(fixtureArg) {
  const file = fixtureUrlToFile(fixtureArg);
  if (!file) {
    return null;
  }
  const p = normalizePayload(JSON.parse(await readFile(file, 'utf8')));
  return { dataKeys: Object.keys(p.data), configKeys: Object.keys(p.config), shape: p.shape };
}

/**
 * 擷取一張：新 Edge（新 user-data-dir）→ 載入頁面 → 等畫完 → 驗證 → 匯出 canvas。
 * @param {object} o withPage 的參數，另加 `w`、`h`；`expect`＝`{ dataKeys, configKeys, shape }`
 *   （頁面回報的資料／設定鍵必須與之完全相同）；`requireData`（預設 true）＝必須真的收到資料。
 * @returns {Promise<{ png: Buffer, pngSha: string, pixelSha: string, state: object,
 *                     requests: string[], blockedByPage: number, control: string|null, problems: string[] }>}
 */
async function capture({ w, h, expect = null, requireData = true, ...o }) {
  const problems = [];
  return withPage({ ...o, w, h }, async ({ ev, state, requests, blocked }) => {
    const { origin, offline } = o;
    if (state.phase !== 'done') {
      problems.push(`畫完狀態 ${state.phase}：${state.error}`);
    }
    if (state.warnings?.length) {
      problems.push(`頁面警告：${state.warnings.join('；')}`);
    }
    if (state.w !== w || state.h !== h) {
      problems.push(`頁面回報尺寸 ${state.w}×${state.h} ≠ 要求 ${w}×${h}`);
    }

    // 資料：頁面必須真的收到 fixture 內容（不能只是「畫出了東西」）
    if (requireData) {
      if (state.dataStatus !== 'ok') {
        problems.push(`頁面沒有收到資料：dataStatus=${state.dataStatus}、dataSource=${state.dataSource}`);
      }
      if (state.dataKeys.length === 0) {
        problems.push('頁面收到的資料沒有任何鍵（空資料）');
      }
      if (state.dataSource === 'fixture' && !state.fixtureUrl) {
        problems.push('dataSource 為 fixture 但沒有 fixtureUrl');
      }
      if (expect) {
        if (!sameSet(state.dataKeys, expect.dataKeys)) {
          problems.push(`頁面資料鍵與 fixture 檔不一致：頁面=${JSON.stringify(state.dataKeys)}，檔案=${JSON.stringify(expect.dataKeys)}`);
        }
        if (!sameSet(state.configKeys, expect.configKeys)) {
          problems.push(`頁面設定鍵與 fixture 檔不一致：頁面=${JSON.stringify(state.configKeys)}，檔案=${JSON.stringify(expect.configKeys)}`);
        }
        if (state.dataShape !== expect.shape) {
          problems.push(`payload 形狀 ${state.dataShape} ≠ 預期 ${expect.shape}`);
        }
      }
    }

    // 字型：document.fonts 逐項 loaded、來源為頁面同源
    const faces = await ev(
      "JSON.parse(JSON.stringify([...document.fonts].map(f => ({ family: f.family.replace(/\"/g, ''), weight: f.weight, status: f.status }))))",
    );
    // 頁面可只載入自己用到的子集（runWallpaper({ fonts })）：逐項檢查頁面要求的字型，且必須都在清單內
    const requested = state.fonts.map((f) => f.id);
    if (requested.length === 0) {
      problems.push('頁面沒有要求任何字型（state.fonts 為空）');
    }
    const unknown = requested.filter((id) => !FONT_REQUIREMENTS.some((r) => r.id === id));
    if (unknown.length) {
      problems.push(`頁面要求的字型不在 FONT_REQUIREMENTS：${unknown.join('、')}`);
    }
    const mustHave = requiredFontsFor(o.pagePath) ?? [];
    const missing = mustHave.filter((id) => !requested.includes(id));
    if (missing.length) {
      problems.push(`頁面 ${o.pagePath} 沒有載入必要字型：${missing.join('、')}（PAGE_REQUIRED_FONTS）`);
    }
    for (const req of FONT_REQUIREMENTS.filter((r) => requested.includes(r.id))) {
      const ok = await ev(`document.fonts.check(${JSON.stringify(cssFont(req))})`);
      const rep = state.fonts.find((f) => f.id === req.id);
      if (!ok || !rep?.ok) {
        problems.push(`字型 ${req.id} 未就緒（check=${ok}，回報=${JSON.stringify(rep)}）`);
      }
    }
    state.faces = faces;
    if (state.fontSources.length === 0) {
      problems.push('沒有任何字型資源紀錄（Resource Timing 為空，無法證明來源）');
    }
    for (const s of state.fontSources) {
      if (!s.url.startsWith(`${origin}/`)) {
        problems.push(`字型來源非本機：${s.url}`);
      }
    }

    // 對外請求：整個過程不得有任何非本機請求
    const external = requests.filter((u) => !u.startsWith(`${origin}/`) && !u.startsWith('data:') && u !== 'about:blank');
    if (external.length) {
      problems.push(`出現對外請求：${external.join('、')}`);
    }
    const blockedByPage = blocked.length;
    if (blocked.length) {
      problems.push(`離線攔截到非本機請求（頁面不該有）：${blocked.join('、')}`);
    }

    // 版面：文字邊界框
    const lay = checkLayout(state.boxes, w, h);
    const layoutProblems = [];
    if (!lay.ok) {
      layoutProblems.push(`版面檢查失敗：超出畫面 ${JSON.stringify(lay.outside)}；相交 ${JSON.stringify(lay.overlaps)}`);
    }
    problems.push(...layoutProblems);
    if (state.boxes.length === 0) {
      problems.push('頁面沒有回報任何文字邊界框（env.boxes 為空），版面檢查無意義');
    }

    // 離線負向對照：確認攔截真的生效（對外 fetch 必須失敗）
    let control = null;
    if (offline) {
      control = await ev("fetch('https://fonts.googleapis.com/css2?family=Cinzel', { cache: 'no-store' }).then(() => 'reached', () => 'blocked')");
      if (control !== 'blocked') {
        problems.push(`離線攔截無效：對外 fetch 結果為 ${control}`);
      }
    }

    // 匯出：canvas → PNG，另對原始像素算 SHA-256（逐像素比對用）
    const dataUrl = await ev("document.getElementById('c').toDataURL('image/png')");
    if (!dataUrl?.startsWith('data:image/png;base64,')) {
      throw new Error('canvas.toDataURL 沒回傳 PNG');
    }
    const png = Buffer.from(dataUrl.slice('data:image/png;base64,'.length), 'base64');
    const size = pngSize(png);
    if (size.w !== w || size.h !== h) {
      problems.push(`PNG 尺寸 ${size.w}×${size.h} ≠ 要求 ${w}×${h}`);
    }
    const pixelSha = await ev(`(async () => {
      const cv = document.getElementById('c');
      const d = cv.getContext('2d').getImageData(0, 0, cv.width, cv.height).data;
      const h = await crypto.subtle.digest('SHA-256', d);
      return [...new Uint8Array(h)].map(b => b.toString(16).padStart(2, '0')).join('');
    })()`);

    const dpr = await ev('window.devicePixelRatio');
    const summary = await ev("typeof window.__wallpaperSummary === 'string' ? window.__wallpaperSummary : null");
    return { png, pngSha: sha256(png), pixelSha, state, requests, blockedByPage, control, problems, layout: lay, layoutProblems, dpr, summary };
  });
}

// ── 頁面契約驗證（--contract-checks）────────────────────────────────────────────────────

/** 假的 Tauri 宿主：bridge.js 的 `get_snapshot` 回 `{data,config}` 信封；其他呼叫記錄在 `window.__hostCalls`。 */
function fakeHostScript({ failReport }) {
  return `(() => {
    window.__hostCalls = [];
    window.__TAURI__ = { core: { invoke: async (cmd, args, options) => {
      if (cmd === 'get_snapshot') {
        const r = await fetch('/test-fixtures/tw_events_sample.json');
        return { channel: args.channel, status: 'ok', data: { data: await r.json(), config: { demo: { title: '主機桌布' } } }, meta: { loadedAt: 0 } };
      }
      const isBytes = args instanceof Uint8Array;
      window.__hostCalls.push({
        cmd,
        isBytes,
        bytes: isBytes ? args.length : null,
        head: isBytes ? Array.from(args.slice(0, 24)) : null,
        args: isBytes ? null : args,
        headers: options && options.headers ? options.headers : null,
      });
      if (${failReport ? 'true' : 'false'}) {
        throw new Error('Command ' + cmd + ' not found');
      }
      return null;
    } } };
  })()`;
}

async function contractChecks({ origin, pagePath, tBase, say, failures, fixtureA = null, envelopeB = null }) {
  const results = [];
  const check = (name, ok, detail = '') => {
    results.push({ name, ok });
    say(`[契約] ${ok ? 'OK  ' : 'FAIL'} ${name}${detail ? `：${detail}` : ''}`, 'contract');
    if (!ok) failures.push(`[契約] ${name}${detail ? `：${detail}` : ''}`);
  };
  const baseQuery = (extra = {}) => new URLSearchParams({ w: '1920', h: '1080', ...tBase, ...extra });
  const dims = { w: 1920, h: 1080 };

  // A. 獨立模式沒帶 fixture：讀預設檔 host/ui/fixtures/tw-events.json，且確實收到資料。
  //    頁面需要預設檔沒有的資料鍵時（脊線、等高線需要 twii_intraday），以 --contract-fixture 指定頁面適用的裸 tw_events
  //    fixture，A 改驗「指定 fixture 真的送進頁面」（task 3.5 修正輪 1）。
  if (fixtureA) {
    const exp = await expectedFixtureShape(fixtureA);
    const r = await capture({ origin, pagePath, query: baseQuery({ fixture: fixtureA }).toString(), ...dims, expect: exp });
    check(`頁面適用 fixture（--contract-fixture ${fixtureA}）真的送進頁面`, r.problems.length === 0 && r.state.fixtureUrl?.endsWith(fixtureA) && r.state.dataShape === exp.shape,
      r.problems.join('；') || `鍵 ${r.state.dataKeys.length} 個、形狀 ${r.state.dataShape}`);
  } else {
    const exp = await expectedFixtureShape('none');
    const r = await capture({ origin, pagePath, query: baseQuery().toString(), ...dims, expect: exp });
    check('預設 fixture（不帶 fixture=）真的送進頁面', r.problems.length === 0 && r.state.fixtureUrl?.endsWith('/fixtures/tw-events.json'),
      r.problems.join('；') || `鍵 ${r.state.dataKeys.length} 個、形狀 ${r.state.dataShape}`);
  }

  // B. 信封 {data, config}：兩半都進得了頁面（--contract-envelope 可換成頁面適用的信封，config 須只有 demo 鍵）
  {
    const fixture = envelopeB ?? '/test-fixtures/envelope.json';
    const exp = await expectedFixtureShape(fixture);
    const r = await capture({ origin, pagePath, query: baseQuery({ fixture }).toString(), ...dims, expect: exp });
    check('{data, config} 信封 fixture：data 與 config 都進頁面', r.problems.length === 0 && r.state.dataShape === 'envelope' && sameSet(r.state.configKeys, ['demo']),
      r.problems.join('；') || `data 鍵 ${JSON.stringify(r.state.dataKeys)}、config 鍵 ${JSON.stringify(r.state.configKeys)}`);
  }

  // C. 未知 query 參數：警告可見（頁面仍 done）
  await withPage({ origin, pagePath, query: baseQuery({ fixture: '/test-fixtures/tw_events_sample.json', now: '2026-10-02T13:00:00Z', fixtures: 'x' }).toString() }, async ({ state }) => {
    const w = state.warnings.filter((x) => x.startsWith('未知參數'));
    check('未知 query 參數（now=、fixtures=）產生警告且仍畫完', state.phase === 'done' && w.length === 2 && w.some((x) => x.includes('now=')) && w.some((x) => x.includes('fixtures=')), JSON.stringify(w));
  });

  // D/E. fixture 讀不到、不是 JSON → 明確失敗，不是 empty
  for (const [name, fixture, needle] of [
    ['fixture 404', '/test-fixtures/does-not-exist.json', 'HTTP 404'],
    ['fixture 不是 JSON', '/wallpapers/lib/core.mjs', '不是合法 JSON'],
  ]) {
    await withPage({ origin, pagePath, query: baseQuery({ fixture }).toString() }, async ({ state }) => {
      check(`${name} → phase error 並說明原因`, state.phase === 'error' && String(state.error).includes(needle) && state.dataStatus !== 'ok', `phase=${state.phase} error=${state.error}`);
    });
  }

  // F. 宿主回報：成功 → wallpaper_render_done（PNG 本體＋meta header）
  await withPage({ origin, pagePath, query: baseQuery().toString(), init: fakeHostScript({ failReport: false }) }, async ({ ev, state }) => {
    const calls = await ev('JSON.parse(JSON.stringify(window.__hostCalls))');
    const call = calls[0];
    const sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    let meta = null;
    try {
      meta = JSON.parse(decodeURIComponent(call?.headers?.['x-wallpaper-meta'] ?? ''));
    } catch {
      meta = null;
    }
    const head = call?.head ?? [];
    const pngW = (head[16] << 24) | (head[17] << 16) | (head[18] << 8) | head[19];
    const pngH = (head[20] << 24) | (head[21] << 16) | (head[22] << 8) | head[23];
    const ok =
      state.phase === 'done' && calls.length === 1 && call.cmd === 'wallpaper_render_done' && call.isBytes &&
      sig.every((b, i) => head[i] === b) && pngW === 1920 && pngH === 1080 && call.bytes > 1000 &&
      meta && meta.w === 1920 && meta.h === 1080 && Array.isArray(meta.warnings) && meta.warnings.length === 0 &&
      meta.dataStatus === 'ok' && state.hostReport.sent === 'done' && state.dataSource === 'bridge' &&
      state.dataShape === 'envelope' && sameSet(state.configKeys, ['demo']);
    check('宿主內成功：呼叫 wallpaper_render_done（PNG 位元組＋meta）', !!ok,
      `呼叫=${calls.map((c) => c.cmd).join(',')} PNG ${pngW}×${pngH} ${call?.bytes}B meta=${JSON.stringify(meta)}`);
  });

  // G. 宿主回報：失敗（字型載入失敗）→ wallpaper_render_failed，且不送 done。
  // 擋掉該頁第一個必要字型的檔案（_demo／星盤＝Cinzel，撕日曆＝Noto Serif TC；task 3.4 起不再寫死 Cinzel）
  const blockReq = FONT_REQUIREMENTS.find((r) => r.id === (requiredFontsFor(pagePath) ?? ['cinzel-600'])[0]);
  const blockFamily = blockReq.id.split('-')[0];
  await withPage({ origin, pagePath, query: baseQuery().toString(), init: fakeHostScript({ failReport: false }), blockUrlPart: blockReq.file }, async ({ ev, state }) => {
    const calls = await ev('JSON.parse(JSON.stringify(window.__hostCalls))');
    const call = calls[0];
    const ok =
      state.phase === 'error' && calls.length === 1 && call.cmd === 'wallpaper_render_failed' && !call.isBytes &&
      typeof call.args?.error === 'string' && call.args.error.includes(blockFamily) && call.args.meta?.w === 1920 &&
      Array.isArray(call.args.meta?.warnings) && state.hostReport.sent === 'failed';
    check(`宿主內失敗（字型 ${blockReq.file} 載入失敗）：呼叫 wallpaper_render_failed、不送 done`, !!ok, `呼叫=${calls.map((c) => c.cmd).join(',')} error=${call?.args?.error?.slice(0, 60)}`);
  });

  // H. 宿主回報指令不存在（4.5 尚未實作）：容錯，phase 照常 done，原因記在 hostReport.error
  await withPage({ origin, pagePath, query: baseQuery().toString(), init: fakeHostScript({ failReport: true }) }, async ({ state }) => {
    check('回報指令不存在：容錯（phase done、hostReport.error 有原因）', state.phase === 'done' && state.hostReport.sent === null && String(state.hostReport.error).includes('not found'), JSON.stringify(state.hostReport));
  });

  // I. done 帶 warnings 仍算成功：警告放在 meta.warnings（w 不合法 → 預設尺寸）
  await withPage({ origin, pagePath, query: baseQuery({ w: 'abc' }).toString(), init: fakeHostScript({ failReport: false }) }, async ({ ev, state }) => {
    const calls = await ev('JSON.parse(JSON.stringify(window.__hostCalls))');
    const meta = JSON.parse(decodeURIComponent(calls[0]?.headers?.['x-wallpaper-meta'] ?? '%7B%7D'));
    check('done 帶 warnings 也算成功：照送 PNG、警告在 meta.warnings、w 為實際尺寸',
      state.phase === 'done' && calls[0]?.cmd === 'wallpaper_render_done' && meta.warnings?.length === 1 && meta.w === 3840 && meta.h === 1080,
      JSON.stringify(meta.warnings));
  });

  return results.every((r) => r.ok);
}

export function parseArgs(argv) {
  const a = {
    page: '_demo.html',
    prefix: 'dw-3.1-demo',
    out: path.join('host', 'tools', 'evidence'),
    sizes: FIVE_SIZES,
    t: '2026-10-02T21:00',
    tz: 'Asia/Taipei',
    fixture: '/test-fixtures/tw_events_sample.json',
    offlineCheck: false,
    contractChecks: false,
    dsf: null,
    dsfCompare: null,
    expectLayoutFail: false,
    contractFixture: null,
    contractEnvelope: null,
  };
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    const next = () => {
      if (i + 1 >= argv.length) {
        throw new Error(`${k} 需要參數值`);
      }
      return argv[++i];
    };
    if (k === '--page') a.page = next();
    else if (k === '--prefix') a.prefix = next();
    else if (k === '--out') a.out = next();
    else if (k === '--t') a.t = next();
    else if (k === '--tz') a.tz = next();
    else if (k === '--fixture') a.fixture = next();
    else if (k === '--offline-check') a.offlineCheck = true;
    else if (k === '--contract-checks') a.contractChecks = true;
    else if (k === '--contract-fixture') a.contractFixture = next();
    else if (k === '--contract-envelope') a.contractEnvelope = next();
    else if (k === '--expect-layout-fail') a.expectLayoutFail = true;
    else if (k === '--dsf' || k === '--dsf-compare') {
      const raw = next();
      const v = Number(raw);
      if (!(Number.isFinite(v) && v > 0 && v <= 4)) throw new Error(`${k} 需要 0–4 之間的數字，實際為 ${raw}`);
      if (k === '--dsf') a.dsf = v;
      else a.dsfCompare = v;
    }
    else if (k === '--sizes') {
      a.sizes = next()
        .split(',')
        .map((s) => {
          const m = /^(\d+)x(\d+)$/.exec(s.trim());
          if (!m) throw new Error(`--sizes 格式錯誤：${s}（應為 WxH，逗號分隔）`);
          return [Number(m[1]), Number(m[2])];
        });
    } else {
      throw new Error(`未知參數：${k}`);
    }
  }
  return a;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  args.page = normalizePagePath(args.page);
  const outDir = path.resolve(REPO_ROOT, args.out);
  await mkdir(outDir, { recursive: true });
  // 先核對 fixture 再起伺服器：之前順序相反，fixture 對應不到時拋錯但伺服器沒關，Node 會一直掛著
  // （3.1 遺留 N2；Git Bash 會把 `/test-fixtures/...` 轉成 `C:/Program Files/Git/test-fixtures/...`，
  // 在 Git Bash 執行要加 MSYS_NO_PATHCONV=1）
  const expect = await expectedFixtureShape(args.fixture);
  if (!expect) {
    throw new Error(`--fixture ${args.fixture} 對應不到本機檔案（無法核對頁面是否真的收到資料；在 Git Bash 執行時請加 MSYS_NO_PATHCONV=1）`);
  }
  const server = await startServer();
  const logs = { main: [], contract: [] };
  const say = (s, bucket = 'main') => {
    console.log(s);
    logs[bucket].push(s);
  };
  const failures = [];
  const header = `# wallpaper-shots ${new Date().toISOString()}  頁面=${args.page}  t=${args.t}  tz=${args.tz}  fixture=${args.fixture}  離線比對=${args.offlineCheck}  契約檢查=${args.contractChecks}  dsf=${args.dsf ?? '（不覆寫）'}  dsf 比對=${args.dsfCompare ?? '否'}  反例模式=${args.expectLayoutFail}`;
  say(header);
  say(`fixture 檔實際內容：形狀 ${expect.shape}、資料鍵 ${expect.dataKeys.length} 個、設定鍵 ${expect.configKeys.length} 個`);
  const required = requiredFontsFor(args.page);
  say(`必要字型（PAGE_REQUIRED_FONTS）：${required ? required.join('、') : '未列出，只檢查頁面自己要求的字型'}`);
  try {
    for (const [w, h] of args.sizes) {
      const q = new URLSearchParams({ w: String(w), h: String(h), t: args.t, tz: args.tz });
      if (args.fixture !== 'none') {
        q.set('fixture', args.fixture);
      }
      const base = { origin: server.origin, pagePath: args.page, query: q.toString(), w, h, expect, dsf: args.dsf };
      const tag = `${w}x${h}`;

      const on = await capture({ ...base, offline: false });
      const file = path.join(outDir, `${args.prefix}-${tag}.png`);
      await writeFile(file, on.png);
      const verdictOn = args.expectLayoutFail
        ? on.layoutProblems.length ? '版面檢查如預期失敗' : 'FAIL（版面檢查沒抓到）'
        : on.problems.length ? 'FAIL' : 'OK';
      say(
        `[${tag}] 連線 ${verdictOn}  png=${on.png.length}B pngSha=${on.pngSha.slice(0, 16)} ` +
          `pixelSha=${on.pixelSha.slice(0, 16)} boxes=${on.state.boxes.length} dpr=${on.dpr} 請求=${on.requests.length}（全本機）` +
          `資料鍵=${on.state.dataKeys.length}（${on.state.dataShape}） → ${path.relative(REPO_ROOT, file)}`,
      );
      if (on.summary) say(`[${tag}] 頁面摘要：${on.summary}`);
      if (args.dsf != null && on.dpr !== args.dsf) failures.push(`[${tag}] --dsf ${args.dsf} 未生效：頁面 devicePixelRatio=${on.dpr}`);
      if (args.expectLayoutFail) {
        // 反例：必須同時抓到出界與相交，且除了版面以外沒有別的問題
        const other = on.problems.filter((p) => !on.layoutProblems.includes(p));
        say(`[${tag}] 反例：超出畫面=${JSON.stringify(on.layout.outside)}  相交=${JSON.stringify(on.layout.overlaps)}`);
        if (on.layout.outside.length === 0) failures.push(`[${tag} 反例] 版面檢查沒有抓到超出畫面的文字`);
        if (on.layout.overlaps.length === 0) failures.push(`[${tag} 反例] 版面檢查沒有抓到相交的文字`);
        other.forEach((p) => failures.push(`[${tag} 反例] 版面以外的問題：${p}`));
      } else {
        on.problems.forEach((p) => failures.push(`[${tag} 連線] ${p}`));
      }

      if (args.dsfCompare != null) {
        const alt = await capture({ ...base, dsf: args.dsfCompare, offline: false });
        const altFile = path.join(outDir, `${args.prefix}-${tag}-dsf${args.dsfCompare}.png`);
        await writeFile(altFile, alt.png);
        const samePixels = alt.pixelSha === on.pixelSha;
        const samePng = alt.pngSha === on.pngSha;
        say(
          `[${tag}] dsf ${args.dsfCompare} ${alt.problems.length ? 'FAIL' : 'OK'}  頁面 devicePixelRatio=${alt.dpr}（基準 ${on.dpr}）  ` +
            `pixelSha=${alt.pixelSha.slice(0, 16)} pngSha=${alt.pngSha.slice(0, 16)} → ${path.relative(REPO_ROOT, altFile)}`,
        );
        say(`[${tag}] dsf ${on.dpr} vs ${alt.dpr}：像素${samePixels ? '逐像素相同' : '不同'}；PNG 位元組${samePng ? '相同' : '不同'}`);
        alt.problems.forEach((p) => failures.push(`[${tag} dsf ${args.dsfCompare}] ${p}`));
        if (alt.dpr !== args.dsfCompare) failures.push(`[${tag}] --dsf-compare ${args.dsfCompare} 未生效：頁面 devicePixelRatio=${alt.dpr}`);
        if (alt.dpr === on.dpr) failures.push(`[${tag}] dsf 比對無效：兩次 devicePixelRatio 相同（${alt.dpr}）`);
        if (!samePixels) failures.push(`[${tag}] dsf ${on.dpr} 與 ${alt.dpr} 像素不同`);
      }

      if (args.offlineCheck) {
        const off = await capture({ ...base, offline: true });
        const samePixels = off.pixelSha === on.pixelSha;
        const samePng = off.pngSha === on.pngSha;
        say(
          `[${tag}] 離線 ${off.problems.length ? 'FAIL' : 'OK'}  對外 fetch 負向對照=${off.control}  頁面被攔截的請求數=${off.blockedByPage}  ` +
            `pixelSha=${off.pixelSha.slice(0, 16)} pngSha=${off.pngSha.slice(0, 16)}`,
        );
        say(`[${tag}] 離線 vs 連線：像素${samePixels ? '逐像素相同' : '不同'}；PNG 位元組${samePng ? '相同' : '不同'}`);
        if (tag === `${args.sizes[0][0]}x${args.sizes[0][1]}`) {
          say(`[${tag}] 字型來源（離線）：${off.state.fontSources.map((s) => `${path.basename(s.url)}${s.local ? '(本機)' : '(外部!)'}`).join(' ')}`);
          say(`[${tag}] document.fonts（離線）：${off.state.faces.map((f) => `${f.family}/${f.weight}=${f.status}`).join(' ')}`);
        }
        off.problems.forEach((p) => failures.push(`[${tag} 離線] ${p}`));
        if (!samePixels) failures.push(`[${tag}] 離線與連線像素不同`);
        if (!samePng) failures.push(`[${tag}] 離線與連線 PNG 位元組不同`);
      }
    }
    if (args.contractChecks) {
      logs.contract.push(header);
      await contractChecks({ origin: server.origin, pagePath: args.page, tBase: { t: args.t, tz: args.tz }, say, failures, fixtureA: args.contractFixture, envelopeB: args.contractEnvelope });
    }
  } finally {
    await server.close();
  }
  const verdict = failures.length ? `結果：FAIL（${failures.length} 項）` : '結果：全部通過';
  say(verdict);
  if (args.contractChecks) logs.contract.push(verdict);
  failures.forEach((f) => say(`  - ${f}`));
  await writeFile(path.join(outDir, `${args.prefix}-shots.log`), `${logs.main.join('\n')}\n`, 'utf8');
  if (args.offlineCheck) {
    await writeFile(path.join(outDir, `${args.prefix}-offline-check.log`), `${logs.main.join('\n')}\n`, 'utf8');
  }
  if (args.contractChecks) {
    await writeFile(path.join(outDir, `${args.prefix}-contract-check.log`), `${logs.contract.join('\n')}\n`, 'utf8');
  }
  process.exitCode = failures.length ? 1 : 0;
}

// 直接執行才跑 main（單元測試 import 本檔只取 normalizePagePath／requiredFontsFor）
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(e);
    process.exitCode = 2;
  });
}
