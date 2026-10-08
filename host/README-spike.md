# fc-host（Tauri 桌布 host spike）

> **本檔為 spike 階段（task 1.x）的歷史記錄，下方旗標與 `fc://` scheme 已在 task 2.1
> 重整時移除**（`src/main.rs` 現在只組裝 `settings`／`data`／`desktop`／`widgets`／`tray`
> 模組骨架，尚未開任何小工具視窗）。正式使用方式待 openspec/changes/archive/2026-10-08-desktop-widget-host/
> tasks.md 後續 task（3.x、5.x）完成後另補文件；本檔保留供對照 spike 期間驗證過的行為。

最小 Tauri 2 程式：把 `finance-calendar.html` 讀進一個獨立、無邊框、永遠置底的頂層視窗，
不透過 Lively／explorer 的 WorkerW。純 cargo，未裝 tauri-cli。

## 建置

```powershell
cd host
cargo build --release
# 產物：host/target/release/fc-host.exe
```

## 執行旗標

- `--dir <path>`：資料夾 D（含 `finance-calendar.html`），預設為 repo 根目錄。
- `--monitor <index>`：目標螢幕（`available_monitors()` 順序），預設 0。
- `--width` / `--height`：邏輯像素，預設 1100×900。
- `--noactivate`：加上 `WS_EX_NOACTIVATE`，點擊視窗不搶焦點。

啟動後會把一行 JSON（pid／hwnd／url／dir／monitor／rect）印到 stdout，並寫到
`host/spike-run.json`。
