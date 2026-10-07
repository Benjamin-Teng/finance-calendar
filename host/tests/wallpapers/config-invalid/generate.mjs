// host/tests/wallpapers/config-invalid/generate.mjs
//
// 從預設檔各複製一份、只植入「一個」變更，產生 schema 驗證用的樣本（task 3.2）：
//   config-invalid/*.json  負向樣本，每份只有一個缺陷，schema 必須擋下
//   config-valid/*.json    正向樣本，schema 必須放行（使用者合法的編輯方式）
// 預設檔結構改了之後重跑一次：
//   node host/tests/wallpapers/config-invalid/generate.mjs
// 驗證用 host/tests/wallpapers/check-config-schema.sh。

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const validDir = path.join(here, '..', 'config-valid');
const defaultPath = path.join(here, '..', '..', '..', 'ui', 'wallpapers', 'config', 'wallpaper-config.default.json');
const base = () => JSON.parse(readFileSync(defaultPath, 'utf8'));

const invalid = {
  // 日期格式錯：休市日寫成 2026/12/25（必須是 YYYY-MM-DD）
  'bad-date-format.json': (c) => {
    c.holidays.NYC['2026'].days.at(-1).date = '2026/12/25';
  },
  // 不存在的日期：2026-02-30（格式對、日曆上沒有；靠 format: date 擋）
  'nonexistent-date.json': (c) => {
    c.holidays.NYC['2026'].days.at(-1).date = '2026-02-30';
  },
  // 農曆正月初一表（task 3.4）：日期寫成 2027/02/06（必須是 YYYY-MM-DD）
  'lunar-new-year-bad-date.json': (c) => {
    c.lunarNewYear.dates['2027'].date = '2027/02/06';
  },
  // 缺必填：整個 thresholds 不見
  'missing-required.json': (c) => {
    delete c.thresholds;
  },
  // 門檻為負：融資維持率 -150
  'negative-threshold.json': (c) => {
    c.thresholds.maintenanceRatio = -150;
  },
  // 時間格式錯：時段起點 25:00（必須是 HH:MM、00–23）
  'bad-time-format.json': (c) => {
    c.markets[0].segments[0].start = '25:00';
  },
  // 半日市沒有收盤時間
  'half-day-without-close.json': (c) => {
    const d = c.holidays.NYC['2026'].days.find((x) => x.half);
    delete d.close;
  },
  // 休市表的市場 id 打錯（TPI，不在六市場 enum 內）
  'unknown-market-id.json': (c) => {
    c.holidays.TPI = c.holidays.TPE;
    delete c.holidays.TPE;
  },
  // 巢狀物件的未知鍵（拼錯 threshold 的鍵名）：只有頂層允許未知鍵
  'unknown-nested-key.json': (c) => {
    c.thresholds.shortRatioo = 2;
  },
};

const valid = {
  // 頂層未知鍵＋使用者自補的休市日省略 src（schema 允許）
  'extra-top-level-key-and-no-src.json': (c) => {
    c.myNotes = { anything: ['goes', 1, true] };
    c.holidays.NYC['2026'].days.push({ date: '2026-12-28', name: '自訂休市日' });
  },
  // 待複核旗標
  'verified-flag.json': (c) => {
    c.holidays.TYO['2027'].verified = false;
  },
};

mkdirSync(validDir, { recursive: true });
for (const [name, mutate] of Object.entries(invalid)) {
  const c = base();
  mutate(c);
  writeFileSync(path.join(here, name), JSON.stringify(c, null, 2) + '\n', 'utf8');
  console.log('wrote config-invalid/' + name);
}
for (const [name, mutate] of Object.entries(valid)) {
  const c = base();
  mutate(c);
  writeFileSync(path.join(validDir, name), JSON.stringify(c, null, 2) + '\n', 'utf8');
  console.log('wrote config-valid/' + name);
}
