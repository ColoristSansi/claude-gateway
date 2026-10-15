import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './App';
import './app.css';

// shortcut: 壳层（导航 / 横幅 / 侧栏徽标 / 设计体系）归票 52；这里先挂上「日志与调试」这一页
// （票 53），52 落地后以它的壳为准。
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
