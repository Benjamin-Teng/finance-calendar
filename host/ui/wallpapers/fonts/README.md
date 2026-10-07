# 桌布繪圖頁內建字型

桌布繪圖頁（`host/ui/wallpapers/`）渲染時只用這個資料夾的字型，以 `fonts.css` 的 `@font-face`
相對路徑載入，不連任何外部網址（規格「渲染不依賴網路」）。全部字型為 SIL Open Font License 1.1，
授權全文在同資料夾 `OFL-*.txt`。

使用者決定（2026-10-02）：收官方 TTF 原檔，合計約 43 MB 可接受；不轉 WOFF2、不做子集化
（詞庫可編輯，可能用到任何字）。同日另決定星盤中文字內建 Noto Sans TC（約 11.9 MB，task 3.3 修正輪），
合計約 55 MB。

## 來源

全部取自 Google Fonts 官方 repo [`google/fonts`](https://github.com/google/fonts) 的固定 commit
`9710da1eacb3be272583c3224dcb70f9da6eadbb`（2026-09-30 的 main），檔案原樣未改。
下載網址格式：`https://raw.githubusercontent.com/google/fonts/9710da1eacb3be272583c3224dcb70f9da6eadbb/ofl/<目錄>/<檔名>`。

LXGW WenKai TC 的 Bold 在上游 `lxgw/LxgwWenkaiTC` 的 releases 只有 Light／Regular／Medium，
沒有 Bold，所以兩個字重都取 Google Fonts 版，確保同一來源與版本。

| 檔案 | 字重 | 目錄 | 位元組 | 字型內部版本 | SHA-256（前 16 碼） |
| --- | --- | --- | ---: | --- | --- |
| `Cinzel-VF.ttf`（原檔名 `Cinzel[wght].ttf`，變動字型 400–900） | 500、600、700 | `ofl/cinzel` | 125,468 | 2.000 | `f4d83d34d1f6c741` |
| `LXGWWenKaiTC-Regular.ttf` | 400 | `ofl/lxgwwenkaitc` | 13,110,528 | 1.330 | `4fcc5aec11cbbf73` |
| `LXGWWenKaiTC-Bold.ttf` | 700 | `ofl/lxgwwenkaitc` | 12,881,104 | 1.330 | `5c9feadfd928f3ae` |
| `NotoSerifTC-VF.ttf`（原檔名 `NotoSerifTC[wght].ttf`，變動字型 200–900） | 900 | `ofl/notoseriftc` | 16,851,596 | 2.003 | `0077e18f57c6908f` |
| `NotoSansTC-VF.ttf`（原檔名 `NotoSansTC[wght].ttf`，變動字型 100–900） | 500、600 | `ofl/notosanstc` | 11,941,968 | 2.004 | `864727d210d54f25` |
| `IBMPlexMono-SemiBold.ttf` | 600 | `ofl/ibmplexmono` | 140,216 | 2.3 | `d3c38e55c78f5b0f` |

`.ttf` 合計 55,050,880 位元組（3.1 的 43,108,912＋Noto Sans TC 11,941,968）。變動字型檔名把 `[wght]` 改成 `-VF`，避免方括號在網址與
指令列造成麻煩，內容不變。

## 授權檔

| 檔案 | 對應字型 |
| --- | --- |
| `OFL-Cinzel.txt` | Cinzel |
| `OFL-LXGWWenKaiTC.txt` | LXGW WenKai TC（霞鶩文楷 TC） |
| `OFL-NotoSerifTC.txt` | Noto Serif TC |
| `OFL-NotoSansTC.txt` | Noto Sans TC |
| `OFL-IBMPlexMono.txt` | IBM Plex Mono |

## 用到的字重（對照樣稿 `assets/design-explore/`）

- Cinzel 500／600／700：星盤（`bg-sessions.html`）。
- LXGW WenKai TC 400／700：撕日曆（`03-tearoff/bg.html`；未寫字重的行＝400）。
- Noto Serif TC 900：撕日曆日期大字。
- Noto Sans TC 500／600：星盤的中文字（市場名與狀態、「接下來」、圖例）。樣稿的字型堆疊是
  `"Microsoft JhengHei UI","Noto Sans TC",sans-serif`，前者是系統字型、不是內建字型；依「渲染不依賴網路」
  內建堆疊中的 Noto Sans TC，保留樣稿的黑體風格（使用者 2026-10-02 決定，task 3.3 修正輪）。
  來源同上表的固定 commit；檔案的 git blob 雜湊（`82943579ad39c281`）與 GitHub API 回報一致，SHA-256 全碼
  `864727d210d54f2537bbe23b3a839436c3992af72de9322af5270897246bd44f`。
- IBM Plex Mono 600：等高線（`07-contour/bg.html` 第 256 行）。

清單的程式碼版本在 `../lib/core.mjs` 的 `FONT_REQUIREMENTS`，單元測試會核對檔案、
`fonts.css` 與授權檔三者一致。

## 更新字型時

換 commit 或版本時，同步更新本檔的 commit、位元組、版本與 SHA-256，並重跑
`node --test host/tests/wallpapers/core.test.mjs` 與
`node host/tools/wallpaper-shots.mjs --offline-check`。
