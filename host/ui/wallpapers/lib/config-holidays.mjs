// host/ui/wallpapers/lib/config-holidays.mjs
//
// 主題設定檔（config/wallpaper-config.default.json）的休市表語意與讀取端檢查（task 3.2 修正輪）。
// ES module、不依賴 DOM，可在 Node 測試。星盤（3.3）、撕日曆（3.4）、宿主設定視窗（4.8）與
// 設定檔讀取（4.1 的「警告並退回預設」）共用，避免各自發明一套。
//
// ## 休市表語意（controller 裁定；完整說明見 config/README.md）
//   a. `days[].date` 是「交易所當地」日曆日（以 markets[].tz 判定），不是台北或本機日期。
//   b. `days` 內明列的日期一律算休市日或半日市，不受 `through` 限制；`through` 只說明官方日曆已知到哪天。
//   c. 晚於 `through`（或該年度鍵不存在）且未列出的日期是「未知」：畫成一般交易日、不變暗，
//      只用於到期提醒。`holidayOn()` 回 status 'unknown' 讓呼叫端分辨，但畫面上等同 'open'。
//   d. 年底提醒：本機日期距 12/31 不足 30 天（12/02 起；12/01 剛好 30 天不算），且任一市場的最晚 `through`
//      早於「明年」12/31 → 提醒（spec「距年底不足 30 天」）。
//   e. 使用者在 `through` 之後新增休市日時應同時延伸 `through`；沒延伸時日期仍算休市，
//      `validateThemeConfig()` 只回警告。
//   f. 半日市：當天非 night 的時段一律截到 `close`；起點 ≥ `close` 的非 night 時段不畫（自然排除收盤 auction
//      與 post、保留開盤 auction）；night（期貨）時段照畫。期貨半日市收盤時間（如 HKEX 12:30）不建模，屬已知簡化。
//   g. 跨午夜的時段以「起點」的交易所當地日期查表：起點那天休市＝整段不畫，否則整段照畫（即使跨進休市日），
//      屬已知簡化。星盤的實作在 astrolabe-sessions.mjs。
//
// ## 函式
//   validateThemeConfig(config)            → string[] 警告（不拋錯）；由 mergeThemeConfig 在合併後呼叫
//   mergeThemeConfig(defaults, config)     → { value, warnings }：`mergeConfig`＋`validateThemeConfig`。繪圖頁的
//                                            `env.withDefaults`（wallpaper.mjs）一律走這裡（task 4.1：巢狀驗證的唯一呼叫點）
//   holidayOn(config, marketId, localDate, warnings?) → { status: 'closed'|'half'|'open'|'unknown', name?, close?, src? }
//   holidayOnAt(config, marketId, nowMs, warnings?)   → 同上；由 markets[].tz 把 UTC 毫秒換成交易所當地日期再查
//   holidayExpiryReminder(config, todayLocal, warnings?) → string[]：需要更新休市表的市場 id（空陣列＝不用提醒）
//   unverifiedHolidayYears(config)         → { market, year }[]：標了 verified:false 的年度
//   lunarNewYearOf(config, year)           → 'YYYY-MM-DD'｜null：設定檔的農曆正月初一（撕日曆干支換年日，task 3.4）
//   PHRASE_MAX_CHARS                       撕日曆字句建議上限 20 字；validateThemeConfig 對超過者回警告
//
// ## 查表函式容錯（task 4.1，3.2 延後項 N3）
//   宿主（Rust）只做頂層鍵合併、頁面 `mergeConfig` 對陣列整份取代，所以 `markets:[null]`、`days:[null]`、
//   打錯的 tz 都可能原樣到這裡。查表函式一律不拋錯：不合法的元素略過、tz 不認得或時刻不是有限數字時回
//   `{ status: 'unknown' }`（畫面上等同開市），並把訊息加進選填的 `warnings`（陣列或 Set；省略＝不收集）。

import { isValidTz, mergeConfig, tzParts } from './core.mjs';

const DATE_RE = /^(\d{4})-(\d{2})-(\d{2})$/;
const MONTH_DAY_RE = /^(\d{2})-(\d{2})$/;
const TIME_RE = /^([01]\d|2[0-3]):[0-5]\d$/;

const isPlain = (v) => v !== null && typeof v === 'object' && !Array.isArray(v);

/** 把警告加進呼叫端給的收集器（陣列不重複加入；Set 本身去重）；沒給收集器就略過。 */
function note(warnings, msg) {
  if (warnings instanceof Set) warnings.add(msg);
  else if (Array.isArray(warnings) && !warnings.includes(msg)) warnings.push(msg);
}

/** `markets` 中的合法市場（物件且 id 為非空字串）；其餘略過並記警告。不是陣列回空陣列並記警告。 */
function usableMarkets(config, warnings) {
  const list = config?.markets;
  if (!Array.isArray(list)) {
    if (list !== undefined) note(warnings, 'config.markets 型別應為陣列，視為沒有任何市場');
    return [];
  }
  return list.filter((m, i) => {
    const ok = isPlain(m) && typeof m.id === 'string' && m.id !== '';
    if (!ok) note(warnings, `config.markets[${i}] 不是含 id 的物件，已略過`);
    return ok;
  });
}

/** 'YYYY-MM-DD' 是否為真實存在的日期（2026-02-30 → false）。 */
export function isRealDate(s) {
  const m = typeof s === 'string' ? DATE_RE.exec(s) : null;
  if (!m) return false;
  const [y, mo, d] = [+m[1], +m[2], +m[3]];
  const t = new Date(Date.UTC(y, mo - 1, d));
  return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === d;
}

/** 'MM-DD' 是否在某個閏年裡存在（02-29 視為存在，02-30 不存在）。 */
function isRealMonthDay(s) {
  const m = typeof s === 'string' ? MONTH_DAY_RE.exec(s) : null;
  return !!m && isRealDate(`2024-${s}`);
}

/** 撕日曆字句（宜忌詞庫、定稿字句、旺季標籤）建議的字數上限（task 3.4 修正輪 1；超過只警告）。 */
export const PHRASE_MAX_CHARS = 20;

/** 設定檔中會畫在撕日曆「宜」「忌」行上的字句：[位置, 字串]（非字串略過）。 */
function phraseEntries(config) {
  const ph = isPlain(config.phrases) ? config.phrases : {};
  const out = [];
  const add = (at, v) => {
    if (typeof v === 'string') out.push([at, v]);
  };
  add('config.phrases.closedDay.yi', ph.closedDay?.yi);
  (Array.isArray(ph.closedDay?.ji) ? ph.closedDay.ji : []).forEach((s, i) => add(`config.phrases.closedDay.ji[${i}]`, s));
  add('config.phrases.marginWarning.ji', ph.marginWarning?.ji);
  for (const [k, v] of Object.entries(isPlain(ph.events) ? ph.events : {})) add(`config.phrases.events.${k}`, v);
  for (const key of ['yiPool', 'jiPool']) {
    (Array.isArray(ph[key]) ? ph[key] : []).forEach((s, i) => add(`config.phrases.${key}[${i}]`, s));
  }
  (Array.isArray(config.orderSeasons) ? config.orderSeasons : []).forEach((s, i) => add(`config.orderSeasons[${i}].label`, s?.label));
  return out;
}

/**
 * 設定檔 `lunarNewYear.dates[year].date`（該西元年的農曆正月初一）；不存在或不合理（不是真實日期、
 * 不在該年 1/21–2/20）回傳 null，呼叫端退回 Intl 中國曆。`year` 可為數字或字串。
 */
export function lunarNewYearOf(config, year) {
  const e = config?.lunarNewYear?.dates?.[String(year)];
  const d = isPlain(e) ? e.date : undefined;
  if (!isRealDate(d) || !d.startsWith(`${year}-`)) return null;
  const md = d.slice(5);
  return md >= '01-21' && md <= '02-20' ? d : null;
}

/**
 * 頁面的主題設定合併＋巢狀驗證（task 4.1）：`mergeConfig(defaults, config)` 後對結果跑
 * `validateThemeConfig`，回傳 `{ value, warnings }`（合併警告在前、驗證警告在後）。宿主 Rust 只做頂層鍵
 * 合併，巢狀內容的錯誤只有這裡會發現；`wallpaper.mjs` 的 `env.withDefaults` 經 `env.warn` 回報宿主。
 */
export function mergeThemeConfig(defaults, config) {
  const merged = mergeConfig(defaults, config ?? {});
  return { value: merged.value, warnings: [...merged.warnings, ...validateThemeConfig(merged.value)] };
}

/**
 * 檢查設定檔中 JSON schema 表達不了的部分，回傳警告字串陣列；不拋錯、不修改輸入。
 * 檢查：休市表（日期真實存在、年度鍵與日期年份相符、日期不晚於 through、市場 id 在 markets 內、
 * half／close 配對、src 在 holidaySources 內、日期不重複）、財報密集週截止日、門檻為正數、
 * 農曆正月初一表（日期合理、出處解得開）、撕日曆字句超過 PHRASE_MAX_CHARS 字。
 */
export function validateThemeConfig(config) {
  const w = [];
  if (!isPlain(config)) return ['config 型別應為物件'];

  const marketIds = new Set(Array.isArray(config.markets) ? config.markets.filter(isPlain).map((m) => m.id) : []);
  const sources = isPlain(config.holidaySources) ? config.holidaySources : {};

  // 市場表（task 4.1，N3）：元素型別、id、tz 是否為 Intl 認得的 IANA 時區（打錯會讓時區換算失敗）
  if (config.markets !== undefined && !Array.isArray(config.markets)) {
    w.push('config.markets 型別應為陣列');
  } else {
    (config.markets ?? []).forEach((m, i) => {
      const at = `config.markets[${i}]`;
      if (!isPlain(m)) {
        w.push(`${at} 型別應為物件`);
        return;
      }
      if (typeof m.id !== 'string' || m.id === '') w.push(`${at}.id 應為非空字串`);
      if (!isValidTz(m.tz)) w.push(`${at}.tz（${String(m.tz)}）不是 Intl 認得的 IANA 時區`);
    });
  }

  if (config.holidays !== undefined && !isPlain(config.holidays)) {
    w.push('config.holidays 型別應為物件');
  } else {
    for (const [mk, years] of Object.entries(config.holidays ?? {})) {
      if (marketIds.size > 0 && !marketIds.has(mk)) {
        w.push(`config.holidays.${mk}：市場不在 markets 內（打錯字？）`);
      }
      if (!isPlain(years)) {
        w.push(`config.holidays.${mk} 型別應為物件`);
        continue;
      }
      for (const [year, y] of Object.entries(years)) {
        const at = `config.holidays.${mk}.${year}`;
        if (!/^\d{4}$/.test(year)) w.push(`${at}：年度鍵應為四位數西元年`);
        if (!isPlain(y)) {
          w.push(`${at} 型別應為物件`);
          continue;
        }
        const through = y.through;
        const throughOk = isRealDate(through);
        if (!throughOk) w.push(`${at}.through 不是 YYYY-MM-DD 的真實日期`);
        else if (!through.startsWith(`${year}-`)) w.push(`${at}.through（${through}）不在 ${year} 年`);
        if (y.verified !== undefined && typeof y.verified !== 'boolean') w.push(`${at}.verified 應為布林值`);
        if (!Array.isArray(y.days)) {
          w.push(`${at}.days 型別應為陣列`);
          continue;
        }
        const seen = new Set();
        let beyond = 0;
        y.days.forEach((d, i) => {
          const da = `${at}.days[${i}]`;
          if (!isPlain(d)) {
            w.push(`${da} 型別應為物件`);
            return;
          }
          if (!isRealDate(d.date)) {
            w.push(`${da}.date（${String(d.date)}）不是 YYYY-MM-DD 的真實日期`);
          } else {
            if (!d.date.startsWith(`${year}-`)) w.push(`${da}.date（${d.date}）與年度鍵 ${year} 不符`);
            if (seen.has(d.date)) w.push(`${da}.date（${d.date}）重複`);
            seen.add(d.date);
            if (throughOk && d.date > through) beyond++;
          }
          if (typeof d.name !== 'string' || d.name === '') w.push(`${da}.name 應為非空字串`);
          if (d.half === true) {
            if (typeof d.close !== 'string' || !TIME_RE.test(d.close)) w.push(`${da}：半日市需要 HH:MM 格式的 close`);
          } else if (d.half !== undefined && d.half !== false) {
            w.push(`${da}.half 只能是 true 或不寫`);
          } else if (d.close !== undefined) {
            w.push(`${da}：非半日市不應有 close`);
          }
          if (d.src !== undefined && !Object.hasOwn(sources, d.src)) {
            w.push(`${da}.src（${String(d.src)}）不在 holidaySources 內`);
          }
        });
        if (beyond > 0) {
          w.push(`${at}：有 ${beyond} 筆日期晚於 through（${String(through)}），仍算休市日；請把 through 延伸到已涵蓋的日期`);
        }
      }
    }
  }

  const dl = config.earningsWeek?.deadlines;
  if (dl !== undefined) {
    if (!Array.isArray(dl)) w.push('config.earningsWeek.deadlines 型別應為陣列');
    else
      dl.forEach((s, i) => {
        if (!isRealMonthDay(s)) w.push(`config.earningsWeek.deadlines[${i}]（${String(s)}）不是 MM-DD 的真實日期`);
      });
  }

  for (const k of ['shortRatio', 'maintenanceRatio']) {
    const v = config.thresholds?.[k];
    if (v !== undefined && !(typeof v === 'number' && Number.isFinite(v) && v > 0)) {
      w.push(`config.thresholds.${k}（${String(v)}）必須是正數`);
    }
  }

  // 農曆正月初一表（task 3.4 修正輪 1）：日期真實、落在年度鍵那年的 1/21–2/20、出處解得開
  const lny = config.lunarNewYear;
  if (lny !== undefined) {
    if (!isPlain(lny)) w.push('config.lunarNewYear 型別應為物件');
    else {
      const lsrc = isPlain(lny.sources) ? lny.sources : {};
      for (const [year, e] of Object.entries(isPlain(lny.dates) ? lny.dates : {})) {
        const at = `config.lunarNewYear.dates.${year}`;
        if (!isPlain(e)) {
          w.push(`${at} 型別應為物件`);
          continue;
        }
        if (lunarNewYearOf(config, year) === null) {
          w.push(`${at}.date（${String(e.date)}）不是 ${year} 年 1/21–2/20 之間的真實日期，該年改用 Intl 中國曆`);
        }
        if (!Object.hasOwn(lsrc, e.src)) w.push(`${at}.src（${String(e.src)}）不在 lunarNewYear.sources 內`);
      }
      if (lny.dates !== undefined && !isPlain(lny.dates)) w.push('config.lunarNewYear.dates 型別應為物件');
    }
  }

  // 撕日曆每一項單行呈現：超過 PHRASE_MAX_CHARS 字的字句會縮到很小（spec 情境以 20 字為上限）
  for (const [at, s] of phraseEntries(config)) {
    const n = [...s].length;
    if (n > PHRASE_MAX_CHARS) w.push(`${at}（${n} 字）超過 ${PHRASE_MAX_CHARS} 字，撕日曆會把該行縮得很小：「${s}」`);
  }
  return w;
}

/**
 * 查某市場在「交易所當地日期」的狀態。`localDate`＝YYYY-MM-DD，必須已換成該市場 tz 的日期
 * （用 `holidayOnAt` 可直接給 UTC 毫秒）。明列的日期一律生效，不看 through。
 * 'unknown'＝未列出且晚於 through（或該年度不存在）；畫面上等同 'open'。週末不處理（呼叫端自行判斷）。
 * `days` 裡不是物件的元素略過，訊息加進選填的 `warnings`（陣列或 Set）。
 */
export function holidayOn(config, marketId, localDate, warnings) {
  const year = String(localDate).slice(0, 4);
  const years = config?.holidays?.[marketId];
  const y = isPlain(years) ? years[year] : undefined;
  let hit;
  if (isPlain(y) && Array.isArray(y.days)) {
    y.days.forEach((d, i) => {
      if (!isPlain(d)) note(warnings, `config.holidays.${marketId}.${year}.days[${i}] 不是物件，已略過`);
      else if (hit === undefined && d.date === localDate) hit = d;
    });
  }
  if (hit) {
    const r = { status: hit.half === true ? 'half' : 'closed', name: hit.name };
    if (hit.half === true) r.close = hit.close;
    if (hit.src !== undefined) r.src = hit.src;
    return r;
  }
  if (!isPlain(y) || !isRealDate(y.through) || localDate > y.through) return { status: 'unknown' };
  return { status: 'open' };
}

/**
 * 同 `holidayOn`，但輸入是 UTC 毫秒：依 markets[] 裡該市場的 tz 換成交易所當地日期。
 * 找不到市場、tz 不是 Intl 認得的時區、`nowMs` 不是有限數字 → `{ status: 'unknown' }`，後兩者記警告。
 */
export function holidayOnAt(config, marketId, nowMs, warnings) {
  const mk = usableMarkets(config, warnings).find((m) => m.id === marketId);
  if (!mk) return { status: 'unknown' };
  if (!isValidTz(mk.tz)) {
    note(warnings, `config.markets 的 ${marketId}.tz（${String(mk.tz)}）不是 Intl 認得的 IANA 時區，休市表當作未知`);
    return { status: 'unknown' };
  }
  if (typeof nowMs !== 'number' || !Number.isFinite(nowMs)) {
    note(warnings, `holidayOnAt：時刻（${String(nowMs)}）不是有限數字，${marketId} 休市表當作未知`);
    return { status: 'unknown' };
  }
  const p = tzParts(mk.tz, nowMs);
  const pad = (n) => String(n).padStart(2, '0');
  return holidayOn(config, marketId, `${p.y}-${pad(p.mo)}-${pad(p.d)}`, warnings);
}

/**
 * 年底提醒（4.8）：`todayLocal`（使用者本機日期 YYYY-MM-DD）距當年 12/31 不足 30 天（12/02 起；12/01 剛好 30 天不算），
 * 且市場的最晚 through 早於「明年」12/31 → 該市場列入。回傳市場 id 陣列，空陣列＝不用提醒。
 * 市場完全沒有休市資料也算「早於」。`markets` 裡不是含 id 物件的元素略過並記警告。
 */
export function holidayExpiryReminder(config, todayLocal, warnings) {
  const m = DATE_RE.exec(todayLocal);
  if (!m || !isRealDate(todayLocal)) return [];
  const year = +m[1];
  const daysLeft = Math.round((Date.UTC(year, 11, 31) - Date.UTC(year, +m[2] - 1, +m[3])) / 86400000);
  if (daysLeft >= 30) return [];
  const target = `${year + 1}-12-31`;
  const holidays = isPlain(config?.holidays) ? config.holidays : {};
  const ids = Array.isArray(config?.markets) ? usableMarkets(config, warnings).map((x) => x.id) : Object.keys(holidays);
  return ids.filter((id) => {
    const years = holidays[id];
    const latest = Object.values(isPlain(years) ? years : {})
      .map((y) => (isPlain(y) ? y.through : undefined))
      .filter(isRealDate)
      .sort()
      .at(-1);
    return !latest || latest < target;
  });
}

/** 標了 `verified: false` 的市場年度（待複核）；4.8 可在設定視窗提示。 */
export function unverifiedHolidayYears(config) {
  const out = [];
  for (const [market, years] of Object.entries(isPlain(config?.holidays) ? config.holidays : {})) {
    for (const [year, y] of Object.entries(isPlain(years) ? years : {})) {
      if (y?.verified === false) out.push({ market, year });
    }
  }
  return out;
}
