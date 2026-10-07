// host/ui/widgets/custom3.js
//
// 擴充插槽 custom3 的薄包裝（design.md D6，task 4.6）：實際內容全部在 custom.js（五個
// 插槽共用同一份實作，理由見該檔檔頭註解），本檔只是讓 widget.html 的
// `import('./widgets/${id}.js')`（依 id 找檔名）能找到 custom3 這個 id 對應的模組。
// 日後要幫 custom3 接上專屬資料源時，把下面這行換成真正的實作即可，不影響其餘四個插槽。

export { mount } from './custom.js';
