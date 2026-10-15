// 概览页（spec §10 第 1 节）：三步清单 + 网关状态与端口 + 今日用量 + 最近错误（折叠）
// + 两个一键动作（批量切客户端、三步全绿后的 haiku 端到端验收）。

import { useCallback, useEffect, useState } from 'react';

import { api, type AcceptanceResult, type RecentError, type UsageSummary } from '../api';
import { ErrorBlock, formatTime, money, Pill } from '../components';
import type { ShellCtx } from '../App';

type Step = { key: string; title: string; done: boolean; detail: string; page: 'upstreams' | 'models' | 'clients'; hint: string };

export function OverviewPage({ ctx }: { ctx: ShellCtx }) {
  const [errors, setErrors] = useState<RecentError[]>([]);
  const [usage, setUsage] = useState<UsageSummary | null>(null);
  const [acceptance, setAcceptance] = useState<AcceptanceResult | null>(null);
  const [slotsBound, setSlotsBound] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [switchResult, setSwitchResult] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [e, u, b] = await Promise.all([
        api.recentErrors(20).catch(() => ({ errors: [] as RecentError[] })),
        api.usageSummary('today', 'virtual_model').catch(() => null),
        api.bindings('global').catch(() => null),
      ]);
      setErrors(e.errors);
      setUsage(u);
      setSlotsBound(!!b && b.slots.every((slot) => slot.binding !== null));
    } catch (err) {
      setError(err);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const badges = ctx.badges;

  // 就绪判据（spec §10 第 1 节）：① 上游至少一条已保存且自检通过；② 四行槽位都绑定且探针通过
  // （探针是「进去测」，清单只看「绑了吗」+ 上次结果）；③ 三个客户端至少一个「已生效」。
  const steps: Step[] = [
    {
      key: 'upstream',
      title: '① 加上游并自检通过',
      done: !!badges && badges.upstreamsTotal > 0 && badges.upstreamsFailedSelfcheck === 0,
      detail: badges
        ? `${badges.upstreamsTotal} 条上游，其中 ${badges.upstreamsFailedSelfcheck} 条没通过自检`
        : '还没法读上游列表',
      page: 'upstreams',
      hint: 'wizard',
    },
    {
      key: 'binding',
      title: '② 四个家族槽位都绑定',
      done: slotsBound,
      detail: '槽位表在「模型与映射」，每行有测试按钮',
      page: 'models',
      hint: 'slots',
    },
    {
      key: 'client',
      title: '③ 至少一个客户端切到网关',
      done: !!badges && badges.clientsNotEffective < 3,
      detail: badges ? `${3 - badges.clientsNotEffective} / 3 个客户端已生效` : '还没法读客户端状态',
      page: 'clients',
      hint: 'cards',
    },
  ];

  const allGreen = steps.every((s) => s.done);

  const switchAll = async () => {
    setBusy('switch-all');
    setError(null);
    setSwitchResult(null);
    try {
      const res = await api.switchAll();
      setSwitchResult(res.message + (res.results.some((r) => r.error) ? '（有失败的，见下方）' : ''));
      await ctx.refresh();
      await load();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const runAcceptance = async () => {
    setBusy('acceptance');
    setError(null);
    try {
      setAcceptance(await api.acceptanceRun());
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="page">
      <h2>概览</h2>

      <ol className="checklist">
        {steps.map((s) => (
          <li key={s.key} className={s.done ? 'done' : 'todo'}>
            <div>
              <div className="step-title">{s.title}</div>
              <div className="muted">{s.detail}</div>
            </div>
            {s.done ? (
              <Pill tone="ok">已就绪</Pill>
            ) : (
              <button className="primary" onClick={() => ctx.goto(s.page, s.hint)}>
                去处理
              </button>
            )}
          </li>
        ))}
      </ol>

      <div className="grid2">
        <div className="card">
          <h3>网关状态</h3>
          {ctx.status ? (
            <ul className="kv">
              <li>
                <span>运行中</span>
                <b>是</b>
              </li>
              <li>
                <span>端口</span>
                <b>{ctx.status.port}</b>
              </li>
              <li>
                <span>版本</span>
                <b>{ctx.status.version}</b>
              </li>
              <li>
                <span>已运行</span>
                <b>{Math.floor(ctx.status.uptimeSeconds / 60)} 分钟</b>
              </li>
            </ul>
          ) : (
            <p className="muted">网关未运行：配置页仍可编辑，数据页等它起来。</p>
          )}
        </div>

        <div className="card">
          <h3>今日用量</h3>
          {usage ? (
            <>
              <ul className="kv">
                <li>
                  <span>请求数</span>
                  <b>{usage.totals.requests}</b>
                </li>
                <li>
                  <span>花费</span>
                  <b>{money(usage.totals.cost_usd, usage.rate)}</b>
                </li>
                <li>
                  <span>输入 / 输出 token</span>
                  <b>
                    {usage.totals.input_tokens} / {usage.totals.output_tokens}
                  </b>
                </li>
              </ul>
              {(usage.totals.no_price > 0 || usage.totals.token_unknown > 0) && (
                <p className="muted">
                  {usage.totals.no_price} 次请求无价格 / {usage.totals.token_unknown} 次请求 token 未知
                  （不计入总额）
                </p>
              )}
              {usage.rows.length > 0 && (
                <table className="table">
                  <thead>
                    <tr>
                      <th>虚拟模型</th>
                      <th>请求数</th>
                      <th>花费</th>
                    </tr>
                  </thead>
                  <tbody>
                    {usage.rows.map((r) => (
                      <tr key={r.key}>
                        <td>{r.key ?? '（未知）'}</td>
                        <td>{r.requests}</td>
                        <td>{money(r.cost_usd, usage.rate)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </>
          ) : (
            <p className="muted">读不到用量（网关未运行或库还是空的）。</p>
          )}
        </div>
      </div>

      <div className="actions">
        <button className="primary" onClick={switchAll} disabled={busy !== null}>
          {busy === 'switch-all' ? '处理中…' : '一键把可用客户端都切到网关'}
        </button>
        <button onClick={runAcceptance} disabled={busy !== null || !allGreen} title={allGreen ? '' : '三步全绿之后才能跑'}>
          {busy === 'acceptance' ? '验收中…' : '跑一次端到端验收（haiku 槽位）'}
        </button>
        <button onClick={() => void load()} disabled={busy !== null}>
          刷新
        </button>
      </div>
      {switchResult && <p className="notice-line">{switchResult}</p>}
      {!allGreen && <p className="muted">端到端验收要等三步都绿（这是刻意的：它走真实链路）。</p>}

      {acceptance && (
        <div className={acceptance.ok ? 'card ok-card' : 'card bad-card'}>
          <h3>端到端验收结果</h3>
          <ul className="kv">
            <li>
              <span>槽位</span>
              <b>{acceptance.slot}</b>
            </li>
            <li>
              <span>上游</span>
              <b>{acceptance.upstream ?? '—'}</b>
            </li>
            <li>
              <span>状态码</span>
              <b>{acceptance.status ?? '—'}</b>
            </li>
            <li>
              <span>耗时</span>
              <b>{acceptance.elapsedMs} ms</b>
            </li>
            <li>
              <span>首字延迟</span>
              <b>{acceptance.firstByteMs ?? '—'} ms</b>
            </li>
          </ul>
          {!acceptance.ok && (
            <ErrorBlock
              error={acceptance.template}
              onAction={(kind, page) => {
                if (kind === 'goto' && page) ctx.goto(page as never);
              }}
            />
          )}
          <details>
            <summary>原始响应</summary>
            <pre className="error-raw">{acceptance.raw}</pre>
          </details>
        </div>
      )}

      <details className="card" open={false}>
        <summary>最近错误（{errors.length}）</summary>
        {errors.length === 0 ? (
          <p className="muted">没有请求错误。拉取 / 自检的提示不算错误，它们在自己的页面上。</p>
        ) : (
          <ul className="errors">
            {errors.map((e) => (
              <li key={`${e.at}-${e.requestId ?? e.status}`}>
                <div className="error-line">
                  <span className="muted">{formatTime(e.at)}</span>
                  <b>{e.status || '—'}</b>
                  <span>
                    {e.virtualModel ?? '—'} → {e.upstream ?? '—'} / {e.realModel ?? '—'}
                  </span>
                  {e.requestId && <span className="muted">#{e.requestId}</span>}
                </div>
                <ErrorBlock error={e.template} compact />
                <details>
                  <summary>原始 JSON</summary>
                  <pre className="error-raw">{e.raw}</pre>
                </details>
              </li>
            ))}
          </ul>
        )}
      </details>

      <ErrorBlock error={error} />
    </section>
  );
}
