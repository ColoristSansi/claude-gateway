import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';
import { RenderBoundary } from './components';
import './app.css';

// shortcut: 壳层（导航 / 横幅 / 侧栏徽标 / 设计体系）归票 52；这里先挂上「日志与调试」这一页
// （票 53），52 落地后以它的壳为准。
// 边界在 `App` **外面**：渲染期抛异常时 React 会卸掉整棵树（页面全白，只能退出重启），
// 边界得包住 App 自己才有机会顶上错误模板（票 60b）。
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <RenderBoundary>
      <App />
    </RenderBoundary>
  </StrictMode>,
);
