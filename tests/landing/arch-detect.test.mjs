// tests/landing/arch-detect.test.mjs
//
// 著陸頁 docs/index.html 的架構偵測（installer-auto-update design D8）：以 node:vm 抽出頁面中
// <script id="arch-detect"> 的腳本，在假的 document／navigator 上執行，斷言主按鈕 href 與 #dl-arch 文字。
// 零安裝：node --test "tests/landing/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const HTML_URL = new URL('../../docs/index.html', import.meta.url);
const BASE = 'https://github.com/Benjamin-Teng/finance-calendar/releases/latest/download/';
const X64 = `${BASE}finance-calendar-setup.exe`;
const ARM64 = `${BASE}finance-calendar-setup-arm64.exe`;

const html = await readFile(HTML_URL, 'utf8');

function extractScript(source) {
  const m = source.match(/<script\b[^>]*\bid=["']arch-detect["'][^>]*>([\s\S]*?)<\/script>/i);
  assert.ok(m, '找不到 <script id="arch-detect">');
  return m[1];
}

const SCRIPT = extractScript(html);

/** 主按鈕初始 href 與 #dl-arch 初始文字取自 HTML 本身，不在測試裡另寫一份。 */
function initialState(source) {
  const btn = source.match(/<a\b[^>]*\bid=["']dl-main["'][^>]*>/i);
  assert.ok(btn, '找不到 #dl-main');
  const href = btn[0].match(/\bhref=["']([^"']+)["']/i)?.[1];
  const label = source.match(/<span\b[^>]*\bid=["']dl-arch["'][^>]*>([^<]*)<\/span>/i)?.[1];
  assert.ok(label !== undefined, '找不到 #dl-arch');
  return { href, label };
}

async function run(navigator) {
  const init = initialState(html);
  const btn = { href: init.href };
  const label = { textContent: init.label };
  const document = {
    getElementById(id) {
      if (id === 'dl-main') return btn;
      if (id === 'dl-arch') return label;
      return null;
    },
  };
  const context = vm.createContext({ document, navigator });
  vm.runInContext(SCRIPT, context, { filename: 'arch-detect.js' });
  // 讓 Promise 鏈（含 reject 的 catch）跑完
  for (let i = 0; i < 5; i++) await new Promise((r) => setImmediate(r));
  return { href: btn.href, label: label.textContent };
}

test('HTML 預設是 x64', () => {
  assert.deepEqual(initialState(html), { href: X64, label: 'x64' });
});

test('回報 arm → 換成 ARM64', async () => {
  const r = await run({ userAgentData: { getHighEntropyValues: async () => ({ architecture: 'arm', bitness: '64' }) } });
  assert.deepEqual(r, { href: ARM64, label: 'ARM64' });
});

test('回報 x86 → 維持 x64', async () => {
  const r = await run({ userAgentData: { getHighEntropyValues: async () => ({ architecture: 'x86', bitness: '64' }) } });
  assert.deepEqual(r, { href: X64, label: 'x64' });
});

test('沒有 userAgentData → 維持 x64', async () => {
  const r = await run({});
  assert.deepEqual(r, { href: X64, label: 'x64' });
});

test('getHighEntropyValues reject → 維持 x64', async () => {
  const r = await run({ userAgentData: { getHighEntropyValues: () => Promise.reject(new Error('NotAllowedError')) } });
  assert.deepEqual(r, { href: X64, label: 'x64' });
});

test('getHighEntropyValues 同步拋例外 → 維持 x64', async () => {
  const r = await run({
    userAgentData: {
      getHighEntropyValues() {
        throw new Error('boom');
      },
    },
  });
  assert.deepEqual(r, { href: X64, label: 'x64' });
});

test('userAgentData 沒有 getHighEntropyValues → 維持 x64', async () => {
  const r = await run({ userAgentData: { brands: [], mobile: false, platform: 'Windows' } });
  assert.deepEqual(r, { href: X64, label: 'x64' });
});

test('主按鈕下方固定保留 x64／ARM64 兩個文字連結', () => {
  const block = html.match(/<div\b[^>]*class=["'][^"']*\barch-links\b[^"']*["'][^>]*>([\s\S]*?)<\/div>/i);
  assert.ok(block, '找不到 .arch-links');
  assert.ok(block[1].includes(`href="${X64}"`), '缺 x64 文字連結');
  assert.ok(block[1].includes(`href="${ARM64}"`), '缺 ARM64 文字連結');
});
