// host/tests/host-cdp-eval-committed.test.mjs
//
// host-cdp-eval.mjs 只能在「已提交」的頁面文件裡執行運算式。
//
// 失效鏈（2026-10-06 實測，verify-5.2／5.4 回歸）：/json/list 的 url 是 WebView2 的「可見 URL」，
// 導覽一開始（Page.frameStartedLoading）就變成目標網址，但真正提交（executionContextsCleared＋
// frameNavigated）要再晚 20–70 ms，宿主主執行緒忙著建小工具時更久。這段期間連上去執行的運算式
// 其實跑在初始的 about:blank 文件裡：同步讀表單會得到「元素不存在」，輪詢型運算式則在提交那一刻
// 拿到 CDP「Execution context was destroyed」。
//
// 執行：node host/tests/host-cdp-eval-committed.test.mjs（結束碼 0＝全部通過）

import assert from 'node:assert/strict';
import { evalOnCommittedPage } from '../tools/host-cdp-eval.mjs';

let failed = 0;
async function test(name, fn) {
  try {
    await fn();
    console.log(`PASS  ${name}`);
  } catch (e) {
    failed++;
    console.log(`FAIL  ${name}\n      ${e.message}`);
  }
}

const SETTINGS = 'http://tauri.localhost/settings.html';
const listed = [{ type: 'page', url: SETTINGS, webSocketDebuggerUrl: 'ws://x/settings' }];

/**
 * 假的 CDP 連線：每次 connect 依序取用一份「文件狀態」。
 * docs[i] = { href, exprResult?, exprError?, hrefError? }
 */
function fakeBrowser(docs) {
  const calls = { connects: 0, exprRuns: [] };
  return {
    calls,
    listTargets: async () => listed,
    connect: async () => {
      const doc = docs[Math.min(calls.connects, docs.length - 1)];
      calls.connects++;
      return {
        evaluate: async (expr) => {
          if (expr === 'location.href') {
            if (doc.hrefError) throw new Error(doc.hrefError);
            return doc.href;
          }
          calls.exprRuns.push(doc.href);
          if (doc.exprError) throw new Error(doc.exprError);
          return doc.exprResult;
        },
        close: () => {},
      };
    },
  };
}

const noSleep = async () => {};

await test('列表已是 settings.html、文件仍是 about:blank → 不在 about:blank 執行，等提交後才執行', async () => {
  const b = fakeBrowser([{ href: 'about:blank' }, { href: 'about:blank' }, { href: SETTINGS, exprResult: 42 }]);
  const v = await evalOnCommittedPage({ ...b, urlPart: 'settings.html', expression: 'X', sleep: noSleep });
  assert.equal(v, 42);
  assert.deepEqual(b.calls.exprRuns, [SETTINGS], '運算式只能在已提交的文件執行一次');
});

await test('讀 location.href 時撞到提交（Execution context was destroyed）→ 重試', async () => {
  const b = fakeBrowser([
    { hrefError: 'CDP 錯誤 -32000：Execution context was destroyed.' },
    { href: SETTINGS, exprResult: 'ok' },
  ]);
  const v = await evalOnCommittedPage({ ...b, urlPart: 'settings.html', expression: 'X', sleep: noSleep });
  assert.equal(v, 'ok');
});

await test('已提交後運算式本身遇到 context destroyed（真的重載）→ 原樣往上丟、不重跑運算式', async () => {
  const b = fakeBrowser([{ href: SETTINGS, exprError: 'CDP 錯誤 -32000：Execution context was destroyed.' }]);
  await assert.rejects(
    evalOnCommittedPage({ ...b, urlPart: 'settings.html', expression: 'X', sleep: noSleep }),
    /Execution context was destroyed/,
  );
  assert.equal(b.calls.exprRuns.length, 1, '有副作用的運算式不得重跑');
});

await test('一直沒提交 → 逾時錯誤（含「尚未提交」）', async () => {
  const b = fakeBrowser([{ href: 'about:blank' }]);
  let t = 0;
  await assert.rejects(
    evalOnCommittedPage({
      ...b,
      urlPart: 'settings.html',
      expression: 'X',
      timeoutMs: 1000,
      sleep: async (ms) => { t += ms; },
      now: () => t,
    }),
    /尚未提交/,
  );
  assert.equal(b.calls.exprRuns.length, 0);
});

await test('列表找不到分頁 → 維持「找不到 url 含」訊息（驗收腳本靠它重試）', async () => {
  await assert.rejects(
    evalOnCommittedPage({
      listTargets: async () => [{ type: 'page', url: 'http://tauri.localhost/widget.html?w=clock' }],
      connect: async () => { throw new Error('不該連線'); },
      urlPart: 'settings.html',
      expression: 'X',
      sleep: noSleep,
    }),
    /找不到 url 含「settings\.html」的分頁/,
  );
});

if (failed > 0) {
  console.log(`\n${failed} 項失敗`);
  process.exit(1);
}
console.log('\n全部通過');
