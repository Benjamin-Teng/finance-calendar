// host/tests/compare/cdp-timeout.test.mjs
//
// 重現並鎖住 Codex adversarial review（.superpowers/sdd/tasks/reviews/task-4.2-codex.md
// [medium]）指出的問題：CDPTarget.send() 沒有逾時機制，WebSocket close/error 也不會拒絕
// pending 指令——對照測試在頁面卡死（連線還活著、永遠不回覆）或連線中途被砍斷時會永遠
// 卡住，`waitForPageCondition()` 外層的 timeoutMs 也救不了（它正在 await 那筆永遠不會
// resolve 的指令，沒有機會重新檢查時間），最終讓驗收程序進不到 finally 清理。
//
// 不依賴真的 headless Edge：起一個最小的 raw WebSocket handshake server（純 node:http＋
// node:crypto，不裝任何套件），只做 101 handshake、不解析/回覆任何 frame——這樣「連線
// 建立成功但永遠沒有回覆」（情境 A）與「連線中途被砍斷」（情境 B）都能在一兩秒內、
// 確定性地重現，不用等真的瀏覽器卡死。
//
// 執行：node host/tests/compare/cdp-timeout.test.mjs
// 通過條件：兩個情境都必須讓 `send()` 在合理時間內 reject（不是 resolve、也不是永遠掛著）。

import crypto from 'node:crypto';
import http from 'node:http';
import { CDPTarget } from './cdp.mjs';

const WS_MAGIC = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

/** 最小 WebSocket handshake server：只回 101，不解析任何 frame。 */
function startRawWsServer(onConnection) {
  const server = http.createServer((req, res) => {
    res.writeHead(404).end();
  });
  server.on('upgrade', (req, socket) => {
    const key = req.headers['sec-websocket-key'];
    if (!key) {
      socket.destroy();
      return;
    }
    const accept = crypto
      .createHash('sha1')
      .update(key + WS_MAGIC)
      .digest('base64');
    socket.write(
      'HTTP/1.1 101 Switching Protocols\r\n' +
        'Upgrade: websocket\r\n' +
        'Connection: Upgrade\r\n' +
        `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    onConnection(socket);
  });
  return new Promise((resolve, reject) => {
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => resolve(server));
  });
}

/** 測試本身的保險絲：萬一被測程式碼真的卡死，讓這支測試自己也能在有限時間內失敗退出，
 * 而不是把「沒有逾時保護」這個 bug 原樣繼承到測試本身。 */
async function withTimeout(promise, ms, label) {
  let timer;
  const guard = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`測試本身逾時：${label}（${ms}ms 內沒有結果）`)), ms);
  });
  try {
    return await Promise.race([promise, guard]);
  } finally {
    clearTimeout(timer);
  }
}

async function scenarioNoReply() {
  // 情境 A：handshake 成功、之後永遠不回覆任何 frame——模擬 renderer 卡死但 TCP 連線
  // 還活著（Codex finding 描述的「頁面卡死」情境）。
  let serverSocket;
  const server = await startRawWsServer((socket) => {
    serverSocket = socket;
    // 什麼都不做：不回 frame、不關閉。
  });
  const { port } = server.address();
  const target = new CDPTarget(`ws://127.0.0.1:${port}/`, { commandTimeoutMs: 200 });
  try {
    await target.connect();
    const start = Date.now();
    let outcome;
    try {
      await target.send('Runtime.evaluate', { expression: '1' });
      outcome = 'RESOLVED_UNEXPECTEDLY';
    } catch (e) {
      outcome = e.message;
    }
    const elapsed = Date.now() - start;
    if (outcome === 'RESOLVED_UNEXPECTEDLY') {
      throw new Error('send() 不應該在完全沒有回覆的情況下 resolve');
    }
    if (elapsed > 2000) {
      throw new Error(`send() 沒有在合理時間內結束（實際 ${elapsed}ms，訊息：${outcome}）`);
    }
    console.log(`[cdp-timeout] 情境 A（無回覆）PASS：${elapsed}ms 內以逾時結束（${outcome}）`);
  } finally {
    target.close();
    // 伺服器端刻意不實作 WebSocket 關閉握手（本來就只做 101 handshake、不解任何
    // frame），client 端的 `ws.close()` 送出的 close frame 永遠得不到回應，TCP 連線
    // 會一直半開著——`server.close()` 要等所有連線真的結束才 resolve，不主動砍掉這個
    // socket 的話，收尾本身會卡住，把「情境 A 沒有回覆」這個假設意外套用到收尾邏輯上。
    serverSocket?.destroy();
    await new Promise((resolve) => server.close(resolve));
  }
}

async function scenarioDisconnect() {
  // 情境 B：handshake 成功、指令送出後伺服器立刻砍斷連線——模擬對照測試常見的「頁面／
  // 分頁在指令送出後被關掉」。commandTimeoutMs 故意設很長（15s），確定是靠 close/error
  // 立刻拒絕 pending，不是靠逾時湊巧撞上。
  let serverSocket;
  const server = await startRawWsServer((socket) => {
    serverSocket = socket;
  });
  const { port } = server.address();
  const target = new CDPTarget(`ws://127.0.0.1:${port}/`, { commandTimeoutMs: 15000 });
  try {
    await target.connect();
    const sendPromise = target.send('Runtime.evaluate', { expression: '1' });
    // 給 server 端一點時間收到 client 送出的 frame（不需要真的解析內容），再砍線。
    await new Promise((r) => setTimeout(r, 100));
    serverSocket.destroy();

    const start = Date.now();
    let outcome;
    try {
      await sendPromise;
      outcome = 'RESOLVED_UNEXPECTEDLY';
    } catch (e) {
      outcome = e.message;
    }
    const elapsed = Date.now() - start;
    if (outcome === 'RESOLVED_UNEXPECTEDLY') {
      throw new Error('斷線後 pending 指令不應該 resolve');
    }
    if (elapsed > 2000) {
      throw new Error(`斷線後 send() 沒有很快被拒絕（實際等了 ${elapsed}ms，訊息：${outcome}）`);
    }
    console.log(`[cdp-timeout] 情境 B（斷線）PASS：${elapsed}ms 內 pending 指令被拒絕（${outcome}）`);
  } finally {
    target.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

async function main() {
  await withTimeout(scenarioNoReply(), 5000, '情境 A（無回覆）');
  await withTimeout(scenarioDisconnect(), 5000, '情境 B（斷線）');
  console.log('[cdp-timeout] 全部情境 PASS');
}

main().catch((e) => {
  console.error('[cdp-timeout] FAIL：' + e.message);
  process.exit(1);
});
