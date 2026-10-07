## Why

2026-09-28 的宣傳視覺探索產出九套桌布提案（`assets/design-explore/`），使用者選出五套，希望它們隨真實市場資料與
時間變化，成為真正的動態桌布，而不只是示意圖。做法必須延續離開 Lively／Wallpaper Engine 的初衷：不把視窗嵌進
explorer、不讓宿主同步等待 explorer、不持續渲染。因此改成「定時產生圖片、交給 Windows 設為桌布」，由現有的桌面
小工具宿主（`host/`）負責。

本 change 接在 desktop-widget-host 之後：規劃先行，實作等該 change 合併進 main 後再開工。

## What Changes

- 宿主新增**桌布接管**：設定視窗選擇一套主題（01 脊線、03 撕日曆、05 星盤、07 等高線、09 天際線）或「不接管」。
  接管時依各主題的節奏，以隱藏的 WebView2 渲染頁畫出 PNG，透過 `IDesktopWallpaper` 對每台螢幕依原生解析度各設一張；
  全部螢幕同一套。
- **還原與讓位**：首次接管前記錄每台螢幕的原桌布（含填滿方式、純色、投影片；圖片另存備份）。選「不接管」、從系統匣
  「結束」時還原；登出、關機不還原。使用者自行在 Windows 換桌布時，宿主自動讓位並通知。原桌布是 Windows 焦點時，
  接管前提醒無法自動切回。
- **節流與暫停**：鎖定、顯示器關閉、睡眠、全螢幕或簡報、勿打擾時暫停，恢復後補畫；沿用既有的電池／省電暫停開關。
  呼叫 explorer 的系統 API 一律放在專屬執行緒並設逾時。每次設定前讀取 explorer 的 GDI 物件數，累積超過門檻即自動
  停止接管並還原原桌布（探針 1.1 實測反覆設桌布會使 explorer 的 GDI 物件累積）。
- **五套主題接真實資料**：01／07 畫加權指數最後一個完整交易日的盤中走勢；09 畫過去 20 個交易日 K 線與 20MA；
  03 撕日曆的「宜」顯示財報密集週、台積電財報日、訂單旺季，「忌」在券資比或融資維持率越過門檻時提醒，休市日改顯示
  固定的家庭提醒；05 星盤改設計為東京、首爾、台北、香港、倫敦、紐約六個市場的交易時段盤，每 15 分鐘轉一格。
- **可編輯設定檔**：市場時段表、年度休市表、訂單旺季月份表、宜忌詞庫、警示門檻，格式錯誤時退回內建預設值。
- **資料層擴充**（`update_tw_events.py`）：新增加權指數盤中走勢、日 K、上市券資比與融資維持率三個資料鍵；排程加
  每日 15:00 觸發（使用者需以系統管理員重跑一次 `setup.bat`）。
- **圖示**：統一產品圖示（金色日曆頁）做成多尺寸 `icon.ico`，用於 exe、安裝檔、應用程式清單與著陸頁 favicon；
  系統匣與設定視窗圖示改為跟著選定的主題切換，「不接管」時用統一圖示。
- **字型**：渲染用字型（Cinzel、霞鶩文楷 TC、Noto Serif TC、Noto Sans TC、IBM Plex Mono）全部隨宿主打包，桌布不依賴網路。

## Capabilities

### New Capabilities

- `desktop-wallpaper`: 宿主接管 Windows 桌布的生命週期——主題選擇、重畫時機、逐螢幕出圖與設定、暫停、
  原桌布記錄與還原、使用者自換讓位、失敗處理、與 explorer 的隔離。
- `wallpaper-themes`: 五套主題各自畫什麼、資料缺漏與過期時的畫面、任何解析度與縮放下的等比縮放、可編輯設定檔的內容
  與退回規則。
- `wallpaper-market-data`: 資料層新增的三個資料鍵（盤中走勢、日 K、券資比與融資維持率）的語意、來源、挑選規則與
  排程時點。
- `app-icon`: 統一產品圖示的使用範圍與多尺寸規格，以及系統匣與設定視窗圖示隨主題切換。

### Modified Capabilities

（無。desktop-widget-host 的 capability 尚未 archive 進 `openspec/specs/`；本 change 只新增設定欄位與系統匣行為，
以新 capability 描述，合併後再一起 archive。）

## Impact

- **宿主程式碼**：新增 `host/src/wallpaper.rs`；`host/src/desktop.rs` 加入 `IDesktopWallpaper` 包裝與專屬 COM 執行緒
  （Win32 仍只在這裡）；`settings.rs` 新增桌布主題欄位；`data.rs` 新增 `wallpaper` 通道；`tray.rs` 依主題換圖示；
  新增 `host/ui/wallpapers/`（五個繪圖頁、渲染頁）與內建字型；`host/icons/` 換成正式多尺寸圖示。
- **相依**：`windows` crate 可能需要多開 `Win32_System_Com` feature（實作時確認）；不新增第三方 crate 為預期目標，若需
  PNG 以外的影像處理再另行徵詢。
- **資料層**：`update_tw_events.py` 新增端點（Yahoo v8 chart 盤中／日線、TWSE 融資融券彙總與個股）；`tw_events.json`
  新增鍵（向下相容，舊鍵不變）；`scripts/setup.ps1` 排程加 15:00。
- **使用者環境**：接管期間會改寫使用者的桌布設定與填滿方式（全域），並在宿主資料夾存放原圖備份與輸出圖；安裝檔大小
  增加約 55 MB（字型官方原檔，未子集化）。
- **著陸頁**：`docs/favicon.svg`／`.png` 換成統一圖示。
- **不在範圍**：安裝檔本身與解除安裝時的還原呼叫（子專案 3）、資料層 Rust 化（子專案 2）、九套中未選的四套、
  Lively 版 `finance-calendar.html`（凍結）。
