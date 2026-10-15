// 七页的壳（顺序逐字照 spec §10 / 票 17：概览、上游、模型与映射、客户端、日志与调试、
// 用量与成本、设置；不合并、不增设）。
//
// 壳层归票 52：导航 / 顶部横幅（未运行、端口被占）/ 侧栏徽标 / 二次确认 / 错误模板 / 样式。
// 日志与调试页是票 53 的（`pages/LogsPage.tsx`，原样挂在第 5 项上），用量与成本与设置页
// 是票 59 的，这里只放「待实现」占位。
//
// 未运行态：`/api/*` 全部连不上 → 顶部常驻横幅 + 「启动网关」；配置类页面照旧可编辑
// （前端本来就只是把改动发给后端，只是发不出去时会如实报错）。
// 端口被占：横幅给「换端口」（探测空闲端口**预填** → 用户确认 → 才写入），换完自动同步
// 已生效客户端的地址。

import { useCallback, useEffect, useMemo, useState, type ReactElement } from 'react';

import { api, ApiError, API_BASE, setApiBase, type Badges, type GatewayStatus } from './api';
import { ConfirmDialog, ErrorBlock, type ConfirmRequest, type PageSlug } from './components';
import { ClientsPage } from './pages/ClientsPage';
import { LogsPage } from './pages/LogsPage';
import { ModelsPage } from './pages/ModelsPage';
import { OverviewPage } from './pages/OverviewPage';
import { SettingsPage } from './pages/SettingsPage';
import { UpstreamsPage } from './pages/UpstreamsPage';
import { UpdateBanner } from './UpdateBanner';
import { UsagePage } from './pages/UsagePage';

type PageDef = {
  slug: PageSlug;
  title: string;
  /** 侧栏徽标挂在哪一项上：`upstreams` = 未通过自检的上游数，`clients` = 未生效的客户端数。 */
  badge?: 'upstreams' | 'clients';
  render?: (ctx: ShellCtx) => ReactElement;
  owner?: string;
  /** 概览三步清单点进来时的页内锚点提示。 */
  hint?: string;
};

export type ShellCtx = {
  /** 跳到某页，可选带一个「去哪一块」的提示（概览三步清单用）。 */
  goto: (page: PageSlug, hint?: string) => void;
  /** 弹一次二次确认；确认了才继续。 */
  confirm: (req: ConfirmRequest) => Promise<boolean>;
  /** 后端 / 前端任何地方要报错都走它（统一模板）。 */
  reportError: (e: unknown) => void;
  /** 网关当前状态（拿不到就是 null = 未运行）。 */
  status: GatewayStatus | null;
  /** 重新读一次状态与徽标（客户端写完文件后调）。 */
  refresh: () => Promise<void>;
  badges: Badges | null;
  /** 概览三步清单点进来的落点提示。 */
  hint: string | null;
};

const PAGES: PageDef[] = [
  { slug: 'overview', title: '概览', render: (ctx) => <OverviewPage ctx={ctx} /> },
  { slug: 'upstreams', title: '上游', badge: 'upstreams', render: (ctx) => <UpstreamsPage ctx={ctx} /> },
  { slug: 'models', title: '模型与映射', render: (ctx) => <ModelsPage ctx={ctx} /> },
  { slug: 'clients', title: '客户端', badge: 'clients', render: (ctx) => <ClientsPage ctx={ctx} /> },
  { slug: 'logs', title: '日志与调试', render: (ctx) => <LogsPage confirm={ctx.confirm} /> },
  { slug: 'usage', title: '用量与成本', render: () => <UsagePage /> },
  { slug: 'settings', title: '设置', render: () => <SettingsPage /> },
];

function currentHash(): { slug: PageSlug; hint: string | null } {
  const raw = window.location.hash.replace(/^#\/?/, '');
  const [slug, hint] = raw.split('?');
  const hit = PAGES.find((p) => p.slug === slug);
  return { slug: hit ? hit.slug : 'overview', hint: hint ?? null };
}

/** 外壳 IPC：只有 Tauri 里有。启动 / 换端口在「网关根本没在跑」时只能靠它。 */
async function ipc<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const g = (window as unknown as { __TAURI_INTERNALS__?: { invoke?: (c: string, a?: unknown) => Promise<T> } })
    .__TAURI_INTERNALS__;
  if (!g?.invoke) throw new Error('没有外壳 IPC（浏览器里打开时做不了这件事）');
  return g.invoke(cmd, args);
}

export function App() {
  const [route, setRoute] = useState(currentHash());
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [badges, setBadges] = useState<Badges | null>(null);
  const [bannerError, setBannerError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dialog, setDialog] = useState<{ req: ConfirmRequest; resolve: (ok: boolean) => void } | null>(null);
  const [portBusy, setPortBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const s = await api.status();
      setStatus(s);
      setBannerError(null);
      try {
        setBadges(await api.badges());
      } catch {
        setBadges(null);
      }
    } catch (e) {
      setStatus(null);
      setBadges(null);
      setBannerError(e instanceof ApiError ? e : null);
    }
  }, []);

  useEffect(() => {
    const onHash = () => setRoute(currentHash());
    window.addEventListener('hashchange', onHash);
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => {
      window.removeEventListener('hashchange', onHash);
      window.clearInterval(timer);
    };
  }, [refresh]);

  const goto = useCallback((page: PageSlug, hint?: string) => {
    window.location.hash = hint ? `#/${page}?${hint}` : `#/${page}`;
    setRoute({ slug: page, hint: hint ?? null });
  }, []);

  const confirm = useCallback(
    (req: ConfirmRequest) =>
      new Promise<boolean>((resolve) => {
        setDialog({ req, resolve });
      }),
    [],
  );

  const reportError = useCallback((e: unknown) => {
    setNotice(e instanceof Error ? e.message : String(e));
  }, []);

  const startGateway = async () => {
    setNotice(null);
    try {
      await api.gatewayStart();
    } catch {
      // 网关没在跑时 `/api/gateway/start` 也发不出去：转外壳 IPC。
      try {
        await ipc('gateway_start');
      } catch (e) {
        reportError(e);
        return;
      }
    }
    setNotice('已让外壳启动网关，稍等几秒…');
    window.setTimeout(() => void refresh(), 1500);
  };

  const changePort = async () => {
    setPortBusy(true);
    setNotice(null);
    try {
      let current = status?.port ?? 8787;
      let suggested: number;
      let free: boolean;
      try {
        const probe = await api.probePort();
        current = probe.currentPort;
        suggested = probe.suggestedPort;
        free = probe.currentPortFree;
      } catch {
        // 网关没在跑：探测也走外壳。
        const probe = await ipc<{ currentPort: number; currentPortFree: boolean; suggestedPort: number }>(
          'gateway_probe_port',
        );
        current = probe.currentPort;
        suggested = probe.suggestedPort;
        free = probe.currentPortFree;
      }
      const ok = await confirm({
        title: `把网关端口从 ${current} 换到 ${suggested}？`,
        action: '换端口并同步客户端',
        body: (
          <div>
            <p>
              端口 {current} 此刻{free ? '空闲' : '**被占用**'}。换端口会：
            </p>
            <ul>
              <li>把新端口写进设置（**不会静默换**：这一步要你确认）；</li>
              <li>自动重写所有「已生效」客户端的地址，未生效的不动；</li>
              <li>让网关按新端口重新监听。</li>
            </ul>
            <p className="muted">Desktop 与 Codex 要完全退出重开才读到新端口。</p>
          </div>
        ),
      });
      if (!ok) return;
      try {
        const res = await api.changePort(suggested);
        setApiBase(res.port);
        setNotice(res.message + '；' + res.restartHint);
      } catch {
        const res = await ipc<{ message: string; port: number }>('gateway_change_port', { port: suggested });
        setApiBase(res.port);
        setNotice(res.message);
      }
      window.setTimeout(() => void refresh(), 1500);
    } catch (e) {
      reportError(e);
    } finally {
      setPortBusy(false);
    }
  };

  const page = useMemo(() => PAGES.find((p) => p.slug === route.slug) ?? PAGES[0], [route.slug]);
  const ctx: ShellCtx = { goto, confirm, reportError, status, refresh, badges, hint: route.hint };

  return (
    <div className="app">
      {bannerError && (
        <div className="banner">
          <span className="banner-text">
            <strong>网关未运行</strong>
            <span className="muted">
              （{bannerError.shape.what}；地址 {API_BASE}）——配置类页面仍可编辑
            </span>
          </span>
          <button className="primary" onClick={startGateway}>
            启动网关
          </button>
          <button onClick={changePort} disabled={portBusy}>
            {portBusy ? '处理中…' : '换端口'}
          </button>
        </div>
      )}

      {/* 升级横幅（票 61）：有新版 / 正在静默安装 / 升级失败时才出现。
          设置页（票 59）那格「关于」直接显示 status.rollback.guide 与 installersDir。 */}
      <UpdateBanner />

      <div className="layout">
        <nav className="nav">
          <span className="brand">Claude Gateway</span>
          {PAGES.map((p) => {
            const count =
              p.badge === 'upstreams'
                ? badges?.upstreamsFailedSelfcheck
                : p.badge === 'clients'
                  ? badges?.clientsNotEffective
                  : 0;
            return (
              <a
                key={p.slug}
                href={`#/${p.slug}`}
                className={p.slug === route.slug ? 'active' : undefined}
                onClick={() => setRoute({ slug: p.slug, hint: null })}
              >
                {p.title}
                {p.badge && count ? <span className="badge">{count}</span> : null}
              </a>
            );
          })}
          <span className="spacer" />
          <span className="muted status-line">
            {status
              ? `网关运行中 · ${status.port} · v${status.version}`
              : '网关未运行'}
          </span>
        </nav>

        <main>
          {notice && (
            <div className="notice" onClick={() => setNotice(null)} title="点一下关掉">
              {notice}
            </div>
          )}
          {page.render ? (
            page.render(ctx)
          ) : (
            <section className="page">
              <h2>{page.title}</h2>
              <p className="muted">这一页归{page.owner}，还没落地（导航项先留着）。</p>
            </section>
          )}
        </main>
      </div>

      <ConfirmDialog
        request={dialog?.req ?? null}
        onDone={(ok) => {
          dialog?.resolve(ok);
          setDialog(null);
        }}
      />
    </div>
  );
}

/** 页面里报错统一用它（页内联提示那一处，spec §10 错误表达的第二处）。 */
export function InlineError({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <ErrorBlock
      error={error instanceof ApiError ? error : { what: String(error), likelyCause: '看原始错误。' }}
    />
  );
}
