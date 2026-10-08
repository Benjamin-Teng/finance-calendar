## Why

新宿主 v0.1.0 要取代 Lively 版，以 GitHub Release 附安裝檔的方式發佈（著陸頁已改指向 `finance-calendar-setup.exe`）。
目前宿主只有開發用的 `cargo build` 產物：沒有安裝檔，沒有人寫入開機自啟，解除安裝時也沒有人呼叫桌布還原。使用者
2026-10-05 決定 v0.1.0 就要有自動更新，讓資料來源改版時，抓取邏輯（data-layer-rust）的修正能快速送到使用者手上，
不必等使用者自己回著陸頁下載。

## What Changes

- **安裝檔**：NSIS、單一使用者安裝（裝到 `%LOCALAPPDATA%`，全程不需要系統管理員權限）；缺 WebView2 時自動下載安裝。
  首次安裝寫入開機自啟（`HKCU\…\Run` 的 `fc-host` 值），更新時保留使用者在設定視窗的選擇；安裝完成後啟動宿主。
- **解除安裝**：先執行 `fc-host.exe --restore-wallpaper` 還原原桌布，再移除程式與開機自啟；使用者設定與資料預設保留。
- **自動更新**：宿主啟動後與每 6 小時檢查 GitHub Release 上的 `latest.json`；有新版就在背景下載並驗證簽章，
  以系統匣通知一次，並提供「更新並重新啟動」選單項；使用者不點的話，就在下次登入（開機自啟）時自動安裝。更新期間
  不還原桌布，更新完成後自動重新啟動並恢復小工具與桌布接管。
- **發佈產物**：每次發版產出 x64 安裝檔 `finance-calendar-setup.exe`、ARM64 安裝檔 `finance-calendar-setup-arm64.exe`、
  兩者的更新簽章，以及 `latest.json`；由 GitHub Actions 在 x64 與 ARM64 runner 各建置一次。
- 更新簽章使用 Tauri 的 minisign 金鑰。程式本身 v0.1.0 不做 Authenticode 簽章，發佈後申請 SignPath Foundation。

## Capabilities

### New Capabilities

- `app-distribution`：安裝、解除安裝、開機自啟登錄、自動更新的檢查／下載／安裝時機與失敗處理，以及發佈產物的命名與
  驗證。

### Modified Capabilities

（無。desktop-widget-host 已規定「開機自啟登錄歸安裝檔」，dynamic-wallpaper 已規定 `--restore-wallpaper` 供安裝檔
呼叫；本 change 是實作這兩項約定的那一方，不改它們的需求。）

## Impact

- 程式：`host/Cargo.toml` 加 `tauri-plugin-updater`（關閉預設功能，只開 `native-tls`、`system-proxy`，與 data-layer-rust
  共用 SChannel）；新增 `host/src/updater.rs`；`host/src/tray.rs` 加更新選單項；`host/tauri.conf.json` 加 `bundle` 與
  `plugins.updater`；新增 `host/installer/hooks.nsh`。
- 建置：本機開發照舊純 `cargo build`；打包改用 tauri-cli 的預編譯執行檔（不需 Node），封裝成
  `host/tools/package.ps1`。新增 `.github/workflows/release.yml`。
- 機密：更新簽章私鑰與密碼放在 GitHub Actions secrets，另做離線備份；公鑰進 repo。私鑰遺失就再也無法推送更新。
- 發版流程：發版歸 desktop-widget-host 收尾線，本 change 提供 workflow 與產物規格。發版時 v0.1.0 必須設為 Latest
  release，之後不得再發佈不含 `latest.json` 的 release。
- 依賴 data-layer-rust：同一個 TLS 組態；安裝檔裝好的宿主要能自己抓資料，否則首次安裝的小工具沒有資料。
