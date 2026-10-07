// host/tools/make-icons.mjs
//
// dynamic-wallpaper task 5.1（design.md D9）：由 host/icons/src/*.svg 產生 .ico。
//
//   node host/tools/make-icons.mjs            產生 host/icons/*.ico ＋ 16/20/24px 放大對照圖，再驗證
//   node host/tools/make-icons.mjs --verify   只解析既有 .ico 檔頭並驗證（不需要 Edge）
//   node host/tools/make-icons.mjs --derive   只由既有 .ico 重寫衍生檔（不需要 Edge），再驗證
//
// 衍生檔（task 5.2，lib/icon-sources.mjs 的 planDerived）：host/icons/png/<name>-<size>.png（執行期
// 以 tauri::include_image! 內嵌，系統匣與設定視窗圖示用）、docs/favicon.svg、docs/favicon.png。
// 產生模式寫完 .ico 後也會重寫衍生檔；--verify 會逐位元組核對衍生檔與 .ico／SVG 一致。
//
// 流程：headless Edge（沿用 host/tests/compare/cdp.mjs，獨立 --user-data-dir、結束時只關自己啟動的
// 那個行程）在頁面內以 <canvas> 用「精確尺寸」繪製 SVG 並匯出 PNG——不用 --screenshot：headless
// Edge 的 viewport 比視窗小約 30px、寬度下限約 492px，精確像素量不準。SVG 根元素會先注入
// width／height＝目標尺寸，讓瀏覽器直接以該尺寸向量點陣化（不是先畫大圖再縮）。透明背景保留 alpha。
// 封裝由 lib/ico.mjs 負責（PNG 內嵌式 ICO，256 的寬高欄位寫 0）。
//
// 來源選擇（每個尺寸各點陣化一次）：
//   統一圖示（icon.ico）：>=32 用 c-sunrise-page.svg、16／20 用 c-16.svg、24 用 c-24.svg。
//   主題圖示（theme-<id>.ico）：<=24 時優先用 theme-<id>-16.svg（簡化版，16／20）或
//     theme-<id>-24.svg（24），沒有簡化版就用 theme-<id>.svg；>=32 一律用 theme-<id>.svg。
// 產物進 git，建置（cargo）不依賴本指令稿；只有改 SVG 或尺寸表時才需要重跑。

import { existsSync, readFileSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { CDPTarget, closeTarget, launchEdge, newTarget } from './../tests/compare/cdp.mjs';
import { REQUIRED_SIZES, buildIco, parseIco, verifyIco } from './lib/ico.mjs';
import { buildOutputs, planDerived, withSize } from './lib/icon-sources.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ICONS_DIR = path.resolve(HERE, '..', 'icons');
const REPO_ROOT = path.resolve(HERE, '..', '..');
const SRC_DIR = path.join(ICONS_DIR, 'src');
const EVIDENCE_DIR = path.join(HERE, 'evidence');

const SHEET_SIZES = [16, 20, 24];
const SHEET_SCALE = 8;
const OUTPUTS = buildOutputs(SRC_DIR);

// 頁面端：把 SVG 文字以精確尺寸畫進 canvas，回傳 PNG dataURL。
const RASTER_JS = (svg, size) => `(async () => {
  const svg = ${JSON.stringify(svg)};
  const size = ${size};
  const img = new Image();
  img.src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
  await img.decode();
  const c = document.createElement('canvas');
  c.width = size; c.height = size;
  const g = c.getContext('2d');
  g.clearRect(0, 0, size, size);
  g.drawImage(img, 0, 0, size, size);
  return c.toDataURL('image/png');
})()`;

// 頁面端：把各圖示 16／20／24px 的 PNG 以最近鄰放大，深色底＋淺色底各一欄組，拼成一張對照圖。
const SHEET_JS = (items) => `(async () => {
  const items = ${JSON.stringify(items)};
  const sizes = ${JSON.stringify(SHEET_SIZES)};
  const S = ${SHEET_SCALE};
  const pad = 12, labelW = 150, groupGap = 24;
  const cellW = sizes.reduce((a, s) => a + s * S + pad, pad);
  const rowH = 24 * S + pad * 2;
  const W = labelW + cellW * 2 + groupGap;
  const H = items.length * rowH + pad;
  const c = document.createElement('canvas');
  c.width = W; c.height = H;
  const g = c.getContext('2d');
  g.fillStyle = '#10141c'; g.fillRect(0, 0, W, H);
  g.fillStyle = '#f2f2ee'; g.fillRect(labelW + cellW + groupGap, 0, cellW, H);
  const load = (u) => new Promise((res, rej) => { const i = new Image(); i.onload = () => res(i); i.onerror = rej; i.src = u; });
  g.imageSmoothingEnabled = false;
  for (let r = 0; r < items.length; r++) {
    const y = pad + r * rowH;
    g.fillStyle = '#dfe6f0'; g.font = '16px sans-serif'; g.textBaseline = 'middle';
    g.fillText(items[r].name, 8, y + (24 * S) / 2);
    for (let grp = 0; grp < 2; grp++) {
      let x = labelW + grp * (cellW + groupGap) + pad;
      for (const s of sizes) {
        const im = await load(items[r].pngs[s]);
        g.imageSmoothingEnabled = false;
        g.drawImage(im, x, y, s * S, s * S);
        x += s * S + pad;
      }
    }
  }
  return c.toDataURL('image/png');
})()`;

function dataUrlToBuffer(url) {
  const m = /^data:image\/png;base64,(.+)$/.exec(url);
  if (!m) throw new Error('canvas 沒有回傳 PNG dataURL');
  return Buffer.from(m[1], 'base64');
}

async function generate() {
  const edge = await launchEdge();
  let targetInfo;
  let cdp;
  try {
    targetInfo = await newTarget(edge.port, 'about:blank');
    cdp = new CDPTarget(targetInfo.webSocketDebuggerUrl, { commandTimeoutMs: 20000 });
    await cdp.connect();
    const evalJs = async (expression) => {
      const r = await cdp.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (r.exceptionDetails) {
        throw new Error(`頁面例外：${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
      }
      return r.result.value;
    };

    const svgCache = new Map();
    const readSvg = async (file) => {
      if (!svgCache.has(file)) svgCache.set(file, await readFile(path.join(SRC_DIR, file), 'utf8'));
      return svgCache.get(file);
    };

    const sheetItems = [];
    for (const out of OUTPUTS) {
      const images = [];
      const pngs = {};
      const used = [];
      for (const size of REQUIRED_SIZES) {
        const file = out.pick(size);
        const png = dataUrlToBuffer(await evalJs(RASTER_JS(withSize(await readSvg(file), size), size)));
        images.push({ size, png });
        used.push(`${size}:${file}`);
        if (SHEET_SIZES.includes(size)) pngs[size] = `data:image/png;base64,${png.toString('base64')}`;
      }
      const ico = buildIco(images);
      await writeFile(path.join(ICONS_DIR, out.file), ico);
      console.log(`寫入 host/icons/${out.file}（${ico.length} bytes）  來源 ${used.join(' ')}`);
      sheetItems.push({ name: out.name, pngs });
    }

    await mkdir(EVIDENCE_DIR, { recursive: true });
    const sheet = dataUrlToBuffer(await evalJs(SHEET_JS(sheetItems)));
    const sheetPath = path.join(EVIDENCE_DIR, 'icons-5.1-zoom-16-20-24.png');
    await writeFile(sheetPath, sheet);
    console.log(`寫入 ${path.relative(process.cwd(), sheetPath)}（16／20／24px 各放大 ${SHEET_SCALE} 倍，深色底＋淺色底）`);
    // 每套各一張，方便逐張檢視
    for (const item of sheetItems) {
      const one = dataUrlToBuffer(await evalJs(SHEET_JS([item])));
      const p = path.join(EVIDENCE_DIR, `icons-5.1-${item.name}.png`);
      await writeFile(p, one);
    }
  } finally {
    cdp?.close();
    if (targetInfo) await closeTarget(edge.port, targetInfo.id);
    await edge.close();
  }
}

async function readDerivedPlan() {
  return planDerived({
    readIco: (name) => readFileSync(path.join(ICONS_DIR, `${name}.ico`)),
    readSvg: (file) => readFileSync(path.join(SRC_DIR, file), 'utf8'),
  });
}

async function derive() {
  const plan = await readDerivedPlan();
  for (const { path: rel, data } of plan) {
    const abs = path.join(REPO_ROOT, rel);
    await mkdir(path.dirname(abs), { recursive: true });
    await writeFile(abs, data);
  }
  console.log(`寫入衍生檔 ${plan.length} 個（host/icons/png/*.png、docs/favicon.svg、docs/favicon.png）`);
}

async function verifyDerived() {
  let bad = 0;
  let plan;
  try {
    plan = await readDerivedPlan();
  } catch (e) {
    console.log(`FAIL  衍生檔：無法由 .ico／SVG 衍生（${e.message}）`);
    return 1;
  }
  for (const { path: rel, data } of plan) {
    const abs = path.join(REPO_ROOT, rel);
    if (!existsSync(abs)) {
      console.log(`FAIL  ${rel}：檔案不存在（跑 --derive）`);
      bad++;
    } else if (!readFileSync(abs).equals(data)) {
      console.log(`FAIL  ${rel}：與 .ico／SVG 衍生結果不一致（跑 --derive）`);
      bad++;
    }
  }
  if (bad === 0) console.log(`PASS  衍生檔 ${plan.length} 個與 .ico／SVG 逐位元組一致`);
  return bad;
}

async function verify() {
  let bad = 0;
  for (const out of OUTPUTS) {
    const file = path.join(ICONS_DIR, out.file);
    if (!existsSync(file)) {
      console.log(`FAIL  ${out.file}：檔案不存在`);
      bad++;
      continue;
    }
    const buf = await readFile(file);
    const problems = verifyIco(buf);
    const sizes = (() => {
      try {
        return parseIco(buf).map((e) => e.width).join(',');
      } catch {
        return '?';
      }
    })();
    if (problems.length) {
      console.log(`FAIL  ${out.file}  尺寸 [${sizes}]：${problems.join('；')}`);
      bad++;
    } else {
      console.log(`PASS  ${out.file}  尺寸 [${sizes}]  需要 [${REQUIRED_SIZES.join(',')}] 全含，IHDR 與 entry 一致`);
    }
  }
  return bad + (await verifyDerived());
}

// 無條件執行（不判斷是否為主程式：經 junction／symlink 路徑執行時 argv[1] 與 import.meta.url 不相等，
// 判斷會讓 --verify 零輸出就 exit 0）。純邏輯在 lib/icon-sources.mjs、lib/ico.mjs，供單元測試 import。
const verifyOnly = process.argv.includes('--verify');
const deriveOnly = process.argv.includes('--derive');
try {
  if (!verifyOnly && !deriveOnly) await generate();
  if (!verifyOnly) await derive();
  const bad = await verify();
  process.exit(bad === 0 ? 0 : 1);
} catch (e) {
  console.error(`make-icons 失敗：${e.stack ?? e}`);
  process.exit(1);
}
