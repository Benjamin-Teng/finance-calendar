## Why

Lively Wallpaper 版自 v0.1.1 起由桌面宿主取代，之後只當搬移來源與顯示對照的基準留在 repo。2026-11-15 的搬移稽核
確認新版顯示與資料層都沒有搬壞（找到的一項已修正），使用者 2026-11-16 指示把 Lively 版與相關檔案完整移除。

## What Changes

- **BREAKING**：刪除 Lively 版產品檔：`finance-calendar.html`、`LivelyProperties.json`、`bg.png`、`setup.bat`、
  `scripts/setup.ps1`。要看舊版原始碼請用 `git show v6.2:finance-calendar.html`。
- 刪除以 Lively 版為基準的測試：`tests/compare_lively_dom.mjs`、`host/tests/compare/panels.mjs`、
  `host/tests/compare/verify-dividend-setting.mjs`。
- `host/tests/compare/compare.mjs` 改為基準檔比對：情境清單在 `scenarios.mjs`，每個情境的畫面文字與
  `golden/<情境>.txt` 逐行比較，刻意改顯示時以 `--update` 重產。時鐘與台股固定事件原本因為與 Lively 版不同而跳過，
  現在也有基準檔。
- 規格不再以「與 Lively 版一致」定義顯示規則，改成直接描述規則本身。
- `AGENTS.md` 刪除「舊版（Lively，已凍結）」整章，沿革只留一段；其餘文件同步。
- 保留：`uninstall.bat` 與 `scripts/uninstall.ps1`（還在用舊版的人移除排程用，`README.md` 有說明）、Python 版
  `update_tw_events.py` 與 `tests/fetch_oracle/`（Rust 抓取的行為等價測試基準）、GitHub 上的舊 release。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `finance-widgets`：「顯示規則與 Lively 版一致」改名為「財經小工具顯示規則」，內容改寫成不依賴 Lively 版。
- `wallpaper-market-data`：「新資料鍵失敗時沿用舊值」的「舊版讀者不受影響」情境改成不指名 Lively 版頁面。
- `widget-data-feed`：「資料新鮮度可見」去掉「依 Lively 版規則」字樣。
- `widget-host-windows`：「預設版面」的情境改成直接描述排列。

## Impact

- 刪除的檔案與測試如上；`host/tests/compare/serve.mjs` 不再提供 `/lively/`。
- `host/tests/compare/golden/` 新增 13 個基準檔。
- 文件：`AGENTS.md`、`README.md`、`host/README.md`、`host/tests/compare/README.md`、`tests/fixtures/README.md`、
  `openspec/config.yaml`、`openspec/specs/finance-widgets/spec.md` 的 Purpose。
- 執行檔行為不變（沒有改 `host/src/` 的程式邏輯與 `host/ui/` 的顯示邏輯，只改註解）。
