## Why

時鐘小工具現在只顯示本機時間、日期與「本週」範圍。使用者想一眼看出六個主要交易所各自幾點、哪幾個正在交易
（使用者 2026-11-15 提出）。宿主已內建這六個交易所的時段與休市表（動態桌布在用），時鐘直接沿用即可。

## What Changes

- 時鐘小工具改成六列，每列一個交易所，只放三項：地區英文簡寫、當地時間（24 小時制 `HH:MM`）、當地日期（`YY-MM-DD`）。
  由東到西：JP（東京）、KR（首爾）、TW（台北）、HK（香港）、UK（倫敦）、US（紐約）。
- 時間的顏色表示開收盤：現貨正規盤（含開收盤集合競價）進行中為白色，其餘為灰色（午休、盤前盤後、期貨夜盤、
  週末、休市日、半日市提早收盤之後）。簡寫與日期固定用次要顏色。
- **BREAKING**：移除原本的本機大時鐘、日期與「本週」範圍三行。
- 開收盤判斷沿用內建的 `wallpaper-config.default.json` 與桌布的時段函式；時鐘只讀內建預設，不讀使用者的
  `wallpaper-config.json`。
- 時鐘的顯示文字由 `compare.mjs` 的基準檔 `golden/clock.txt` 把關（對照工具在 change `retire-lively` 改為基準檔比對）。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `finance-widgets`：新增「時鐘顯示六個交易所時間」。

## Impact

- `host/ui/widgets/clock.js`、`host/ui/widget.css`（時鐘區塊）。
- 新增 `host/ui/exchange-clock.mjs`（純函式）與 `host/tests/exchange-clock.test.mjs`。
- `host/tests/compare/golden/clock.txt`（基準檔）、`host/tests/compare/measure-clock-natural-width.mjs`（改量新版面）、
  `host/tests/widget-font-controls.test.mjs`（若有綁舊節點 id）。
- 時鐘的最小框（`widgets.rs` 的 212×160）不變：新版面要在這個框內放得下，由量測腳本確認。
- `AGENTS.md`「現況」的小工具說明。
- 不新增資料通道、IPC 指令、設定欄位或相依套件。
