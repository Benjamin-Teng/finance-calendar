## Context

`host/tests/compare/compare.mjs` 原本把 Lively 版頁面與新版小工具放在同一個 headless Edge 裡、固定時區與時間，逐行比較
畫面文字。Lively 版刪除後這個基準就沒了，但它擋的回歸（總經日曆、台股動態事件、行情條的顯示規則）仍然需要有東西擋。

## Goals / Non-Goals

**Goals:**

- repo 裡不再有 Lively 版產品檔，也沒有需要它才能跑的測試或工具。
- 顯示規則的回歸保護不因此變弱。

**Non-Goals:**

- 不動 Python 版 `update_tw_events.py`（含它鏡像到 Lively 資料夾的程式）：它是 Rust 抓取的 oracle，等價測試經
  `tests/fetch_oracle/harness.py` 隔離執行，鏡像不會發生。
- 不刪 GitHub 上的舊 release、不移除使用者電腦上的 Lively。
- 不改寫 `host/tools/evidence/` 的歷史證據與已歸檔的 change。

## Decisions

### D1：對照測試改成基準檔比對

刪除前，先以舊的跨版本比對把每個情境再跑一次（macro、dynamic、quotes 的預設畫面、跨日推進、連續休市順延、資料過期、
空行情、顯示除權息），確認與 Lively 版逐行相同，再把新版輸出存成 `golden/<情境>.txt`。唯一不同的是
`dynamic-stale-3d` 的頁尾警示文案，那是 v0.3.6 刻意改寫的。之後 `compare.mjs` 拿新版輸出比基準檔。

替代方案：直接刪掉 `compare.mjs`、只靠 `host/tests/*.test.mjs`。單元測試測的是各函式，沒有「整個小工具在瀏覽器裡
畫出來的文字」這一層，刪掉會少一道保護。

替代方案：把 `finance-calendar.html` 搬進測試 fixture 繼續當基準。等於沒有退場，而且兩邊刻意差異只會越來越多。

### D2：情境集中在 `scenarios.mjs`

原本的情境散在命令列參數與 README 的範例裡，另有一支獨立腳本驗「顯示除權息」。改成一份清單，`compare.mjs` 不帶
參數就全跑；`--today`、`--fixture`、`--advance-to`、`--panel` 等參數拿掉，改用 `--widget`、`--scenario` 篩選。
「顯示除權息」併成一個帶 `settings` 覆寫的情境。

### D3：程式註解裡的行號引用

`host/ui/` 與 `host/src/` 有不少「搬自 finance-calendar.html 第 N 行」的註解。保留當作來歷，在 `host/ui/common.js`
檔頭註明該檔已移除、行號對應 tag `v6.2`。

## Risks / Trade-offs

- [基準檔會被不經意地 `--update` 蓋掉] → 基準檔進版控，改動會出現在 diff；README 寫明重產後要讀過。
- [基準檔只記文字，不記顏色與版面] → 與原本的跨版本比對相同；顏色與版面由各自的單元測試與量測腳本負責。
- [`clock-six-exchanges` 尚未歸檔，原本也改同一條需求] → 那個 change 的 delta 已改成只新增時鐘需求，兩個 change
  的歸檔順序不互相影響。
