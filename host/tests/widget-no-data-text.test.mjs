// host/tests/widget-no-data-text.test.mjs
//
// fetch-progress-and-manual-refresh task 3.4（design.md D4／D6；specs/widget-data-feed「尚無資料」）：
// 總經日曆、台股動態事件、行情條三個模組在 empty 快照下的顯示——
//   - 總經日曆、台股動態事件的無資料文字改用 `common.fetchActivityView`（依抓取狀態區分：抓取已關閉
//     ＋資料目錄、最近一輪失敗＋產生 log.txt、其餘＋資料目錄），抓取狀態或設定改變時跟著更新，
//     有資料時不因抓取狀態改變而重畫（不打斷捲動位置）。
//   - 行情條無資料一律整個隱藏、不畫文字（widget-host-windows「行情條無資料」），抓取中的提示由
//     外殼的整塊載入負責。
//
// 做法同 custom-null-snapshot.test.mjs：直接 import 真的模組與 host/ui/common.js，餵最小的假
// document／container／ctx。
//
// 執行：node --test host/tests/widget-no-data-text.test.mjs

import test from 'node:test';
import assert from 'node:assert/strict';

import * as common from '../ui/common.js';
import { mount as mountMacro } from '../ui/widgets/macro.js';
import { mount as mountDynamic } from '../ui/widgets/dynamic.js';
import { mount as mountQuotes } from '../ui/widgets/quotes.js';

function makeElementStub(tag) {
  const el = {
    tagName: tag,
    className: '',
    id: '',
    style: {},
    textContent: '',
    _children: [],
    _html: '',
    _writes: 0,
    classList: { add() {}, remove() {}, toggle() {} },
    append(...kids) {
      this._children.push(...kids);
    },
    appendChild(kid) {
      this._children.push(kid);
      return kid;
    },
    insertAdjacentHTML(_pos, html) {
      this._html += html;
      this._writes++;
    },
    querySelector() {
      return null;
    },
    querySelectorAll() {
      return [];
    },
  };
  Object.defineProperty(el, 'innerHTML', {
    get() {
      return this._html;
    },
    set(v) {
      this._html = v;
      this._writes++;
      this._children = [];
    },
  });
  return el;
}

globalThis.document = {
  hidden: false,
  createElement: (tag) => makeElementStub(tag),
  addEventListener() {},
  removeEventListener() {},
  documentElement: { style: { setProperty() {} } },
};
globalThis.window = { __TEST_TODAY: '2026-09-28' };
// macro.js／dynamic.js 掛 60 秒的週期計時器：斷言失敗而沒走到 unmount 時，計時器不得讓行程一直不結束。
const realSetInterval = globalThis.setInterval;
globalThis.setInterval = (fn, ms) => realSetInterval(fn, ms).unref();

const DIR = 'D:/data/fc';
const EMPTY = { channel: 'tw-events', status: 'empty', data: null, meta: null };
const act = (running, enabled, last) => ({ running, enabled, last });

function mountWith(mount, { snapshot = EMPTY, activity = null, settings = { data_dir: DIR } } = {}) {
  const container = makeElementStub('div');
  const handlers = { data: [], settings: [], fetch: [], pause: [] };
  let current = activity;
  const ctx = {
    common,
    config: { id: 'x', channel: 'tw-events', needsData: true },
    snapshot,
    settings,
    onData: (fn) => handlers.data.push(fn),
    onSettings: (fn) => handlers.settings.push(fn),
    onPause: (fn) => handlers.pause.push(fn),
    fetchActivity: () => current,
    onFetchActivity: (fn) => handlers.fetch.push(fn),
  };
  const unmount = mount(container, ctx);
  return {
    container,
    unmount,
    setActivity(next) {
      current = next;
      handlers.fetch.forEach((fn) => fn(next));
    },
    setSettings(next) {
      handlers.settings.forEach((fn) => fn(next));
    },
  };
}

// 清單模組：container 子元素為 header、清單、頁腳（macro／dynamic 皆是 append(header, list, foot)）。
const listHtml = (w) => w.container._children[1].innerHTML;

const CASES = [
  ['抓取已關閉', act(false, false, 'ok'), (t) => /^尚無資料/.test(t) && /抓取已關閉/.test(t) && t.includes(DIR)],
  ['最近一輪失敗', act(false, true, 'failed'), (t) => /^尚無資料/.test(t) && /系統匣「產生 log\.txt」/.test(t)],
  ['成功過但目前沒有資料', act(false, true, 'ok'), (t) => t === `尚無資料 —— 資料目錄：${DIR}`],
  ['活動狀態未知', null, (t) => t === `尚無資料 —— 資料目錄：${DIR}`],
];

const emptyText = (html) => {
  const m = /^<div class="empty">([\s\S]*)<\/div>$/.exec(html);
  return m ? m[1] : null;
};

for (const [name, mount] of [
  ['總經日曆', mountMacro],
  ['台股動態事件', mountDynamic],
]) {
  test(`${name}：empty 快照下的文字依抓取狀態區分`, () => {
    for (const [label, activity, ok] of CASES) {
      const w = mountWith(mount, { activity });
      const text = emptyText(listHtml(w));
      w.unmount();
      assert.ok(text !== null, `${label}：要有 .empty 提示，實際 ${listHtml(w)}`);
      assert.ok(ok(text), `${label}：${text}`);
      if (activity?.last !== 'failed') {
        assert.doesNotMatch(text, /連線失敗|失敗/, `${label}：未失敗時不宣稱失敗`);
      }
    }
  });

  test(`${name}：無資料時抓取狀態改變就更新文字（整塊載入結束後看到的是最新狀態）`, () => {
    const w = mountWith(mount, { activity: act(true, true, 'none') });
    w.setActivity(act(false, true, 'failed'));
    assert.match(emptyText(listHtml(w)), /產生 log\.txt/);
    w.setActivity(act(false, false, 'failed'));
    assert.match(emptyText(listHtml(w)), /抓取已關閉/);
    w.unmount();
  });

  test(`${name}：無資料時資料目錄設定改變就更新路徑`, () => {
    const w = mountWith(mount, { activity: act(false, false, 'ok'), settings: { data_dir: 'A:/old' } });
    assert.ok(emptyText(listHtml(w)).includes('A:/old'));
    w.setSettings({ data_dir: 'B:/new' });
    assert.ok(emptyText(listHtml(w)).includes('B:/new'));
    w.unmount();
  });
}

test('總經日曆：有資料時抓取狀態改變不重畫清單（不打斷捲動位置）', () => {
  const snapshot = {
    channel: 'tw-events',
    status: 'ok',
    data: { macro: [], updated: '2026-09-28 00:00' },
    meta: { loadedAt: 1 },
  };
  const w = mountWith(mountMacro, { snapshot, activity: act(false, true, 'ok') });
  const list = w.container._children[1];
  const writes = list._writes;
  w.setActivity(act(true, true, 'ok'));
  w.setActivity(act(false, true, 'ok'));
  assert.equal(list._writes, writes);
  w.unmount();
});

test('台股動態事件：有資料時抓取狀態改變不重畫清單', () => {
  const snapshot = {
    channel: 'tw-events',
    status: 'ok',
    data: { events: [], punish: [], holidays: [], fetched: '2026-09-28 00:00' },
    meta: { loadedAt: 1 },
  };
  const w = mountWith(mountDynamic, { snapshot, activity: act(false, true, 'ok') });
  const list = w.container._children[1];
  const writes = list._writes;
  w.setActivity(act(true, true, 'ok'));
  assert.equal(list._writes, writes);
  w.unmount();
});

test('總經日曆：有資料檔但沒有總經事件（總經來源從未取得資料）沿用原本的提示', () => {
  const snapshot = { channel: 'tw-events', status: 'ok', data: { macro: [] }, meta: { loadedAt: 1 } };
  const w = mountWith(mountMacro, { snapshot, activity: act(false, true, 'ok') });
  assert.match(emptyText(listHtml(w)), /^尚無總經資料/);
  w.unmount();
});

test('行情條：empty 快照下不論抓取狀態都整個隱藏、不畫文字（抓取中由外殼整塊載入）', () => {
  for (const [label, activity] of [
    ['進行中', act(true, true, 'none')],
    ...CASES.map(([l, a]) => [l, a]),
  ]) {
    const w = mountWith(mountQuotes, { activity });
    assert.equal(w.container.style.display, 'none', label);
    assert.equal(w.container.innerHTML, '', label);
    w.setActivity(act(false, true, 'failed'));
    assert.equal(w.container.style.display, 'none', `${label}：狀態改變後仍隱藏`);
    w.unmount();
  }
});
