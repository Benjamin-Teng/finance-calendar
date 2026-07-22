# 財經日曆桌布

桌面上的財經儀表板 — 總經日曆、台股事件、即時行情條，深色毛玻璃風格，資料每 6 小時自動更新。
底層使用 [Lively Wallpaper](https://www.rocksdanister.com/lively/)（開源、原生 ARM64 WebView2）。

---

## 🚀 安裝

**需求**：Windows 10/11（ARM64 或 x64 皆可）。Python 與 Lively 不用先裝，`setup.bat` 會自動處理。

1. **下載並解壓**本專案（綠色 `Code` → `Download ZIP`，或到 Releases 下載最新版）。
2. **執行 `setup.bat`**：對它按右鍵 →「以系統管理員身分執行」（或直接雙擊，按 UAC「是」）。它會自動：
   - 偵測／安裝 **Python**（winget，自動選 arm64/amd64）
   - 偵測／安裝 **Lively Wallpaper**（winget，Microsoft Store 版）
   - 建立排程：**每 6 小時 ＋ 每次登入**自動更新資料
   - **抓一次**最新資料
3. **在 Lively 設成桌布**：開啟 Lively → 把資料夾裡的 `finance-calendar.html` 拖進 Lively → 設為桌布。完成 ✅ 桌面立即顯示最新資料，之後全自動更新。

> 小提醒：步驟 2 的 `setup.bat` 已幫你把 Python 與 Lively 都裝好；步驟 3 只需在 Lively 拖一下設定桌布即可。

---

## ✨ 功能

- **總經日曆** — Forex Factory（美國・歐元區・日本，中高重要性），含預測/前值，**跟隨系統時區**顯示。
- **台股事件** — 法說會・股東會（預告清單）＋處置股（當日制、遇休市順延），資料來自 TWSE／TPEx。
- **行情條** — 13 檔紅漲綠跌跑馬燈：USD/TWD、美債 10Y、WTI、加權、台積電、日經 225、KOSPI、歐股 50、道瓊、S&P 500、NASDAQ、**費半（SOX）**、SOFR 3M。
- **可調外觀** — 介面縮放、面板不透明度、毛玻璃模糊、強調色（Lively 屬性面板即時調）。

---

## 🖱️ 操作

- **清單太長看不完？** 清單正上／正下方會出現 **▴ / ▾ 按鈕**，點一下翻一頁；捲到最底時 ▾ 會變成**回頂鈕**，點一下回到最上。
  （Lively 只轉發滑鼠「點擊」、不轉發滾輪，所以用按鈕而非捲軸。）
- **調整外觀**：Lively → 對這張桌布開「自訂 / Customize」→ 4 個滑桿（縮放／不透明度／模糊／強調色），即時生效。

---

## 🔄 資料怎麼自動更新

- `setup.bat` 建立的排程，每 6 小時＋每次登入跑 `update_tw_events.py` 抓最新資料。
- Lively 會把桌布**複製**到自己的資料夾，而它的 WebView2 擋掉「跨資料夾讀取」；因此資料層會**動態找到** Lively 的桌布資料夾，把最新資料**鏡像**過去（不寫死路徑，重匯桌布也自動跟上）。
- 桌布每分鐘輕量重讀自身資料夾 → 更新後約 1 分鐘內自動翻新（看著桌面時）。

---

## 🛠️ 進階自訂

- **桌布參數**：`finance-calendar.html` 最上方的 `CONFIG`（欄寬、顏色、重載時間、行情條開關等）。改完在 Lively **重新匯入一次** html 生效。
- **增減行情標的**：`update_tw_events.py` 最上方的 `QUOTES`（symbol／顯示名／類型）。
- **時鐘卡右下角的版本小字**：對照你改的版本有沒有套用成功。

---

## ❓ 疑難排解

| 狀況 | 處理 |
|---|---|
| 資料沒更新 | 確認桌布已在 Lively 設好；重跑 `setup.bat` 會立即推一次 |
| 桌面看不到桌布 | 開 Lively 的「Startup with Windows」；在 Lively 重新選一次這張桌布 |
| 面板顯示「請執行 update_tw_events.py」 | 資料檔還沒到位；跑一次 `setup.bat` 或等下一次排程 |
| 想確認版本 | 看時鐘卡右下角小字；重匯 html 後版號有跳＝新版已套用 |

---

## 🧩 技術說明

- 原本使用 Wallpaper Engine，但在 ARM64（Snapdragon）機器上 WE 的 CEF 渲染器反覆崩潰導致桌布消失；改用 **Lively Wallpaper**（原生 ARM64 WebView2）根治。
- 資料層 `update_tw_events.py` 為**純 Python 標準函式庫、零第三方依賴**——這也是能用 winget 一鍵裝 Python 就跑的原因。
- 資料來源：Forex Factory、TWSE／TPEx OpenAPI、Yahoo Finance chart API、NY Fed SOFR。
