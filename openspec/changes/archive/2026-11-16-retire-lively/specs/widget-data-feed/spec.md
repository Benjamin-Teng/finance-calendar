## MODIFIED Requirements

### Requirement: 資料新鮮度可見

通道 `tw-events` 的快照 SHALL 保留資料檔中的 `fetched` 與 `updated` 時間戳記，使小工具能顯示資料更新時間與
「已 N 天未更新」提示。

#### Scenario: 資料過期

- **WHEN** `tw_events.json` 的 `fetched` 距今超過 3 天
- **THEN** 台股動態事件小工具的頁腳標示已多少天未更新
