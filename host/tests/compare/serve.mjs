// host/tests/compare/serve.mjs
//
// 本機 HTTP 伺服器（design.md D11：「以本機 HTTP 伺服器提供 Lively 版與新版頁面」）。
//
// 用 Node 內建 `node:http` 自己起一個極簡靜態伺服器，不外殼呼叫 `python -m http.server`：
// compare.mjs 本體已經用 Node（全域 `fetch`／`WebSocket` 驅動 CDP，見 cdp.mjs 檔頭「零安裝
// 方案的選擇」），伺服器改成同一個語言可以少一個外部行程、少一種「兩個語言的暫存路徑/編碼
// 對得上嗎」的心智負擔，且 `node:http` 一樣是 stdlib、零安裝，符合 global-constraints.md
// 「自己開的瀏覽器一律 headless；裝任何新套件前先回報 NEEDS_CONTEXT」的零安裝要求。
//
// 三個路徑前綴各對應一個實體目錄：
//   /lively/   → 暫存目錄下的 Lively 版頁面（finance-calendar.html 複製一份＋依 fixture
//                產生的 tw_events.js；不動 repo 根目錄，見 brief「不要在 repo 根留下
//                tw_events.js」）
//   /new/      → host/ui/ 本體（**直接讀 repo 內的檔案，不複製**，4.3–4.5 邊改邊測時永遠
//                拿到最新版本，不會因為忘記重新複製而比對到舊檔）
//   /fixtures/ → 暫存目錄下的 fixture 副本（host/ui/fixtures/ 整份複製＋覆寫 tw-events.json
//                成呼叫端指定的 --fixture 內容）；新版頁面用 `?fixtures=/fixtures/`
//                （host/ui/bridge.js 支援的 query 參數覆寫）指過來，確保兩邊吃到同一份資料
//                ——即使呼叫端指定了非預設的 --fixture 路徑，也不會出現「Lively 版用新資料、
//                新版仍讀 repo 內的舊 fixture」這種假陽性差異。

import { createServer } from 'node:http';
import { readFile, writeFile, mkdir, cp, rm, mkdtemp } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const HOST_UI_DIR = path.join(REPO_ROOT, 'host', 'ui');
const LIVELY_HTML_SRC = path.join(REPO_ROOT, 'finance-calendar.html');

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.png': 'image/png',
};

function mimeFor(filePath) {
  return MIME[path.extname(filePath).toLowerCase()] || 'application/octet-stream';
}

/** 把 fixture JSON 包成 Lively 版讀法的 `window.TW_EVENTS = {...};`
 * （finance-calendar.html 836 行 `s.src = CONFIG.dataBase + 'tw_events.js?...'`，
 * 實際內容格式已用 D:\finance-calendar\tw_events.js 現場資料核對過，就是這個形狀，
 * 見 task-4.2-report.md「fixture 格式核對」）。 */
function toTwEventsJs(fixtureObj) {
  return `window.TW_EVENTS = ${JSON.stringify(fixtureObj)};\n`;
}

/**
 * 準備暫存伺服器根目錄＋啟動靜態伺服器。
 * @param {string} fixturePath 絕對路徑，tw-events fixture JSON 檔。
 * @returns {Promise<{url: string, port: number, tmpRoot: string, tmpRootDisplay: string, close: () => Promise<void>}>}
 *   `tmpRootDisplay`：印到主控台／證據記錄用的暫存目錄，系統暫存目錄那一段改寫成 `%TEMP%`
 *   （task 7.7：repo 公開，證據不得留下使用者設定檔路徑）。
 */
export async function startServer(fixturePath) {
  const tmpRoot = await mkdtemp(path.join(tmpdir(), 'fc-compare-serve-'));
  const livelyDir = path.join(tmpRoot, 'lively');
  const fixturesDir = path.join(tmpRoot, 'fixtures');
  await mkdir(livelyDir, { recursive: true });

  if (!existsSync(LIVELY_HTML_SRC)) {
    throw new Error(`找不到 Lively 版原始檔：${LIVELY_HTML_SRC}`);
  }
  if (!existsSync(fixturePath)) {
    throw new Error(`找不到 fixture 檔：${fixturePath}`);
  }

  const fixtureRaw = await readFile(fixturePath, 'utf8');
  let fixtureObj;
  try {
    fixtureObj = JSON.parse(fixtureRaw);
  } catch (e) {
    throw new Error(`fixture 檔不是合法 JSON：${fixturePath}（${e.message}）`);
  }

  await cp(LIVELY_HTML_SRC, path.join(livelyDir, 'finance-calendar.html'));
  await writeFile(path.join(livelyDir, 'tw_events.js'), toTwEventsJs(fixtureObj));

  // 新版 fixtures 根目錄：整份複製 host/ui/fixtures/（settings.json／customN.json 等），
  // 再用呼叫端指定的 fixture 覆寫 tw-events.json，確保兩邊資料同源。
  const hostUiFixturesDir = path.join(HOST_UI_DIR, 'fixtures');
  if (existsSync(hostUiFixturesDir)) {
    await cp(hostUiFixturesDir, fixturesDir, { recursive: true });
  } else {
    await mkdir(fixturesDir, { recursive: true });
  }
  await writeFile(path.join(fixturesDir, 'tw-events.json'), JSON.stringify(fixtureObj, null, 1));

  const ROUTES = [
    { prefix: '/lively/', dir: livelyDir },
    { prefix: '/fixtures/', dir: fixturesDir },
    { prefix: '/new/', dir: HOST_UI_DIR },
  ];

  function resolveFile(urlPath) {
    for (const { prefix, dir } of ROUTES) {
      if (urlPath.startsWith(prefix)) {
        const rel = urlPath.slice(prefix.length);
        const resolved = path.normalize(path.join(dir, rel));
        if (!resolved.startsWith(path.normalize(dir))) return null; // 防路徑穿越
        return resolved;
      }
    }
    return null;
  }

  const server = createServer(async (req, res) => {
    try {
      const urlPath = decodeURIComponent(new URL(req.url, 'http://localhost').pathname);
      const filePath = resolveFile(urlPath);
      if (!filePath || !existsSync(filePath)) {
        res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' });
        res.end(`404 Not Found: ${urlPath}`);
        return;
      }
      const body = await readFile(filePath);
      res.writeHead(200, { 'content-type': mimeFor(filePath) });
      res.end(body);
    } catch (e) {
      res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' });
      res.end(`500: ${e.message}`);
    }
  });

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => resolve());
  });
  const { port } = server.address();

  return {
    url: `http://127.0.0.1:${port}`,
    port,
    tmpRoot,
    tmpRootDisplay: tmpRoot.startsWith(tmpdir()) ? `%TEMP%${tmpRoot.slice(tmpdir().length)}` : tmpRoot,
    /** @param {{keepFiles?: boolean}} [opts] keepFiles=true 時只停伺服器、保留暫存目錄
     * （`--keep-temp` 除錯用；不保留的話伺服器一定會停，否則 Node 事件迴圈不會結束）。 */
    async close(opts = {}) {
      await new Promise((resolve) => server.close(() => resolve()));
      if (!opts.keepFiles) {
        await rm(tmpRoot, { recursive: true, force: true }).catch(() => {});
      }
    },
  };
}
