// 用量与成本页（spec §10 第 6 条，票 59）。
//
// 五个时间范围（今日 / 近 7 天 / 近 30 天 / 全部 / 自定义区间，日边界按**本机本地时区自然日**，
// 自定义按天聚合）× 三个维度（虚拟模型 / 上游 / 客户端）+ 按天明细表；
// 花费 / 请求数 / token 的图表；人民币为主、可切美元（全局显示态，票 33 第 2 节）。
//
// 「无价格」与「token 未知」**单独计数、不计入总额**（票 33 第 1 节）：总额那一行下面专门
// 说明这两句话，不显示 0、不留空。

import { useCallback, useEffect, useMemo, useState } from 'react';

import { api, ApiError, type UsageReport, type UsageRow } from '../api';
import { CurrencySwitch, money } from '../components';
import { useCurrency } from '../currency';

/** 五个范围（逐字照 spec §10 第 6 条 / 票 59）。 */
const RANGES: { id: 'today' | '7d' | '30d' | 'all' | 'custom'; label: string }[] = [
  { id: 'today', label: '今日' },
  { id: '7d', label: '近 7 天' },
  { id: '30d', label: '近 30 天' },
  { id: 'all', label: '全部' },
  { id: 'custom', label: '自定义区间' },
];

/** 三个维度（顺序照 spec：虚拟模型 / 上游 / 客户端，票 33 第 6 节）。 */
const DIMS: { id: 'virtual_model' | 'upstream' | 'client'; label: string }[] = [
  { id: 'virtual_model', label: '按虚拟模型' },
  { id: 'upstream', label: '按上游' },
  { id: 'client', label: '按客户端' },
];

function localDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function tokens(row: { input_tokens: number; output_tokens: number; cache_read: number; cache_write: number }): string {
  const parts = [`入 ${row.input_tokens}`, `出 ${row.output_tokens}`];
  if (row.cache_read) parts.push(`缓存读 ${row.cache_read}`);
  if (row.cache_write) parts.push(`缓存写 ${row.cache_write}`);
  return parts.join(' · ');
}

/** 一条横向条形：宽度按占最大值的比例（没有图表库，也不必引一个）。 */
function Bar({ value, max, label }: { value: number; max: number; label: string }) {
  const pct = max > 0 ? Math.max(2, Math.round((value / max) * 100)) : 0;
  return (
    <div className="bar-row">
      <span className="bar-label" title={label}>{label}</span>
      <span className="bar-track">
        <span className="bar-fill" style={{ width: `${pct}%` }} />
      </span>
      <span className="bar-value">{value.toLocaleString()}</span>
    </div>
  );
}

export function UsagePage() {
  const currency = useCurrency();
  const [range, setRange] = useState<'today' | '7d' | '30d' | 'all' | 'custom'>('today');
  const [dim, setDim] = useState<'virtual_model' | 'upstream' | 'client'>('virtual_model');
  const today = useMemo(() => localDate(new Date()), []);
  const [from, setFrom] = useState(today);
  const [to, setTo] = useState(today);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const r = await api.usage(range, dim, range === 'custom' ? from : undefined, range === 'custom' ? to : undefined);
      setReport(r);
      setError(null);
    } catch (e) {
      setError(e);
      setReport(null);
    } finally {
      setBusy(false);
    }
  }, [range, dim, from, to]);

  useEffect(() => {
    void load();
  }, [load]);

  const rate = report?.rate ?? null;
  const rows = report?.rows ?? [];
  const totals = report?.totals;
  const dimLabel = DIMS.find((d) => d.id === dim)?.label ?? '';
  const maxCost = Math.max(0, ...rows.map((r) => r.cost_usd ?? 0));
  const maxReq = Math.max(0, ...rows.map((r) => r.requests));
  const maxTok = Math.max(0, ...rows.map((r) => r.input_tokens + r.output_tokens + r.cache_read + r.cache_write));

  return (
    <section className="page">
      <header className="page-head">
        <h2>用量与成本</h2>
        <CurrencySwitch />
      </header>

      <div className="filters">
        <label>
          时间范围
          <select value={range} onChange={(e) => setRange(e.target.value as typeof range)}>
            {RANGES.map((r) => (
              <option key={r.id} value={r.id}>{r.label}</option>
            ))}
          </select>
        </label>
        {range === 'custom' && (
          <>
            <label>
              开始（含）
              <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
            </label>
            <label>
              结束（含当天）
              <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
            </label>
          </>
        )}
        <label>
          维度
          <select value={dim} onChange={(e) => setDim(e.target.value as typeof dim)}>
            {DIMS.map((d) => (
              <option key={d.id} value={d.id}>{d.label}</option>
            ))}
          </select>
        </label>
        <button type="button" onClick={() => void load()} disabled={busy}>
          {busy ? '读取中…' : '刷新'}
        </button>
        <span className="muted">日边界按本机本地时区自然日</span>
      </div>

      {error !== null && (
        <p className="error">
          {error instanceof ApiError ? `${error.shape.what}：${error.shape.likelyCause}` : String(error)}
        </p>
      )}

      {totals && (
        <div className="cards">
          <div className="card">
            <h3>总计（{dimLabel}）</h3>
            <ul className="kv">
              <li>
                <span>花费</span>
                <b>{money(totals.cost_usd, rate, currency)}</b>
              </li>
              <li>
                <span>请求数</span>
                <b>{totals.requests}</b>
              </li>
              <li>
                <span>token</span>
                <b>{tokens(totals)}</b>
              </li>
            </ul>
            {(totals.no_price > 0 || totals.token_unknown > 0) && (
              <p className="muted">
                {totals.no_price} 次请求无价格 / {totals.token_unknown} 次请求 token 未知（
                <b>不计入总额</b>）
              </p>
            )}
            {totals.cost_usd === null && totals.requests > 0 && (
              <p className="muted">这一段一条都算不出价：价格缺失的请求不计入总额。</p>
            )}
          </div>

          <div className="card">
            <h3>图表 · 花费</h3>
            {rows.length === 0 ? (
              <p className="muted">这一段还没有请求。</p>
            ) : (
              rows.map((r) => (
                <Bar
                  key={`c-${r.key}`}
                  label={r.key ?? '（未知）'}
                  value={r.cost_usd ?? 0}
                  max={maxCost}
                />
              ))
            )}
            <h3>图表 · 请求数</h3>
            {rows.map((r) => (
              <Bar key={`r-${r.key}`} label={r.key ?? '（未知）'} value={r.requests} max={maxReq} />
            ))}
            <h3>图表 · token</h3>
            {rows.map((r) => (
              <Bar
                key={`t-${r.key}`}
                label={r.key ?? '（未知）'}
                value={r.input_tokens + r.output_tokens + r.cache_read + r.cache_write}
                max={maxTok}
              />
            ))}
            <p className="muted">条形按最大值归一；无价格的组显示 0（明细里标「未知」）。</p>
          </div>
        </div>
      )}

      <h3>按 {dimLabel.replace(/^按/, '')} 分组</h3>
      <table className="table">
        <thead>
          <tr>
            <th>分组</th>
            <th>请求数</th>
            <th>花费</th>
            <th>输入</th>
            <th>输出</th>
            <th>缓存读</th>
            <th>缓存写</th>
            <th>无价格</th>
            <th>token 未知</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r: UsageRow) => (
            <tr key={r.key ?? '(null)'}>
              <td>{r.key ?? '（未知）'}</td>
              <td>{r.requests}</td>
              <td>{money(r.cost_usd, rate, currency)}</td>
              <td>{r.input_tokens}</td>
              <td>{r.output_tokens}</td>
              <td>{r.cache_read}</td>
              <td>{r.cache_write}</td>
              <td>{r.no_price || '—'}</td>
              <td>{r.token_unknown || '—'}</td>
            </tr>
          ))}
          {rows.length === 0 && (
            <tr>
              <td colSpan={9} className="muted">这一段没有请求。</td>
            </tr>
          )}
        </tbody>
      </table>

      <h3>按天明细</h3>
      <table className="table">
        <thead>
          <tr>
            <th>日期</th>
            <th>请求数</th>
            <th>花费</th>
            <th>输入</th>
            <th>输出</th>
            <th>缓存读</th>
            <th>缓存写</th>
            <th>无价格</th>
            <th>token 未知</th>
          </tr>
        </thead>
        <tbody>
          {(report?.daily ?? []).map((d) => (
            <tr key={d.day}>
              <td>{d.day}</td>
              <td>{d.requests}</td>
              <td>{money(d.cost_usd, rate, currency)}</td>
              <td>{d.input_tokens}</td>
              <td>{d.output_tokens}</td>
              <td>{d.cache_read}</td>
              <td>{d.cache_write}</td>
              <td>{d.no_price || '—'}</td>
              <td>{d.token_unknown || '—'}</td>
            </tr>
          ))}
          {(report?.daily ?? []).length === 0 && (
            <tr>
              <td colSpan={9} className="muted">这一段没有请求。</td>
            </tr>
          )}
        </tbody>
      </table>

      <p className="muted">
        口径（票 33）：价格按请求时冻结，改价与刷新快照不重算历史；改汇率会让历史人民币跟着变。
        归集口径是虚拟模型，上游与客户端是另两个维度。
      </p>
    </section>
  );
}
