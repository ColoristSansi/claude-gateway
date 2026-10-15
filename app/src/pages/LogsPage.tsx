// 日志与调试页（spec §10 第 5 条，票 53 做 M2 的基础那半）。
//
// 本票落地：元数据表格（时间倒序 50 条）+ 四个筛选 + 模型名 / 请求 ID 搜索 + 每行单次成本
// + 右上角「记录完整 body」开关（默认关、开启需一次隐私确认、1 小时自动关）。
// 详情抽屉四段（概要 / 请求与响应体 / 帧时间轴 / 错误信息）归票 59。

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  api,
  EMPTY_FILTERS,
  type BodyRecording,
  type Filters,
  type LogFilters,
  type LogRow,
} from '../api';

/** 状态码四档的文案（取值域与后端 `obs::STATUS_FILTERS` 一致）。 */
const STATUS_LABEL: Record<string, string> = {
  ok: '成功',
  '4xx': '4xx',
  '5xx': '5xx',
  error: '仅看错误',
};

function time(ts: number): string {
  const d = new Date(ts * 1000);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 金额：库内美元、界面人民币为主（票 33 第 2 节）；未知标「未知」，不足 ¥0.01 显示 `<¥0.01`。 */
function money(usd: number | null, rate: number | null, currency: 'cny' | 'usd'): string {
  if (usd === null || usd === undefined) return '未知';
  if (currency === 'usd') return usd < 0.0001 ? '<$0.0001' : `$${usd.toFixed(4)}`;
  const v = usd * (rate ?? 1);
  if (v === 0) return '¥0.00';
  return v < 0.01 ? '<¥0.01' : `¥${v.toFixed(2)}`;
}

function tokens(row: LogRow): string {
  if (row.input_tokens === null && row.output_tokens === null && row.cache_read === null) {
    return 'token 未知';
  }
  const parts = [`入 ${row.input_tokens ?? '?'}`, `出 ${row.output_tokens ?? '?'}`];
  if (row.cache_read) parts.push(`缓存读 ${row.cache_read}`);
  return parts.join(' · ');
}

export function LogsPage() {
  const [filters, setFilters] = useState<LogFilters>(EMPTY_FILTERS);
  const [applied, setApplied] = useState<LogFilters>(EMPTY_FILTERS);
  const [rows, setRows] = useState<LogRow[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [rate, setRate] = useState<number | null>(null);
  const [candidates, setCandidates] = useState<Filters | null>(null);
  const [currency, setCurrency] = useState<'cny' | 'usd'>('cny');
  const [body, setBody] = useState<BodyRecording | null>(null);
  const [remaining, setRemaining] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 只让最后一次请求写回列表（切筛选时的竞态，慢的那次不许覆盖新的）。
  const seq = useRef(0);

  const load = useCallback(
    async (f: LogFilters, from: number | null) => {
      const mine = ++seq.current;
      setBusy(true);
      try {
        const r = await api.logs(f, from);
        if (mine !== seq.current) return;
        setRows((prev) => (from === null ? r.logs : [...prev, ...r.logs]));
        setCursor(r.next_cursor);
        setRate(r.rate);
        setError(null);
      } catch (e) {
        if (mine === seq.current) setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (mine === seq.current) setBusy(false);
      }
    },
    [],
  );

  // 每次进页面实读（不缓存）：筛选项候选 + body 开关状态 + 第一页。
  useEffect(() => {
    void api.filters().then(setCandidates).catch(() => setCandidates(null));
    void api
      .bodyRecording()
      .then((b) => {
        setBody(b);
        setRemaining(b.seconds_remaining);
      })
      .catch(() => setBody(null));
  }, []);

  useEffect(() => {
    void load(applied, null);
  }, [applied, load]);

  // 「1 小时后自动关」的倒计时：本地每秒减，到点回后端确认真关了。
  useEffect(() => {
    if (!body?.on) return;
    const id = window.setInterval(() => {
      setRemaining((r) => {
        if (r === null) return r;
        if (r <= 1) {
          void api
            .bodyRecording()
            .then((b) => {
              setBody(b);
              setRemaining(b.seconds_remaining);
            })
            .catch(() => undefined);
          return 0;
        }
        return r - 1;
      });
    }, 1000);
    return () => window.clearInterval(id);
  }, [body?.on]);

  async function toggleBody(next: boolean) {
    setError(null);
    if (!next) {
      const b = await api.setBodyRecording(false);
      setBody(b);
      setRemaining(b.seconds_remaining);
      return;
    }
    // 隐私确认：一次，明确说清存哪、存多久（spec §10 第 5 条）。
    const ok = window.confirm(
      '开启「记录完整 body」后：请求与响应体会以明文写进本机 SQLite 数据库，' +
        `随日志一起保留 ${body?.retention_days ?? 7} 天，之后自动清理。` +
        `开关在 ${Math.round((body?.window_seconds ?? 3600) / 60)} 分钟后自动关闭。\n\n确定开启吗？`,
    );
    if (!ok) return;
    const b = await api.setBodyRecording(true, true);
    setBody(b);
    setRemaining(b.seconds_remaining);
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    setApplied(filters);
  }

  const set = (k: keyof LogFilters) => (v: string) => setFilters((f) => ({ ...f, [k]: v }));

  return (
    <section className="page">
      <header className="page-head">
        <h2>日志与调试</h2>
        <label className="body-switch">
          <input
            type="checkbox"
            checked={body?.on ?? false}
            disabled={body === null}
            onChange={(e) => void toggleBody(e.target.checked)}
          />
          记录完整 body
          {body?.on && remaining !== null && (
            <span className="muted">（{Math.floor(remaining / 60)} 分 {remaining % 60} 秒后自动关）</span>
          )}
          {!body?.on && <span className="muted">（默认关，开启需一次隐私确认）</span>}
        </label>
      </header>

      {error && <p className="error">加载失败：{error}</p>}

      <form className="filters" onSubmit={submit}>
        <label>
          客户端
          <select value={filters.client} onChange={(e) => set('client')(e.target.value)}>
            <option value="">全部</option>
            {candidates?.clients.map((c) => (
              <option key={c} value={c}>{c}</option>
            ))}
          </select>
        </label>
        <label>
          上游
          <select value={filters.upstream} onChange={(e) => set('upstream')(e.target.value)}>
            <option value="">全部</option>
            {candidates?.upstreams.map((u) => (
              <option key={u} value={u}>{u}</option>
            ))}
          </select>
        </label>
        <label>
          虚拟模型
          <select value={filters.virtual_model} onChange={(e) => set('virtual_model')(e.target.value)}>
            <option value="">全部</option>
            {candidates?.virtual_models.map((m) => (
              <option key={m} value={m}>{m}</option>
            ))}
          </select>
        </label>
        <label>
          状态码
          <select value={filters.status} onChange={(e) => set('status')(e.target.value)}>
            <option value="">全部</option>
            {(candidates?.statuses ?? Object.keys(STATUS_LABEL)).map((s) => (
              <option key={s} value={s}>{STATUS_LABEL[s] ?? s}</option>
            ))}
          </select>
        </label>
        <label className="grow">
          搜索
          <input
            type="search"
            placeholder="模型名或请求 ID（不搜 body）"
            value={filters.q}
            onChange={(e) => set('q')(e.target.value)}
          />
        </label>
        <button type="submit" disabled={busy}>查询</button>
        <button
          type="button"
          onClick={() => {
            setFilters(EMPTY_FILTERS);
            setApplied(EMPTY_FILTERS);
          }}
        >
          重置
        </button>
        <label title="库内以美元为锚，界面人民币为主（票 33）">
          币种
          <select value={currency} onChange={(e) => setCurrency(e.target.value as 'cny' | 'usd')}>
            <option value="cny">人民币</option>
            <option value="usd">美元</option>
          </select>
        </label>
      </form>

      <table className="logs">
        <thead>
          <tr>
            <th>时间</th>
            <th>状态码</th>
            <th>虚拟模型</th>
            <th>客户端</th>
            <th>上游</th>
            <th>真实模型</th>
            <th>耗时</th>
            <th>首字延迟</th>
            <th>token</th>
            <th>成本</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => (
            <tr key={r.id} className={r.aborted || (r.status ?? 0) >= 400 ? 'bad' : undefined}>
              <td title={r.request_id ?? undefined}>{time(r.ts)}</td>
              <td>{r.status ?? '—'}</td>
              <td>{r.virtual_model ?? '—'}</td>
              <td>{r.client ?? '—'}</td>
              <td>{r.upstream ?? '—'}</td>
              <td>{r.real_model ?? '—'}</td>
              <td>{r.total_ms === null || r.total_ms < 0 ? '—' : `${r.total_ms} ms`}</td>
              <td>{r.first_byte_ms === null || r.first_byte_ms < 0 ? '—' : `${r.first_byte_ms} ms`}</td>
              <td>{tokens(r)}</td>
              <td title={r.cost_usd === null ? '当时没有价格，不计入总额' : `$${r.cost_usd}`}>
                {money(r.cost_usd, rate, currency)}
              </td>
            </tr>
          ))}
          {rows.length === 0 && !busy && (
            <tr>
              <td colSpan={10} className="muted">还没有请求记录。</td>
            </tr>
          )}
        </tbody>
      </table>

      {cursor !== null && (
        <button type="button" disabled={busy} onClick={() => void load(applied, cursor)}>
          {busy ? '加载中…' : '加载更多'}
        </button>
      )}
    </section>
  );
}
