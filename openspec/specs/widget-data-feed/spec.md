# widget-data-feed Specification

## Purpose

定義宿主核心如何以「資料通道」取得各類資料、驗證內容，並以一致的快照與更新通知供應給訂閱的小工具，使小工具不需自行讀檔、彼此資料同步，且日後可加入新資料源而不修改核心。

## Requirements

### Requirement: 資料通道

宿主 SHALL 以具名的資料通道組織資料，每個通道有獨立的來源與快照。本 change SHALL 提供通道 `tw-events`（來源為資料目錄中的 `tw_events.json`，即現有資料層產出格式）與五個擴充通道 `custom1`–`custom5`（來源為資料目錄中的 `custom1.json`–`custom5.json`，內容為任意合法 JSON）。資料通道 MUST NOT 改寫任何來源檔；`tw_events.json` 由宿主內建的資料抓取（見 `market-data-fetch`）或外部資料層產生，資料通道只讀取。

#### Scenario: 讀取資料層產出

- **WHEN** 資料目錄中存在由宿主資料抓取或 `update_tw_events.py` 產出的 `tw_events.json`
- **THEN** 通道 `tw-events` 的快照為其內容

#### Scenario: 擴充通道讀取任意 JSON

- **WHEN** 使用者把一份合法 JSON 存為資料目錄中的 `custom1.json`
- **THEN** 通道 `custom1` 的快照為其內容，其他通道不受影響

### Requirement: 資料目錄

資料目錄 SHALL 預設為使用者本機應用程式資料夾下的 `data` 子目錄，並 SHALL 可在設定視窗中修改；修改後所有通道改從新目錄讀取。

#### Scenario: 指向 Python 資料層的輸出夾

- **WHEN** 使用者在設定中把資料目錄改為資料層的輸出資料夾
- **THEN** 60 秒內通道 `tw-events` 載入該資料夾的 `tw_events.json`

### Requirement: 偵測資料更新

宿主 SHALL 至少每 30 秒檢查一次各通道的來源檔是否變更；有變更時 SHALL 重新載入並通知訂閱該通道的小工具，未變更時 MUST NOT 通知。

#### Scenario: 資料層寫入新檔

- **WHEN** 資料層以原子寫入更新 `tw_events.json`
- **THEN** 60 秒內所有訂閱 `tw-events` 的小工具顯示新資料

#### Scenario: 檔案未變

- **WHEN** 連續多次檢查時來源檔皆未變
- **THEN** 小工具不收到更新通知、不重繪

### Requirement: 拒收無效資料並保留上一份好資料

來源檔無法解析時，宿主 SHALL 保留該通道上一份有效快照。通道 `tw-events` 另 SHALL 拒收「合法但全空」的內容（所有事件、行情、總經清單皆為空）。

#### Scenario: 資料檔損壞

- **WHEN** 已有有效快照後，來源檔被寫成無法解析的內容
- **THEN** 小工具繼續顯示上一份有效資料

#### Scenario: 全空資料

- **WHEN** 通道 `tw-events` 已有非空快照後，`tw_events.json` 變成結構合法但所有清單皆為空
- **THEN** 小工具繼續顯示上一份非空資料

### Requirement: 尚無資料

某通道從未取得有效快照時，宿主 SHALL 回報該通道為「尚無資料」，訂閱的小工具 SHALL 顯示提示而非空白。

#### Scenario: 首次啟動且資料目錄為空

- **WHEN** 資料目錄中沒有 `tw_events.json`
- **THEN** 財經小工具顯示「尚無資料」提示與目前的資料目錄路徑

### Requirement: 小工具取得當前快照

任何小工具開啟或重新載入時 SHALL 能立即取得其訂閱通道的當前快照與設定，不需等待下一次更新通知。

#### Scenario: 新開啟的小工具

- **WHEN** 使用者在設定中開啟一個原本關閉的小工具
- **THEN** 該小工具一出現就顯示與其他訂閱同通道小工具相同的資料

### Requirement: 資料新鮮度可見

通道 `tw-events` 的快照 SHALL 保留資料檔中的 `fetched` 與 `updated` 時間戳記，使小工具能依 Lively 版規則顯示資料更新時間與「已 N 天未更新」提示。

#### Scenario: 資料過期

- **WHEN** `tw_events.json` 的 `fetched` 距今超過 3 天
- **THEN** 台股動態事件小工具的頁腳標示已多少天未更新
