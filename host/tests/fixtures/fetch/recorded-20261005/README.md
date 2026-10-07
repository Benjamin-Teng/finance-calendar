# 錄製樣本 recorded-20261005

`tests/fetch_oracle/record.py` 實跑 `update_tw_events.py` 一輪錄下的真實來源回應（design.md D4）。自動產生，請勿手改；重錄請新增另一個日期的目錄。

- 錄製時間（台北）：2026-10-05T01:12:14+08:00
- 錄製時間（本機，naive）：2026-10-05T01:12:14
- 不同請求數：28（同一請求多次只留一筆，次數見 manifest 的 `count`）；失敗 1 筆
- `live_output.json`：錄製當下 Python 版的輸出；回放（`expect.py`）必須與它完全相同。

## 失敗的請求

- `GET https://nfs.faireconomy.media/ff_calendar_nextweek.json`：HTTP 404

平日 ForexFactory 下週檔回 404 屬正常（週末才發布）；其餘失敗代表當時來源不可用或被限流，照實保留，回放時重現同樣的錯誤。
