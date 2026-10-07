// host/tools/lib/icon-sources.mjs
//
// make-icons.mjs 的純邏輯（無 Edge、無 I/O 副作用）：輸出清單、按尺寸挑來源 SVG 的規則、
// SVG 根元素尺寸注入。獨立成模組是為了讓單元測試（host/tests/ico.test.mjs）能直接 import，
// 而 make-icons.mjs 本身是無條件執行 main 的純 CLI（不靠「是否為主程式」判斷——經 junction／
// symlink 路徑執行時那種判斷會不成立，--verify 就會零輸出、exit 0 假通過）。

import { existsSync } from 'node:fs';
import path from 'node:path';
import { extractPng } from './ico.mjs';

export const THEMES = ['astrolabe', 'tearoff', 'ridgeline', 'contour', 'skyline'];

/**
 * 輸出清單：{ name, file（.ico 檔名）, pick(size)→來源 SVG 檔名 }。
 *   統一圖示：>=32 用 c-sunrise-page.svg、24 用 c-24.svg、16／20 用 c-16.svg。
 *   主題圖示：>=32 用 theme-<id>.svg；<=24 優先簡化版（24 先找 -24 再找 -16，16／20 找 -16），
 *     找不到簡化版就用原圖。`exists(檔名)` 預設查 srcDir 下的實際檔案，測試可注入。
 */
export function buildOutputs(srcDir, exists = (f) => existsSync(path.join(srcDir, f))) {
  return [
    {
      name: 'icon',
      file: 'icon.ico',
      pick: (size) => (size >= 32 ? 'c-sunrise-page.svg' : size === 24 ? 'c-24.svg' : 'c-16.svg'),
    },
    ...THEMES.map((id) => ({
      name: `theme-${id}`,
      file: `theme-${id}.ico`,
      pick: (size) => {
        const base = `theme-${id}.svg`;
        if (size >= 32) return base;
        const candidates = size === 24 ? [`theme-${id}-24.svg`, `theme-${id}-16.svg`] : [`theme-${id}-16.svg`];
        return candidates.find((f) => exists(f)) ?? base;
      },
    })),
  ];
}

/** 在 SVG 根元素注入 width／height（已有就換掉），讓點陣化以目標尺寸向量繪製。 */
export function withSize(svgText, size) {
  return svgText.replace(/<svg\b([^>]*)>/, (_m, attrs) => {
    const cleaned = attrs.replace(/\s(width|height)\s*=\s*"[^"]*"/g, '');
    return `<svg${cleaned} width="${size}" height="${size}">`;
  });
}

/** 輸出清單的名稱（＝.ico 檔名主幹），順序同 {@link buildOutputs}。 */
export const OUTPUT_NAMES = ['icon', ...THEMES.map((id) => `theme-${id}`)];

/**
 * dynamic-wallpaper task 5.2：執行期以 `tauri::include_image!` 內嵌的尺寸（系統匣 16×縮放、設定視窗
 * 32×縮放，見 host/src/app_icon.rs）。256 不內嵌——include_image! 存的是未壓縮 RGBA，256 一張就 256 KB。
 */
export const RUNTIME_PNG_SIZES = [16, 20, 24, 32, 40, 48, 64];

/**
 * 由已產生的 .ico 衍生的檔案（不需要 Edge）：
 *   - `host/icons/png/<name>-<size>.png`：各 .ico 對應尺寸 entry 的 PNG 原始位元組（與 .ico 逐位元組相同，
 *     16／20／24 因此是簡化版造型）；
 *   - `docs/favicon.svg`：統一圖示完整造型 SVG（c-sunrise-page.svg，根元素注入 64×64）；
 *   - `docs/favicon.png`：統一圖示 .ico 的 64px entry。
 * `readIco(name)`→Buffer、`readSvg(檔名)`→string 由呼叫端提供（測試可注入）。回傳 [{ path（相對 repo 根，
 * 斜線分隔）, data: Buffer }]。
 */
export function planDerived({ readIco, readSvg }) {
  const out = [];
  for (const name of OUTPUT_NAMES) {
    const ico = readIco(name);
    for (const size of RUNTIME_PNG_SIZES) {
      out.push({ path: `host/icons/png/${name}-${size}.png`, data: extractPng(ico, size) });
    }
  }
  out.push({ path: 'docs/favicon.svg', data: Buffer.from(withSize(readSvg('c-sunrise-page.svg'), 64), 'utf8') });
  out.push({ path: 'docs/favicon.png', data: extractPng(readIco('icon'), 64) });
  return out;
}
