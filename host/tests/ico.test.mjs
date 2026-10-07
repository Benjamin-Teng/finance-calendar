// host/tests/ico.test.mjs
//
// dynamic-wallpaper task 5.1：host/tools/lib/ico.mjs（PNG 內嵌式 ICO 封裝／解析／驗證）與
// host/tools/lib/icon-sources.mjs 的 withSize 與按尺寸選來源 SVG 的規則，加上「進 git 的 6 個 .ico 真的含齊所有尺寸」。
// 執行：node --test host/tests/ico.test.mjs（不需要 Edge；產生圖示才需要）。

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { deflateSync } from 'node:zlib';
import test from 'node:test';
import { fileURLToPath } from 'node:url';
import { REQUIRED_SIZES, buildIco, extractPng, parseIco, readPngHeader, verifyIco } from '../tools/lib/ico.mjs';
import { OUTPUT_NAMES, RUNTIME_PNG_SIZES, THEMES, buildOutputs, planDerived, withSize } from '../tools/lib/icon-sources.mjs';

// 最小合法 PNG（RGBA 8-bit、全透明）；只為了讓 IHDR 與尺寸對得上。
function crc32(buf) {
  let c;
  let crc = 0xffffffff;
  for (const b of buf) {
    c = (crc ^ b) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'latin1'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function fakePng(w, h, colorType = 6) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = colorType;
  const raw = Buffer.alloc(h * (1 + w * 4));
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}
const fullSet = () => REQUIRED_SIZES.map((size) => ({ size, png: fakePng(size, size) }));

test('buildIco → parseIco：尺寸由小到大、256 的寬高欄位寫 0、offset 正確', () => {
  const images = fullSet().reverse(); // 故意倒序，驗證會排序
  const ico = buildIco(images);
  assert.equal(ico.readUInt16LE(2), 1);
  assert.equal(ico.readUInt16LE(4), REQUIRED_SIZES.length);
  const last = 6 + 16 * (REQUIRED_SIZES.length - 1);
  assert.equal(ico[last], 0, '256 寬欄位應為 0');
  assert.equal(ico[last + 1], 0, '256 高欄位應為 0');
  const entries = parseIco(ico);
  assert.deepEqual(entries.map((e) => e.width), REQUIRED_SIZES);
  assert.ok(entries.every((e) => e.png && e.png.width === e.width && e.png.height === e.height));
  assert.deepEqual(verifyIco(ico), []);
});

test('verifyIco：缺尺寸、尺寸重複、非 PNG、非 RGBA 都要回報', () => {
  const missing = buildIco(fullSet().filter((i) => i.size !== 24));
  assert.deepEqual(verifyIco(missing), ['缺 24x24']);

  const notRgba = buildIco(fullSet().map((i) => (i.size === 48 ? { size: 48, png: fakePng(48, 48, 2) } : i)));
  assert.match(verifyIco(notRgba).join(';'), /（48px）不是 RGBA 8-bit/);

  // 把 entry 的寬度欄位竄改成與 IHDR 不一致
  const tampered = Buffer.from(buildIco(fullSet()));
  tampered[6] = 17; // 第 0 個 entry（16px）的寬度欄位改成 17
  assert.match(verifyIco(tampered).join(';'), /缺 16x16/);
  assert.match(verifyIco(tampered).join(';'), /與 IHDR 16x16 不一致/);

  assert.match(verifyIco(Buffer.from('not an ico')).join(';'), /ICONDIR/);

  // entry 內嵌的不是 PNG（例如 BMP／任意位元組）：把 24px 那份影像資料的前 8 位元組（PNG 簽章）蓋掉
  const notPng = Buffer.from(buildIco(fullSet()));
  const e24 = parseIco(notPng).find((e) => e.width === 24);
  notPng.fill(0, e24.offset, e24.offset + 8);
  assert.match(verifyIco(notPng).join(';'), /（24px）內嵌影像不是 PNG/);
});

test('buildIco：PNG 實際尺寸與宣告不符、尺寸重複、超出範圍都拒絕', () => {
  assert.throws(() => buildIco([{ size: 32, png: fakePng(16, 16) }]), /實際是 16x16/);
  assert.throws(() => buildIco([{ size: 16, png: fakePng(16, 16) }, { size: 16, png: fakePng(16, 16) }]), /重複/);
  assert.throws(() => buildIco([{ size: 512, png: fakePng(512, 512) }]), /超出/);
  assert.throws(() => readPngHeader(Buffer.alloc(40)), /不是 PNG/);
});

test('withSize：注入／覆蓋根元素 width、height，其他內容不動', () => {
  const a = withSize('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect/></svg>', 20);
  assert.equal(a, '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="20" height="20"><rect/></svg>');
  const b = withSize('<svg width="64" height="64" viewBox="0 0 64 64"><g/></svg>', 16);
  assert.equal(b, '<svg viewBox="0 0 64 64" width="16" height="16"><g/></svg>');
});

test('按尺寸選來源 SVG：統一圖示 16／20 用 c-16、24 用 c-24、>=32 用 c-sunrise-page', () => {
  const icon = buildOutputs('unused', () => false).find((o) => o.name === 'icon');
  assert.equal(icon.file, 'icon.ico');
  const got = Object.fromEntries(REQUIRED_SIZES.map((s) => [s, icon.pick(s)]));
  assert.deepEqual(got, {
    16: 'c-16.svg',
    20: 'c-16.svg',
    24: 'c-24.svg',
    32: 'c-sunrise-page.svg',
    40: 'c-sunrise-page.svg',
    48: 'c-sunrise-page.svg',
    64: 'c-sunrise-page.svg',
    256: 'c-sunrise-page.svg',
  });
});

test('按尺寸選來源 SVG：主題有簡化版時 <=24 用簡化版、>=32 用原圖；沒有就一律原圖', () => {
  // ridgeline 只有 -16 簡化版；contour 有 -16 與 -24 專用版；tearoff 沒有簡化版
  const have = new Set(['theme-ridgeline-16.svg', 'theme-contour-16.svg', 'theme-contour-24.svg']);
  const outs = buildOutputs('unused', (f) => have.has(f));
  assert.deepEqual(outs.slice(1).map((o) => o.name), THEMES.map((id) => `theme-${id}`));
  const by = Object.fromEntries(outs.map((o) => [o.name, o]));
  for (const s of [16, 20, 24]) assert.equal(by['theme-ridgeline'].pick(s), 'theme-ridgeline-16.svg', `ridgeline ${s}`);
  for (const s of [32, 40, 48, 64, 256]) assert.equal(by['theme-ridgeline'].pick(s), 'theme-ridgeline.svg', `ridgeline ${s}`);
  assert.equal(by['theme-contour'].pick(16), 'theme-contour-16.svg');
  assert.equal(by['theme-contour'].pick(20), 'theme-contour-16.svg');
  assert.equal(by['theme-contour'].pick(24), 'theme-contour-24.svg', '有 -24 專用版時 24 優先用它');
  assert.equal(by['theme-contour'].pick(32), 'theme-contour.svg');
  for (const s of REQUIRED_SIZES) assert.equal(by['theme-tearoff'].pick(s), 'theme-tearoff.svg', `tearoff ${s}（無簡化版）`);
});

test('實際 host/icons/src 的簡化版配置：ridgeline、astrolabe、contour 用 -16，tearoff、skyline 用原圖', () => {
  const srcDir = fileURLToPath(new URL('../icons/src/', import.meta.url));
  const by = Object.fromEntries(buildOutputs(srcDir).map((o) => [o.name, o]));
  for (const id of ['ridgeline', 'astrolabe', 'contour']) {
    assert.equal(by[`theme-${id}`].pick(16), `theme-${id}-16.svg`);
    assert.equal(by[`theme-${id}`].pick(24), `theme-${id}-16.svg`);
    assert.equal(by[`theme-${id}`].pick(32), `theme-${id}.svg`);
  }
  for (const id of ['tearoff', 'skyline']) assert.equal(by[`theme-${id}`].pick(16), `theme-${id}.svg`);
});

test('進 git 的 .ico 都含 16、20、24、32、40、48、64、256 且 IHDR 與 entry 一致', () => {
  const dir = fileURLToPath(new URL('../icons/', import.meta.url));
  for (const f of ['icon', 'theme-astrolabe', 'theme-tearoff', 'theme-ridgeline', 'theme-contour', 'theme-skyline']) {
    const buf = readFileSync(`${dir}${f}.ico`);
    assert.deepEqual(verifyIco(buf), [], `${f}.ico`);
    assert.deepEqual(parseIco(buf).map((e) => e.width), REQUIRED_SIZES, `${f}.ico 尺寸清單`);
  }
});

// ── dynamic-wallpaper task 5.2：執行期內嵌 PNG 與 favicon（由 .ico／統一圖示 SVG 衍生）──────────

test('extractPng：取出指定尺寸 entry 的 PNG 原始位元組；沒有該尺寸就丟錯', () => {
  const images = fullSet();
  const ico = buildIco(images);
  for (const { size, png } of images) assert.ok(extractPng(ico, size).equals(png), `${size}px`);
  assert.throws(() => extractPng(ico, 128), /沒有 128px/);
});

test('planDerived：每個圖示 7 個執行期 PNG＋favicon 兩檔，路徑與內容來自對應 .ico／統一圖示 SVG', () => {
  const icos = new Map(OUTPUT_NAMES.map((name, i) => [name, buildIco(REQUIRED_SIZES.map((size) => ({ size, png: fakePng(size, size, 6) })).map((im) => ({ ...im, png: Buffer.concat([im.png, Buffer.from([i])]) })))]));
  // 每個圖示的 PNG 尾端多一個位元組，用來辨認內容來自哪個 .ico（readPngHeader 只看檔頭，不受影響）
  const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect/></svg>';
  const plan = planDerived({ readIco: (name) => icos.get(name), readSvg: (file) => (file === 'c-sunrise-page.svg' ? svg : null) });
  assert.deepEqual(RUNTIME_PNG_SIZES, [16, 20, 24, 32, 40, 48, 64]);
  assert.equal(plan.length, OUTPUT_NAMES.length * RUNTIME_PNG_SIZES.length + 2);
  for (const [i, name] of OUTPUT_NAMES.entries()) {
    for (const size of RUNTIME_PNG_SIZES) {
      const item = plan.find((p) => p.path === `host/icons/png/${name}-${size}.png`);
      assert.ok(item, `${name}-${size}`);
      assert.ok(item.data.equals(extractPng(icos.get(name), size)), `${name}-${size} 內容`);
      assert.equal(item.data[item.data.length - 1], i);
    }
  }
  const svgOut = plan.find((p) => p.path === 'docs/favicon.svg');
  assert.equal(svgOut.data.toString('utf8'), withSize(svg, 64));
  const pngOut = plan.find((p) => p.path === 'docs/favicon.png');
  assert.ok(pngOut.data.equals(extractPng(icos.get('icon'), 64)), 'favicon.png＝統一圖示的 64px');
});

test('進 git 的執行期 PNG 與 favicon 都和 .ico／統一圖示 SVG 一致（改 .ico 後忘了重跑 --derive 會失敗）', () => {
  const root = fileURLToPath(new URL('../../', import.meta.url));
  const plan = planDerived({
    readIco: (name) => readFileSync(`${root}host/icons/${name}.ico`),
    readSvg: (file) => readFileSync(`${root}host/icons/src/${file}`, 'utf8'),
  });
  for (const { path: p, data } of plan) {
    assert.ok(readFileSync(`${root}${p}`).equals(data), `${p} 與衍生結果不一致`);
  }
});

test('著陸頁的 favicon 連結：svg 與 64x64 png 兩條都在', () => {
  const html = readFileSync(fileURLToPath(new URL('../../docs/index.html', import.meta.url)), 'utf8');
  assert.match(html, /<link rel="icon" type="image\/svg\+xml" href="favicon\.svg">/);
  assert.match(html, /<link rel="icon" type="image\/png" sizes="64x64" href="favicon\.png">/);
});
