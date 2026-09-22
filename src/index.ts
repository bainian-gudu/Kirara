import { createApp } from 'vue';
import App from './App.vue';
import './index.css';

function mount() {
  createApp(App).mount('#root');
}

// 单文件构建会把脚本内联到 <head>。内联脚本上的 defer 不生效，
// 因此这里必须等 DOM 解析完成后再挂载，不能假设 #root 已经存在。
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', mount, { once: true });
} else {
  mount();
}

if (process.env.NODE_ENV !== 'development') {
  window.addEventListener('contextmenu', (e) => {
    e.preventDefault();
  });
  document.addEventListener('keydown', function (event) {
    // 禁止 F5、Ctrl+R（Windows/Linux）和 Command+R（Mac）刷新页面
    if (
      event.key === 'F5' ||
      (event.ctrlKey && event.key === 'r') ||
      (event.metaKey && event.key === 'r')
    ) {
      event.preventDefault();
    }
  });
}
