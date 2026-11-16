## Why

v0.3.7 的時鐘是單欄六列的小表格，使用者看過後覺得不好看（2026-11-16），希望回到原本大時鐘的樣子縮小，左三右三擺放。
看過五種樣式後選定「開盤底色」那一種。

## What Changes

- 時鐘版面改成左右兩欄、每欄三格：左欄 JP／KR／TW，右欄 HK／UK／US。
- 每格是原本大時鐘的縮小版：粗體時間在上、日期在下，地區簡寫以小字放在時間左上角。
- 開盤的那一格整格亮起並帶主題色細框、簡寫變主題色、時間白色；收盤的格子時間與日期為灰色。
- 時鐘的框維持 212×160，開收盤判斷與資料來源不變。

## Capabilities

### New Capabilities

（無）

### Modified Capabilities

- `finance-widgets`：「時鐘顯示六個交易所時間」的版面描述由六列改為左右兩欄各三格，開盤多了底色與細框。

## Impact

- `host/ui/widgets/clock.js`（每格多一層定位容器）、`host/ui/widget.css` 的時鐘區塊與編輯版面時的對應規則。
- 文件：`AGENTS.md`。顯示文字的順序不變，`host/tests/compare/golden/clock.txt` 不用重產。
