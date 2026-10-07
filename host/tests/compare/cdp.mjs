// host/tests/compare/cdp.mjs
//
// 極簡 Chrome DevTools Protocol 客戶端。只用 Node 22 內建的全域 `fetch`／`WebSocket`
// （`node -e "console.log(typeof WebSocket, typeof fetch)"` 實測皆為 function，2026-09-28），
// 不裝任何 npm 套件——見 README.md「零安裝方案的選擇」一節的理由。
//
// 只實作對照測試會用到的最小子集：啟動/關閉 headless Edge、開/關一個 target、對單一 target
// 的 devtools websocket 送 CDP 指令。刻意不走 Target.attachToTarget 的 flatten session 模式
// （browser-level websocket＋sessionId）：每個 target 自己的 `webSocketDebuggerUrl` 本來就是
// 專屬於該 target 的 devtools 端點，直接連上去送 Page／Runtime／Emulation 指令不需要
// sessionId，這是 CDP 最基本的「連到單一分頁」用法，比 flatten session 少一層間接。

import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

// headless Edge 執行檔候選路徑（依 preflight.md／AGENTS.md「不寫死 x64 專屬假設」，兩個安裝
// 位置都檢查；ARM64 機器上 Edge 是原生安裝，安裝路徑慣例相同，不受影響）。
const EDGE_CANDIDATES = [
  'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
  'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
];

export function findEdge() {
  for (const p of EDGE_CANDIDATES) {
    if (existsSync(p)) return p;
  }
  throw new Error(
    `找不到 msedge.exe（已檢查：${EDGE_CANDIDATES.join('、')}）。若安裝路徑不同，` +
      '請設定環境變數 FC_COMPARE_EDGE_PATH 指向 msedge.exe。',
  );
}

function resolveEdgePath() {
  return process.env.FC_COMPARE_EDGE_PATH || findEdge();
}

async function waitFor(fn, { timeoutMs = 10000, intervalMs = 100, label = 'condition' } = {}) {
  const start = Date.now();
  let lastErr;
  while (Date.now() - start < timeoutMs) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      lastErr = e;
    }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`逾時等待 ${label}（${timeoutMs}ms）${lastErr ? '：' + lastErr.message : ''}`);
}

/**
 * 啟動一個獨立的 headless Edge（獨立 --user-data-dir，避免附掛到既有 Edge 實例——
 * global-constraints.md「headless Edge、獨立 --user-data-dir」）。
 *
 * `--remote-debugging-port=0` 讓 Edge 自己選一個可用埠，實際埠號寫在
 * `<user-data-dir>/DevToolsActivePort` 檔案第一行——比自己找可用埠再指定更可靠
 * （自己找埠有「找到後、Edge 啟動前被別的行程搶走」的競態）。
 */
export async function launchEdge() {
  const exe = resolveEdgePath();
  const userDataDir = await mkdtemp(path.join(tmpdir(), 'fc-compare-edge-'));
  const proc = spawn(
    exe,
    [
      '--headless=new',
      '--disable-gpu',
      '--no-first-run',
      '--no-default-browser-check',
      '--hide-scrollbars',
      '--disable-extensions',
      `--user-data-dir=${userDataDir}`,
      '--remote-debugging-port=0',
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  let exited = null;
  proc.on('exit', (code, signal) => {
    exited = { code, signal };
  });

  const portFile = path.join(userDataDir, 'DevToolsActivePort');
  const port = await waitFor(
    async () => {
      if (exited) throw new Error(`msedge 提前結束（code=${exited.code} signal=${exited.signal}）`);
      if (!existsSync(portFile)) return null;
      const text = await readFile(portFile, 'utf8');
      const line = text.split('\n')[0]?.trim();
      return line ? Number(line) : null;
    },
    { timeoutMs: 15000, label: 'DevToolsActivePort 檔案' },
  );

  await waitFor(
    async () => {
      const res = await fetch(`http://127.0.0.1:${port}/json/version`);
      return res.ok;
    },
    { timeoutMs: 10000, label: '/json/version 就緒' },
  );

  return {
    port,
    userDataDir,
    async close() {
      // 實測踩到的坑（2026-09-28）：`proc.kill()` 只是送出終止訊號、立刻回傳，Windows
      // 對「行程仍持有 user-data-dir 檔案控制代碼」的檔案鎖定比 POSIX 嚴格——緊接著
      // `rm()` 常因為程序還沒真正結束、鎖還沒放掉而失敗（原本用 `.catch(() => {})` 靜默吞掉，
      // 結果留下一批空跑的暫存 profile 目錄，`tasklist`／`Get-CimInstance Win32_Process`
      // 核對過：Chromium `--headless=new` 底下的子行程（renderer／gpu-process 等）是用
      // Windows Job Object 掛在主行程下，主行程一死就會跟著死，不會變成孤兒——所以問題
      // 不是行程樹沒殺乾淨，是「殺乾淨」到「檔案鎖真的放掉」之間有一段時間差。改成：
      // 等 `exit` 事件（若行程已經不在了就不等）＋ `rm` 失敗時重試幾次帶退避。
      if (proc.exitCode === null && !proc.killed) {
        const exited = new Promise((resolve) => proc.once('exit', resolve));
        try {
          proc.kill();
        } catch {
          // 行程可能已結束，忽略。
        }
        await Promise.race([exited, new Promise((r) => setTimeout(r, 5000))]);
      }
      let lastErr;
      for (let attempt = 0; attempt < 5; attempt++) {
        try {
          await rm(userDataDir, { recursive: true, force: true });
          return;
        } catch (e) {
          lastErr = e;
          await new Promise((r) => setTimeout(r, 300));
        }
      }
      console.warn(`[cdp] 清除暫存 user-data-dir 失敗（已重試 5 次）：${userDataDir}：${lastErr?.message}`);
    },
  };
}

/** 開一個新分頁（target），回傳含 `id`／`webSocketDebuggerUrl` 的物件。 */
export async function newTarget(port, url = 'about:blank') {
  const res = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
    method: 'PUT',
  });
  if (!res.ok) {
    throw new Error(`/json/new 失敗：HTTP ${res.status}`);
  }
  return res.json();
}

/** 關掉一個分頁；找不到／已關閉時靜默略過（收尾用，不影響測試結果）。 */
export async function closeTarget(port, id) {
  await fetch(`http://127.0.0.1:${port}/json/close/${id}`).catch(() => {});
}

/** 對單一 target 的 devtools websocket 送 CDP 指令、收事件。
 *
 * fix round 1（task-4.2-codex.md [medium]）：原本 `send()` 沒有逾時，WebSocket
 * close/error 也不會拒絕 pending 指令——頁面卡死（連線還活著、永遠不回覆）或連線中途
 * 被砍斷時，pending 的 promise 永遠不會 settle，`waitForPageCondition()` 外層的
 * timeoutMs 救不了（它正 await 那筆指令，沒機會重新檢查時間），整支對照測試會卡住、
 * 進不到 finally 清理 headless Edge／暫存目錄。對策：① 每筆指令自己帶逾時，逾時就從
 * `pending` 移除並 reject；② WebSocket `close`／`error` 時把所有還在等的 pending 全部
 * reject、並記住「已關閉」讓後續 `send()` 直接 reject（不再假裝還能送）；③ 連線建立
 * （`ready`）本身也加逾時，不再無限等 `open`。重現與回歸測試見
 * host/tests/compare/cdp-timeout.test.mjs（起一個不回覆／中途斷線的假 WebSocket
 * server，修好前會卡住或掛著，修好後兩種情境都能在期限內 reject）。
 */
export class CDPTarget {
  constructor(webSocketDebuggerUrl, { commandTimeoutMs = 10000, connectTimeoutMs = 10000 } = {}) {
    this.ws = new WebSocket(webSocketDebuggerUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.eventHandlers = new Map();
    this.commandTimeoutMs = commandTimeoutMs;
    this.closed = false;
    this.closeError = null;

    this.ready = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(`CDP WebSocket 連線逾時（${connectTimeoutMs}ms）：${webSocketDebuggerUrl}`));
      }, connectTimeoutMs);
      this.ws.addEventListener(
        'open',
        () => {
          clearTimeout(timer);
          resolve();
        },
        { once: true },
      );
      this.ws.addEventListener(
        'error',
        () => {
          clearTimeout(timer);
          reject(new Error(`CDP WebSocket 連線失敗：${webSocketDebuggerUrl}`));
        },
        { once: true },
      );
    });
    // `ready` 在 connect() 被呼叫前不會有人 await 它；連線失敗時避免被 Node 當成
    // unhandledRejection 印出來（connect() 呼叫時才是真正該處理這個 rejection 的地方）。
    this.ready.catch(() => {});

    this.ws.addEventListener('message', (ev) => {
      let msg;
      try {
        msg = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (msg.id != null && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        if (msg.error) reject(new Error(`CDP 錯誤 ${msg.error.code}：${msg.error.message}`));
        else resolve(msg.result);
      } else if (msg.method) {
        const handlers = this.eventHandlers.get(msg.method);
        if (handlers) for (const h of [...handlers]) h(msg.params);
      }
    });

    const rejectAllPending = (err) => {
      this.closed = true;
      this.closeError = err;
      for (const { reject } of this.pending.values()) {
        reject(err);
      }
      this.pending.clear();
    };
    this.ws.addEventListener('close', () => {
      rejectAllPending(new Error('CDP WebSocket 已關閉（分頁或瀏覽器可能已結束）'));
    });
    this.ws.addEventListener('error', () => {
      rejectAllPending(new Error(`CDP WebSocket 發生錯誤：${webSocketDebuggerUrl}`));
    });
  }

  async connect() {
    await this.ready;
  }

  send(method, params = {}, { timeoutMs = this.commandTimeoutMs } = {}) {
    if (this.closed) {
      return Promise.reject(this.closeError || new Error('CDP WebSocket 已關閉'));
    }
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`CDP 指令逾時（${timeoutMs}ms）：${method}`));
      }, timeoutMs);
      const settle = (fn) => (value) => {
        clearTimeout(timer);
        fn(value);
      };
      this.pending.set(id, { resolve: settle(resolve), reject: settle(reject) });
      try {
        this.ws.send(JSON.stringify({ id, method, params }));
      } catch (e) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(e);
      }
    });
  }

  on(method, handler) {
    if (!this.eventHandlers.has(method)) this.eventHandlers.set(method, []);
    this.eventHandlers.get(method).push(handler);
  }

  close() {
    try {
      this.ws.close();
    } catch {
      // 忽略。
    }
  }
}

/** 對頁面執行一段 JS，等到回傳真值或逾時；用於等待頁面渲染完成，避免依賴
 * `Page.loadEventFired`（本頁的資料是同源 `<script>` 動態插入，`load` 事件不保證涵蓋它）。 */
export async function waitForPageCondition(target, expression, { timeoutMs = 8000, intervalMs = 100 } = {}) {
  // fix round 1（task-4.2-codex.md [medium]）：`send()` 現在自己會逾時，但這裡的單筆
  // 指令逾時仍要明顯短於外層 `timeoutMs`，`waitFor()` 的迴圈才有機會在外層期限內重新
  // 檢查時間、正常印出「逾時等待」而不是卡在單一筆指令上；`Math.min` 也保護呼叫端把
  // `timeoutMs` 設得比預設單筆逾時還短的情況。
  const perCommandTimeoutMs = Math.min(3000, timeoutMs);
  await waitFor(
    async () => {
      const result = await target.send(
        'Runtime.evaluate',
        { expression, returnByValue: true },
        { timeoutMs: perCommandTimeoutMs },
      );
      if (result.exceptionDetails) return false;
      return !!result.result?.value;
    },
    { timeoutMs, intervalMs, label: `頁面條件：${expression}` },
  );
}

/** 執行一段 JS 並以 `returnByValue` 取回結果；拋出頁面端例外時原樣往上丟，方便除錯。 */
export async function evaluate(target, expression) {
  const result = await target.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    const desc =
      result.exceptionDetails.exception?.description || JSON.stringify(result.exceptionDetails);
    throw new Error(`頁面端例外：${desc}`);
  }
  return result.result?.value;
}
