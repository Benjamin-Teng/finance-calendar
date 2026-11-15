// host/tests/compare/serve.mjs
//
// 本機 HTTP 伺服器：用 Node 內建 `node:http` 起一個極簡靜態伺服器（零安裝，與 cdp.mjs 同一個語言、
// 同一個事件迴圈），給 compare.mjs 與同目錄的 verify-*／measure-* 腳本共用。
//
// 兩個路徑前綴各對應一個實體目錄：
//   /new/      → host/ui/ 本體（**直接讀 repo 內的檔案，不複製**，邊改邊測時永遠拿到最新版本）
//   /fixtures/ → 暫存目錄下的 fixture 副本（host/ui/fixtures/ 整份複製＋覆寫 tw-events.json
//                成呼叫端指定的 fixture 內容）；頁面用 `?fixtures=/fixtures/`
//                （host/ui/bridge.js 支援的 query 參數覆寫）指過來。呼叫端可以在啟動後改寫
//                暫存目錄裡的 settings.json 等檔案，不會動到 repo。

import { createServer } from 'node:http';
import { readFile, writeFile, mkdir, cp, rm, mkdtemp } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..', '..', '..');
const HOST_UI_DIR = path.join(REPO_ROOT, 'host', 'ui');

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

/**
 * 準備暫存伺服器根目錄＋啟動靜態伺服器。
 * @param {string} fixturePath 絕對路徑，tw-events fixture JSON 檔。
 * @returns {Promise<{url: string, port: number, tmpRoot: string, tmpRootDisplay: string, close: () => Promise<void>}>}
 *   `tmpRootDisplay`：印到主控台／證據記錄用的暫存目錄，系統暫存目錄那一段改寫成 `%TEMP%`
 *   （task 7.7：repo 公開，證據不得留下使用者設定檔路徑）。
 */
export async function startServer(fixturePath) {
  const tmpRoot = await mkdtemp(path.join(tmpdir(), 'fc-compare-serve-'));
  const fixturesDir = path.join(tmpRoot, 'fixtures');
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

  // fixtures 根目錄：整份複製 host/ui/fixtures/（settings.json／customN.json 等），
  // 再用呼叫端指定的 fixture 覆寫 tw-events.json。
  const hostUiFixturesDir = path.join(HOST_UI_DIR, 'fixtures');
  if (existsSync(hostUiFixturesDir)) {
    await cp(hostUiFixturesDir, fixturesDir, { recursive: true });
  } else {
    await mkdir(fixturesDir, { recursive: true });
  }
  await writeFile(path.join(fixturesDir, 'tw-events.json'), JSON.stringify(fixtureObj, null, 1));

  const ROUTES = [
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
