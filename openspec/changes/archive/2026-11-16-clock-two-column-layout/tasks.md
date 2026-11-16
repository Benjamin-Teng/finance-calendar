# Tasks：clock-two-column-layout

> 執行路徑：直接 `opsx:apply`｜理由：只改時鐘的 DOM 包一層與 CSS，樣式已由使用者從五個候選中選定。

## 1. 版面

- [x] 1.1 `clock.js` 每格加定位容器、`widget.css` 改成兩欄三列的樣式 5。驗證：
  `node host/tests/compare/measure-clock-natural-width.mjs` PASS（自然尺寸含餘裕不超過 212×160、溢出 0）。
- [x] 1.2 編輯版面時右上格不與字級控制重疊、框較高時「已達上限」提示仍顯示。驗證：
  `node host/tests/compare/verify-font-controls-geometry.mjs` PASS。
- [x] 1.3 `node host/tests/compare/compare.mjs --widget clock` PASS（文字順序不變）；headless 截圖確認開盤格亮起。

## 2. 收尾

- [x] 2.1 更新 `AGENTS.md` 的時鐘說明與相關註解；品質 gate 全過。
- [x] 2.2 發版。使用者 2026-11-16 指示直接發版，未做隔離宿主的實機驗收（正式版執行中）；以 headless 量測、幾何驗證與截圖為證，發版後在本機安裝的正式版上確認。
