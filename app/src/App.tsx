// 七页的壳（顺序逐字照 spec §10 / 票 17：概览、上游、模型与映射、客户端、日志与调试、
// 用量与成本、设置；不合并、不增设）。
//
// shortcut: 票 52 的正式壳层（导航 / 横幅 / 侧栏徽标 / 样式体系）还没并进来，这里先用一个
// 最小 hash 路由把**日志与调试**这一页挂上；52 落地后以它的壳为准，本文件让位。
// 其余六页在 M2 / M3 各自落地，这里只留占位。

import { useEffect, useState, type ReactElement } from 'react';
import { LogsPage } from './pages/LogsPage';
import { UpdateBanner } from './UpdateBanner';

type Page = { slug: string; title: string; render?: () => ReactElement; owner: string };

const PAGES: Page[] = [
  { slug: 'overview', title: '概览', owner: '票 52' },
  { slug: 'upstreams', title: '上游', owner: '票 52' },
  { slug: 'models', title: '模型与映射', owner: '票 52' },
  { slug: 'clients', title: '客户端', owner: '票 52' },
  { slug: 'logs', title: '日志与调试', owner: '票 53', render: () => <LogsPage /> },
  { slug: 'usage', title: '用量与成本', owner: '票 59' },
  { slug: 'settings', title: '设置', owner: '票 59' },
];

function currentSlug(): string {
  const hash = window.location.hash.replace(/^#\/?/, '');
  return PAGES.some((p) => p.slug === hash) ? hash : 'logs';
}

export function App() {
  const [slug, setSlug] = useState(currentSlug());

  useEffect(() => {
    const onHash = () => setSlug(currentSlug());
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const page = PAGES.find((p) => p.slug === slug) ?? PAGES[0];

  return (
    <div className="app">
      <nav className="nav">
        <span className="brand">Claude Gateway</span>
        {PAGES.map((p) => (
          <a
            key={p.slug}
            href={`#/${p.slug}`}
            className={p.slug === slug ? 'active' : undefined}
          >
            {p.title}
          </a>
        ))}
      </nav>
      {/* 升级横幅（票 61）：有新版 / 正在静默安装 / 升级失败时才出现。 */}
      <UpdateBanner />
      <main>
        {page.render ? (
          page.render()
        ) : (
          <section className="page">
            <h2>{page.title}</h2>
            <p className="muted">这一页归{page.owner}，还没落地。</p>
          </section>
        )}
      </main>
    </div>
  );
}
