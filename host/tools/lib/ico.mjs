// host/tools/lib/ico.mjs
//
// PNG 內嵌式 ICO 的封裝與檔頭解析（dynamic-wallpaper task 5.1，design.md D9）。
// 零安裝：只用 Node 內建能力。make-icons.mjs 產生與驗證都走這支；單元測試是
// host/tests/ico.test.mjs。
//
// ICO 結構（little-endian）：
//   ICONDIR      6 bytes：reserved(2)=0、type(2)=1、count(2)
//   ICONDIRENTRY 16 bytes × count：width(1)、height(1)（256 寫 0）、colorCount(1)=0、
//                reserved(1)=0、planes(2)=1、bitCount(2)=32、bytesInRes(4)、imageOffset(4)
//   影像資料：每個 entry 一份完整 PNG（Vista 以上的 shell 與 rc.exe 都吃 PNG 內嵌）

export const REQUIRED_SIZES = [16, 20, 24, 32, 40, 48, 64, 256];

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

/** 讀 PNG 的 IHDR；回傳 { width, height, bitDepth, colorType }，不合法就丟錯。 */
export function readPngHeader(png) {
  if (png.length < 33 || !png.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error('不是 PNG（簽章不符）');
  }
  if (png.readUInt32BE(8) !== 13 || png.toString('latin1', 12, 16) !== 'IHDR') {
    throw new Error('PNG 第一個 chunk 不是 IHDR');
  }
  return {
    width: png.readUInt32BE(16),
    height: png.readUInt32BE(20),
    bitDepth: png[24],
    colorType: png[25],
  };
}

/**
 * 把 [{ size, png }] 封成 ICO（依尺寸由小到大排列）。size 必須是 1–256，且 PNG 的 IHDR 寬高
 * 必須等於 size（避免封進錯尺寸的點陣圖）。
 */
export function buildIco(images) {
  if (images.length === 0) throw new Error('沒有任何影像');
  const sorted = [...images].sort((a, b) => a.size - b.size);
  const seen = new Set();
  for (const { size, png } of sorted) {
    if (!Number.isInteger(size) || size < 1 || size > 256) throw new Error(`尺寸 ${size} 超出 1–256`);
    if (seen.has(size)) throw new Error(`尺寸 ${size} 重複`);
    seen.add(size);
    const h = readPngHeader(png);
    if (h.width !== size || h.height !== size) {
      throw new Error(`${size}px 的 PNG 實際是 ${h.width}x${h.height}`);
    }
  }
  const headerBytes = 6 + 16 * sorted.length;
  const out = Buffer.alloc(headerBytes);
  out.writeUInt16LE(0, 0);
  out.writeUInt16LE(1, 2);
  out.writeUInt16LE(sorted.length, 4);
  let offset = headerBytes;
  sorted.forEach(({ size, png }, i) => {
    const e = 6 + 16 * i;
    out[e] = size === 256 ? 0 : size;
    out[e + 1] = size === 256 ? 0 : size;
    out[e + 2] = 0;
    out[e + 3] = 0;
    out.writeUInt16LE(1, e + 4);
    out.writeUInt16LE(32, e + 6);
    out.writeUInt32LE(png.length, e + 8);
    out.writeUInt32LE(offset, e + 12);
    offset += png.length;
  });
  return Buffer.concat([out, ...sorted.map((s) => s.png)]);
}

/**
 * 解析 ICO 檔頭，回傳 [{ width, height, bitCount, bytes, offset, png }]（width／height 的 0
 * 已換成 256；png 是內嵌影像的 IHDR 結果或 null）。格式不對就丟錯。
 */
export function parseIco(buf) {
  if (buf.length < 6) throw new Error('檔案小於 ICONDIR');
  if (buf.readUInt16LE(0) !== 0 || buf.readUInt16LE(2) !== 1) {
    throw new Error('ICONDIR 的 reserved／type 不是 0／1');
  }
  const count = buf.readUInt16LE(4);
  if (buf.length < 6 + 16 * count) throw new Error('檔案小於 ICONDIRENTRY 表');
  const entries = [];
  for (let i = 0; i < count; i++) {
    const e = 6 + 16 * i;
    const bytes = buf.readUInt32LE(e + 8);
    const offset = buf.readUInt32LE(e + 12);
    if (offset + bytes > buf.length) throw new Error(`entry ${i} 的影像資料超出檔案`);
    const data = buf.subarray(offset, offset + bytes);
    let png = null;
    try {
      png = readPngHeader(data);
    } catch {
      png = null;
    }
    entries.push({
      width: buf[e] === 0 ? 256 : buf[e],
      height: buf[e + 1] === 0 ? 256 : buf[e + 1],
      bitCount: buf.readUInt16LE(e + 6),
      bytes,
      offset,
      png,
    });
  }
  return entries;
}

/**
 * 驗證一個 ICO：含 `required` 內每個尺寸、無多餘重複、每個 entry 都是 PNG 且 IHDR 寬高
 * 與 entry 一致、PNG 為 RGBA 8-bit（colorType 6）。回傳問題字串陣列（空＝通過）。
 */
export function verifyIco(buf, required = REQUIRED_SIZES) {
  const problems = [];
  let entries;
  try {
    entries = parseIco(buf);
  } catch (e) {
    return [e.message];
  }
  const sizes = entries.map((e) => e.width);
  for (const s of required) {
    if (!entries.some((e) => e.width === s && e.height === s)) problems.push(`缺 ${s}x${s}`);
  }
  if (new Set(sizes).size !== sizes.length) problems.push(`尺寸重複：${sizes.join(',')}`);
  entries.forEach((e, i) => {
    if (e.width !== e.height) problems.push(`entry ${i} 不是正方形：${e.width}x${e.height}`);
    if (!e.png) {
      problems.push(`entry ${i}（${e.width}px）內嵌影像不是 PNG`);
      return;
    }
    if (e.png.width !== e.width || e.png.height !== e.height) {
      problems.push(`entry ${i} 檔頭 ${e.width}x${e.height} 與 IHDR ${e.png.width}x${e.png.height} 不一致`);
    }
    if (e.png.colorType !== 6 || e.png.bitDepth !== 8) {
      problems.push(`entry ${i}（${e.width}px）不是 RGBA 8-bit（colorType=${e.png.colorType} bitDepth=${e.png.bitDepth}）`);
    }
  });
  return problems;
}

/** 取出 ICO 中 `size`×`size` 那個 entry 的 PNG 原始位元組（task 5.2：執行期內嵌與 favicon 用）。 */
export function extractPng(buf, size) {
  const entry = parseIco(buf).find((e) => e.width === size && e.height === size);
  if (!entry) throw new Error(`ICO 裡沒有 ${size}px 的 entry`);
  return Buffer.from(buf.subarray(entry.offset, entry.offset + entry.bytes));
}
