// host/tools/verify-dw-5.2-favicon.mjs
//
// dynamic-wallpaper task 5.2：headless Edge（獨立 --user-data-dir，host/tests/compare/cdp.mjs）開
// docs/index.html，確認每條 favicon 連結（rel 含 icon 與 apple-touch-icon）都指向可載入的圖檔、
// 尺寸符合宣告；favicon 內容與統一圖示一致由 host/tests/ico.test.mjs 逐位元組核對。
//
//   node host/tools/verify-dw-5.2-favicon.mjs [記錄檔路徑]
//
// 預設記錄檔 host/tools/evidence/dw-5.2-favicon.log；結束碼 0＝PASS、1＝FAIL。

import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { CDPTarget, closeTarget, launchEdge, newTarget } from '../tests/compare/cdp.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PAGE = path.resolve(HERE, '..', '..', 'docs', 'index.html');
const LOG = process.argv[2] ?? path.join(HERE, 'evidence', 'dw-5.2-favicon.log');

const PROBE_JS = `(async () => {
  const links = [...document.querySelectorAll('link[rel~="icon"], link[rel="apple-touch-icon"]')];
  const out = [];
  for (const l of links) {
    const img = new Image();
    let ok = true, err = '';
    try {
      img.src = l.href;
      await img.decode();
    } catch (e) {
      ok = false;
      err = String(e && e.message || e);
    }
    out.push({
      rel: l.rel,
      type: l.type || '',
      sizes: l.getAttribute('sizes') || '',
      href: l.getAttribute('href'),
      ok,
      err,
      w: img.naturalWidth,
      h: img.naturalHeight,
    });
  }
  return { title: document.title, links: out };
})()`;

const lines = [`dw-5.2 著陸頁 favicon 連結檢查  ${new Date().toISOString()}`, `頁面：docs/index.html（file://，headless Edge、獨立 user-data-dir）`];
let fail = 0;
const edge = await launchEdge();
let targetInfo;
let cdp;
try {
  targetInfo = await newTarget(edge.port, pathToFileURL(PAGE).href);
  cdp = new CDPTarget(targetInfo.webSocketDebuggerUrl, { commandTimeoutMs: 20000 });
  await cdp.connect();
  // 等文件載入完成
  for (let i = 0; i < 100; i++) {
    const r = await cdp.send('Runtime.evaluate', { expression: 'document.readyState', returnByValue: true });
    if (r.result.value === 'complete') break;
    await new Promise((res) => setTimeout(res, 100));
  }
  const r = await cdp.send('Runtime.evaluate', { expression: PROBE_JS, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(`頁面例外：${r.exceptionDetails.text}`);
  const { title, links } = r.result.value;
  lines.push(`標題：${title}`);
  if (links.length === 0) {
    lines.push('FAIL  頁面沒有任何 favicon 連結');
    fail++;
  }
  const want = { 'favicon.svg': [64, 64], 'favicon.png': [64, 64] };
  for (const l of links) {
    const expect = want[l.href];
    const sizeOk = expect ? l.w === expect[0] && l.h === expect[1] : l.w > 0;
    const good = l.ok && sizeOk;
    if (!good) fail++;
    lines.push(
      `${good ? 'PASS' : 'FAIL'}  rel=${l.rel} type=${l.type || '-'} sizes=${l.sizes || '-'} href=${l.href}  載入=${l.ok ? '成功' : `失敗（${l.err}）`}  實際 ${l.w}x${l.h}${expect ? `（預期 ${expect[0]}x${expect[1]}）` : ''}`,
    );
  }
  for (const href of Object.keys(want)) {
    if (!links.some((l) => l.href === href)) {
      lines.push(`FAIL  沒有指向 ${href} 的連結`);
      fail++;
    }
  }
} catch (e) {
  lines.push(`FAIL  ${e.stack ?? e}`);
  fail++;
} finally {
  cdp?.close();
  if (targetInfo) await closeTarget(edge.port, targetInfo.id);
  await edge.close();
}
lines.push(fail === 0 ? 'VERDICT PASS' : `VERDICT FAIL（${fail} 項）`);
const text = `${lines.join('\n')}\n`;
await writeFile(LOG, text, 'utf8');
process.stdout.write(text);
process.exit(fail === 0 ? 0 : 1);
