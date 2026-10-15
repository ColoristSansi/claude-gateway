// 日志与调试页（spec §10 第 5 条；列表那半是票 53 的，详情四段与清空是票 59 的）。
//
// 列表：元数据表格（时间倒序 50 条）+ 四个筛选 + 模型名 / 请求 ID 搜索 + 每行单次成本
// + 右上角「记录完整 body」开关（默认关、开启需一次隐私确认、1 小时自动关；与设置页同一个）。
// 详情抽屉四段（逐字）：**概要 / 请求与响应体 / 帧时间轴（可切原始 SSE 文本）/ 错误信息**。
// 清空日志走二次确认（票 17 第 86 条：删数据的动作都要确认，且不提供「下次不再问」）。

import { useCallback, useEffect, useRef, useState } from 'react';

import {
  api,
  EMPTY_FILTERS,
  type BodyRecording,
  type Filters,
  type LogDetail,
  type LogFilters,
  type LogRow,
} from '../api';
import { CurrencySwitch, formatTime, json, money, type ConfirmRequest } from '../components';
import { useCurrency } from '../currency';

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

function tokens(row: LogRow): string {
  if (
    row.input_tokens === null &&
    row.output_tokens === null &&
    row.cache_read === null &&
    row.cache_write === null
  ) {
    return 'token 未知';
  }
  const parts = [`入 ${row.input_tokens ?? '?'}`, `出 ${row.output_tokens ?? '?'}`];
  if (row.cache_read) parts.push(`缓存读 ${row.cache_read}`);
  if (row.cache_write) parts.push(`缓存写 ${row.cache_write}`);
  return parts.join(' · ');
}

export function LogsPage({ confirm }: { confirm: (req: ConfirmRequest) => Promise<boolean> }) {
  const currency = useCurrency();
  const [filters, setFilters] = useState<LogFilters>(EMPTY_FILTERS);
  const [applied, setApplied] = useState<LogFilters>(EMPTY_FILTERS);
  const [rows, setRows] = useState<LogRow[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const [rate, setRate] = useState<number | null>(null);
  const [candidates, setCandidates] = useState<Filters | null>(null);
  const [body, setBody] = useState<BodyRecording | null>(null);
  const [remaining, setRemaining] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [detail, setDetail] = useState<LogDetail | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);
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

  async function openDetail(row: LogRow) {
    setDetailError(null);
    setDetail(null);
    try {
      setDetail(await api.logDetail(row.id));
    } catch (e) {
      setDetailError(e instanceof Error ? e.message : String(e));
    }
  }

  async function clearAll() {
    const ok = await confirm({
      title: `清空全部请求日志？`,
      action: '清空日志',
      danger: true,
      body: (
        <div>
          <p>这会删掉库里<b>全部</b>请求日志（含已记录的 body 与帧时间轴），删了拿不回来。</p>
          <p className="muted">上游、绑定、客户端配置与手工价都不动。</p>
        </div>
      ),
    });
    if (!ok) return;
    try {
      const r = await api.clearLogs();
      setDetail(null);
      setRows([]);
      setCursor(null);
      setError(`已清空 ${r.deleted} 条请求日志`);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
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
        <button type="button" onClick={() => void clearAll()}>清空日志</button>
      </header>

      {error && <p className="error">{error}</p>}

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
        <CurrencySwitch />
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
            <th />
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
              <td>
                <button type="button" className="link" onClick={() => void openDetail(r)}>
                  详情
                </button>
              </td>
            </tr>
          ))}
          {rows.length === 0 && !busy && (
            <tr>
              <td colSpan={11} className="muted">还没有请求记录。</td>
            </tr>
          )}
        </tbody>
      </table>

      {cursor !== null && (
        <button type="button" disabled={busy} onClick={() => void load(applied, cursor)}>
          {busy ? '加载中…' : '加载更多'}
        </button>
      )}

      {(detail || detailError) && (
        <LogDrawer
          detail={detail}
          error={detailError}
          rate={rate}
          currency={currency}
          onClose={() => {
            setDetail(null);
            setDetailError(null);
          }}
        />
      )}
    </section>
  );
}

/** 右侧详情抽屉：四段逐字（概要 / 请求与响应体 / 帧时间轴 / 错误信息）。 */
function LogDrawer({
  detail,
  error,
  rate,
  currency,
  onClose,
}: {
  detail: LogDetail | null;
  error: string | null;
  rate: number | null;
  currency: 'CNY' | 'USD';
  onClose: () => void;
}) {
  // 第三段的两个视图：逐帧时间轴 / 原始 SSE 文本（票 59 的「可切」）。
  const [view, setView] = useState<'frames' | 'sse'>('frames');
  const [copied, setCopied] = useState(false);
  if (error) {
    return (
      <aside className="drawer">
        <header className="drawer-head">
          <h3>请求详情</h3>
          <button type="button" className="link" onClick={onClose}>关闭</button>
        </header>
        <p className="error">{error}</p>
      </aside>
    );
  }
  if (!detail) return null;
  const s = detail.summary;

  const copyDiagnostics = async () => {
    try {
      const { text } = await api.diagnostics({
        requestId: s.request_id ?? String(s.id),
        error: detail.error.raw ?? undefined,
      });
      await navigator.clipboard?.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  return (
    <aside className="drawer">
      <header className="drawer-head">
        <h3>请求详情</h3>
        <button type="button" onClick={() => void copyDiagnostics()}>
          {copied ? '已复制诊断' : '复制诊断'}
        </button>
        <button type="button" className="link" onClick={onClose}>关闭</button>
      </header>

      {/* ① 概要 */}
      <h4>概要</h4>
      <ul className="kv">
        <li><span>时间</span><b>{formatTime(s.ts)}</b></li>
        <li><span>请求 ID</span><b><code>{s.request_id ?? '—'}</code></b></li>
        <li><span>虚拟模型</span><b>{s.virtual_model ?? '—'}</b></li>
        <li><span>客户端</span><b>{s.client ?? '—'}</b></li>
        <li><span>上游 / 真实模型</span><b>{s.upstream ?? '—'} / {s.real_model ?? '—'}</b></li>
        <li><span>协议</span><b>{s.wire ?? '—'}</b></li>
        <li><span>状态码</span><b>{s.status ?? '—'}{s.aborted ? '（提前中断）' : ''}</b></li>
        <li><span>耗时 / 首字延迟</span><b>{s.total_ms ?? '—'} ms / {s.first_byte_ms ?? '—'} ms</b></li>
        <li><span>token</span><b>{tokens(s)}</b></li>
        <li><span>成本</span><b title={s.cost_usd === null ? '当时没有价格' : `$${s.cost_usd}`}>{money(s.cost_usd, rate, currency)}</b></li>
      </ul>

      {/* ② 请求与响应体（只在 body 记录开启时有） */}
      <h4>请求与响应体</h4>
      {detail.bodies.recorded ? (
        <>
          <p className="muted">明文存在本机 SQLite，随日志保留 {detail.bodies.retention_days} 天后清理。</p>
          <details open>
            <summary>发给上游的请求体</summary>
            <pre className="raw">{detail.bodies.request ?? '（这一发没有请求体）'}</pre>
          </details>
          <details>
            <summary>上游回来的原始响应（SSE 原文）</summary>
            <pre className="raw">{detail.bodies.response ?? '（这一发没有响应体）'}</pre>
          </details>
        </>
      ) : (
        <p className="muted">
          body 记录没开着，这一发只有元数据。要看请求 / 响应体，把右上角「记录完整 body」打开
          （需一次隐私确认，1 小时后自动关），再发一次请求。
        </p>
      )}

      {/* ③ 帧时间轴（可切原始 SSE 文本） */}
      <h4>帧时间轴</h4>
      {!detail.frames.recorded ? (
        <p className="muted">
          没有帧记录（与 body 同一个开关：响应内容也是内容）。开启后再发一次请求即可看到时间轴。
        </p>
      ) : (
        <>
          <div className="switch">
            <button
              type="button"
              className={view === 'frames' ? 'tab active' : 'tab'}
              onClick={() => setView('frames')}
            >
              逐帧（{detail.frames.frames.length} 帧）
            </button>
            <button
              type="button"
              className={view === 'sse' ? 'tab active' : 'tab'}
              onClick={() => setView('sse')}
            >
              原始 SSE 文本
            </button>
          </div>
          {view === 'frames' ? (
            <table className="table frames">
              <thead>
                <tr>
                  <th>#</th>
                  <th>时间（相对开始）</th>
                  <th>事件类型</th>
                  <th>摘要</th>
                </tr>
              </thead>
              <tbody>
                {detail.frames.frames.map((f) => (
                  <tr key={f.seq}>
                    <td>{f.seq}</td>
                    <td>{f.at_ms} ms</td>
                    <td><code>{f.event ?? '（无事件名）'}</code></td>
                    <td>
                      <details>
                        <summary>{f.summary || '（空帧）'}</summary>
                        <pre className="raw">{f.raw ?? '（没有原文）'}</pre>
                      </details>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <pre className="raw">{detail.frames.sse ?? '（没有原始文本）'}</pre>
          )}
        </>
      )}

      {/* ④ 错误信息 */}
      <h4>错误信息</h4>
      {detail.error.template ? (
        <>
          <div className="error-block compact">
            <div className="error-what">{detail.error.template.what}</div>
            <div className="error-cause">可能原因：{detail.error.template.likelyCause}</div>
          </div>
          <details>
            <summary>原始错误</summary>
            <pre className="raw">{detail.error.raw ?? '（没有原文）'}</pre>
          </details>
        </>
      ) : (
        <p className="muted">这一发没有错误。</p>
      )}
      <p className="muted">原始 JSON：{json(s)}</p>
    </aside>
  );
}
