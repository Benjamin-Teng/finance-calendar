// tests/landing/inline-scripts.test.mjs
//
// 著陸頁 docs/index.html 的 inline <script> 守門：
// (a) 每一段都要能編譯——頁尾動畫腳本若有語法錯誤，整段不會執行，html.js-motion 的隱藏狀態就永久生效。
// (b) head 腳本的 4 秒保險：頁尾腳本沒在期限內設 window.__fcMotionReady 時，要拿掉 js-motion。
// 零安裝：node --test "tests/landing/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const html = await readFile(new URL('../../docs/index.html', import.meta.url), 'utf8');

/** 取出所有沒有 src 的 inline script（含 id 等其他屬性）。 */
function inlineScripts(source) {
  const out = [];
  const re = /<script\b([^>]*)>([\s\S]*?)<\/script>/gi;
  let m;
  while ((m = re.exec(source))) {
    if (/\bsrc\s*=/i.test(m[1])) continue;
    if (/\btype\s*=\s*["'](?!text\/javascript|module)/i.test(m[1])) continue; // JSON-LD 等非腳本
    out.push({ attrs: m[1].trim(), src: m[2], index: out.length });
  }
  return out;
}

const scripts = inlineScripts(html);

test('頁面有 inline script（head 一段、頁尾多段）', () => {
  assert.ok(scripts.length >= 3, `只找到 ${scripts.length} 段`);
});

for (const s of scripts) {
  test(`inline script #${s.index}${s.attrs ? ` (${s.attrs})` : ''} 語法可編譯`, () => {
    assert.doesNotThrow(() => new vm.Script(s.src, { filename: `inline-${s.index}.js` }));
  });
}

const head = scripts.find((s) => s.src.includes("classList.add('js-motion')"));

test('找得到加 js-motion 的 head 腳本，且位於 </head> 之前', () => {
  assert.ok(head, '找不到加 js-motion 的 inline script');
  assert.ok(html.indexOf(head.src) < html.indexOf('</head>'), 'head 腳本不在 <head> 內');
});

/** 在假 window／document 上執行 head 腳本；timers 可手動觸發。 */
function runHead({ withObserver, ready }) {
  const classes = new Set();
  const timers = [];
  const window = {
    setTimeout(fn, ms) {
      timers.push({ fn, ms });
      return timers.length;
    },
  };
  if (withObserver) window.IntersectionObserver = function () {};
  if (ready !== undefined) window.__fcMotionReady = ready;
  const document = {
    documentElement: {
      classList: {
        add: (c) => classes.add(c),
        remove: (c) => classes.delete(c),
        contains: (c) => classes.has(c),
      },
    },
  };
  // 腳本用 'IntersectionObserver' in window，故 window 同時當全域物件
  const context = vm.createContext({ window, document });
  Object.assign(context, window);
  context.window = context;
  context.setTimeout = window.setTimeout;
  vm.runInContext(head.src, context, { filename: 'head.js' });
  return { classes, timers, window: context };
}

test('有 IntersectionObserver：加上 js-motion，並設 4 秒計時器', () => {
  const r = runHead({ withObserver: true });
  assert.ok(r.classes.has('js-motion'));
  assert.equal(r.timers.length, 1);
  assert.equal(r.timers[0].ms, 4000);
});

test('4 秒到期時 __fcMotionReady 未設 → 拿掉 js-motion', () => {
  const r = runHead({ withObserver: true });
  r.timers[0].fn();
  assert.equal(r.classes.has('js-motion'), false);
});

test('4 秒到期時 __fcMotionReady 不是 true（例如 "yes"）→ 拿掉 js-motion', () => {
  const r = runHead({ withObserver: true });
  r.window.__fcMotionReady = 'yes';
  r.timers[0].fn();
  assert.equal(r.classes.has('js-motion'), false);
});

test('4 秒到期時 __fcMotionReady === true → 保留 js-motion', () => {
  const r = runHead({ withObserver: true });
  r.window.__fcMotionReady = true;
  r.timers[0].fn();
  assert.equal(r.classes.has('js-motion'), true);
});

test('頁尾腳本已自行拿掉 class → 計時器到期不報錯、class 維持不在', () => {
  const r = runHead({ withObserver: true });
  r.classes.delete('js-motion');
  r.timers[0].fn();
  assert.equal(r.classes.has('js-motion'), false);
});

test('沒有 IntersectionObserver：不加 class、不設計時器', () => {
  const r = runHead({ withObserver: false });
  assert.equal(r.classes.has('js-motion'), false);
  assert.equal(r.timers.length, 0);
});

test('頁尾動畫腳本在 observe 之後才設 __fcMotionReady（不在腳本開頭）', () => {
  const foot = scripts.find((s) => s.src.includes('__fcMotionReady = true'));
  assert.ok(foot, '找不到設 __fcMotionReady 的頁尾腳本');
  const iObserve = foot.src.indexOf('io.observe(');
  const iReady = foot.src.indexOf('__fcMotionReady = true');
  assert.ok(iObserve >= 0 && iReady > iObserve, '__fcMotionReady 必須出現在 io.observe 之後');
});
