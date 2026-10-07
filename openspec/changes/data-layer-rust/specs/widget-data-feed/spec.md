## MODIFIED Requirements

### Requirement: 資料通道

宿主 SHALL 以具名的資料通道組織資料，每個通道有獨立的來源與快照。本 change SHALL 提供通道 `tw-events`（來源為資料目錄中的 `tw_events.json`，即現有資料層產出格式）與五個擴充通道 `custom1`–`custom5`（來源為資料目錄中的 `custom1.json`–`custom5.json`，內容為任意合法 JSON）。資料通道 MUST NOT 改寫任何來源檔；`tw_events.json` 由宿主內建的資料抓取（見 `market-data-fetch`）或外部資料層產生，資料通道只讀取。

#### Scenario: 讀取資料層產出

- **WHEN** 資料目錄中存在由宿主資料抓取或 `update_tw_events.py` 產出的 `tw_events.json`
- **THEN** 通道 `tw-events` 的快照為其內容

#### Scenario: 擴充通道讀取任意 JSON

- **WHEN** 使用者把一份合法 JSON 存為資料目錄中的 `custom1.json`
- **THEN** 通道 `custom1` 的快照為其內容，其他通道不受影響
