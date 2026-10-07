#!/usr/bin/env node
// Lively 版頁面 DOM 比對（dynamic-wallpaper tasks 2.4 最後一句）：
// 同一份 finance-calendar.html、固定 window.__TEST_TODAY，分別讀「含新鍵」與「不含新鍵」兩份
// tw_events.js（其餘內容完全相同），用 headless Edge --dump-dom 取出渲染後 DOM，
// 排除時鐘卡（時間／日期／週區間／版本）與兩個頁腳（資料更新時間）後，兩份必須一致。
//
// 執行（零安裝，Node 22）：node tests/compare_lively_dom.mjs
// 環境變數：EDGE＝msedge.exe 路徑（預設 Program Files (x86) 的 Edge）
// finance-calendar.html 凍結不改：只在暫存資料夾的「複本」第一行 <head> 後插入 __TEST_TODAY。
// 每次比對用不同的 --user-data-dir（避免附掛到使用者既有的 Edge 實例）。

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const SAMPLE = join(ROOT, "tests", "fixtures", "tw_events_sample.json");
const NEW_KEYS = ["twii_intraday", "twii_daily", "margin", "wallpaper_errors"];
const TEST_TODAY = "2026-10-02";
const EDGE = process.env.EDGE ||
  "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
// 時鐘（時間／日期／週區間／版本標籤）與兩個頁腳（含「資料更新」時間）會隨真實時鐘變動，比對前清空內容
const EXCLUDE_IDS = ["clockTime", "clockDate", "weekRange", "verTag", "macroFoot", "dynFoot"];

if (!existsSync(EDGE)) {
  console.error(`找不到 Edge：${EDGE}（可用環境變數 EDGE 指定）`);
  process.exit(2);
}

const full = JSON.parse(readFileSync(SAMPLE, "utf8"));
for (const k of NEW_KEYS) {
  if (!(k in full)) throw new Error(`樣本缺新鍵 ${k}`);
}
const without = Object.fromEntries(Object.entries(full).filter(([k]) => !NEW_KEYS.includes(k)));
// sources 也有新鍵的來源說明；「不含新鍵」那份連同這些說明一併去掉
without.sources = Object.fromEntries(
  Object.entries(full.sources ?? {}).filter(([k]) => !NEW_KEYS.includes(k)));

const work = mkdtempSync(join(tmpdir(), "fc-dom-"));
let rc = 1;
try {
  const html = readFileSync(join(ROOT, "finance-calendar.html"), "utf8");
  if (!html.includes("<head>")) throw new Error("找不到 <head>，無法注入 __TEST_TODAY");
  const injected = html.replace("<head>", `<head><script>window.__TEST_TODAY='${TEST_TODAY}';</script>`);

  const dump = (label, payload) => {
    const dir = join(work, label);
    mkdirSync(dir);
    writeFileSync(join(dir, "finance-calendar.html"), injected, "utf8");
    writeFileSync(join(dir, "tw_events.js"), "window.TW_EVENTS = " + JSON.stringify(payload) + ";\n", "utf8");
    const bg = join(ROOT, "bg.png");
    if (existsSync(bg)) copyFileSync(bg, join(dir, "bg.png"));
    const r = spawnSync(EDGE, [
      "--headless=new", "--disable-gpu", "--window-size=1920,1080",
      "--virtual-time-budget=6000", `--user-data-dir=${join(work, "profile-" + label)}`,
      "--dump-dom", pathToFileURL(join(dir, "finance-calendar.html")).href,
    ], { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: 120000 });
    if (r.status !== 0 || !r.stdout) {
      throw new Error(`Edge 失敗（${label}）：status=${r.status} ${r.error ?? ""}\n${r.stderr.slice(-500)}`);
    }
    return r.stdout;
  };

  const normalize = (dom) => {
    let s = dom;
    for (const id of EXCLUDE_IDS) {
      // 清空該 id 的葉節點 div 內容（內層至多有 span）；元素本身與其屬性保留
      const re = new RegExp(`(<div[^>]*\\bid="${id}"[^>]*>)[\\s\\S]*?(</div>)`);
      if (!re.test(s)) throw new Error(`DOM 內找不到可排除的元素 #${id}`);
      s = s.replace(re, "$1$2");
    }
    // 資料載入用的快取破壞參數（?t=Date.now()）每次不同，不是頁面內容
    return s.replace(/tw_events\.js\?t=\d+/g, "tw_events.js?t=#");
  };

  const domA = dump("without-new-keys", without);
  const domB = dump("with-new-keys", full);

  // 健全性：頁面真的渲染了資料（否則兩份同樣空白也會「一致」）
  for (const [label, dom] of [["without", domA], ["with", domB]]) {
    for (const needle of ["加權指數", "非農", "FOMC"].filter((n) => JSON.stringify(full).includes(n))) {
      if (!dom.includes(needle)) throw new Error(`${label} 的 DOM 缺少預期內容「${needle}」，疑似沒渲染到資料`);
    }
    if (dom.length < 20000) throw new Error(`${label} 的 DOM 過短（${dom.length}），疑似沒渲染到資料`);
  }
  if (readFileSync(join(work, "with-new-keys", "tw_events.js"), "utf8").includes("twii_daily") === false) {
    throw new Error("含新鍵那份 tw_events.js 實際沒有新鍵");
  }
  if (NEW_KEYS.some((k) => readFileSync(join(work, "without-new-keys", "tw_events.js"), "utf8").includes(k))) {
    throw new Error("不含新鍵那份 tw_events.js 卻含有新鍵");
  }

  const a = normalize(domA);
  const b = normalize(domB);
  if (a === b) {
    console.log(`PASS：含／不含新鍵的 DOM 一致（排除 ${EXCLUDE_IDS.map((i) => "#" + i).join("、")}；` +
      `__TEST_TODAY=${TEST_TODAY}；DOM 長度 ${a.length}）`);
    rc = 0;
  } else {
    writeFileSync(join(work, "a.html"), a, "utf8");
    writeFileSync(join(work, "b.html"), b, "utf8");
    console.error(`FAIL：DOM 不一致，差異檔保留於 ${work}（a.html＝不含新鍵、b.html＝含新鍵）`);
    process.exit(1);
  }
} finally {
  if (rc === 0) rmSync(work, { recursive: true, force: true });
}
process.exit(rc);
