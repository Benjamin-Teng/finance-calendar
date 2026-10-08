## REMOVED Requirements

### Requirement: 每日 15:00 更新

**Reason**: 原要求以 Windows 排程器（重跑 `setup.bat` 的安裝設定）增加 15:00 觸發；資料抓取改由宿主內建排程負責後，不再依賴 Windows 排程器，「重跑安裝設定」情境已不適用。

**Migration**: 由本 change 新增的「每日 15:00 抓取」取代；時點由 `market-data-fetch`「定時抓取」提供。

## ADDED Requirements

### Requirement: 每日 15:00 抓取

資料抓取 SHALL 在既有時點之外，於台北時間每日 15:00 增加一輪，使收盤後的日 K 線與盤中走勢在當日下午即可取得。此時點由宿主內建的資料抓取排程提供（見 `market-data-fetch`「定時抓取」），不再依賴 Windows 排程器。

#### Scenario: 收盤後取得當日資料

- **WHEN** 宿主在交易日持續執行並跨過台北時間 15:00，前一輪在 14:00 之前開始
- **THEN** 15:06 前開始的那一輪抓取寫出的盤中走勢日期為當日
