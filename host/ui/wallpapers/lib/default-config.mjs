// host/ui/wallpapers/lib/default-config.mjs
//
// 讀內建預設設定檔 config/wallpaper-config.default.json（與 4.1 編進宿主的是同一份）。撕日曆（3.4）、
// 脊線與等高線（3.5）共用；星盤（3.3）仍用自己的一份，未改動。需要 fetch（瀏覽器），Node 測試請直接讀檔。

/** 內建預設設定檔的網址（相對本模組）。 */
export const DEFAULT_CONFIG_URL = new URL('../config/wallpaper-config.default.json', import.meta.url).href;

/** 讀內建預設設定檔；讀不到就拋錯（整張失敗、宿主保留舊圖）。 */
export async function loadDefaultConfig(url = DEFAULT_CONFIG_URL) {
  let res;
  try {
    res = await fetch(url, { cache: 'no-store' });
  } catch (e) {
    throw new Error(`內建預設設定讀取失敗：${url}（${e?.message || e}）`);
  }
  if (!res.ok) {
    throw new Error(`內建預設設定讀取失敗：HTTP ${res.status} ${url}`);
  }
  return res.json();
}
