// 模型与映射页（spec §10 第 3 节）：客户端标签（全局 / Desktop / Claude Code / Codex）
// + 四行家族槽位表（槽位 | 上游 | 真实模型 | 协议 | 测试）+ 普通虚拟模型列表
// + 每条一颗「价格」按钮（四项可填可留空）。
//
// 协议列可手改，但候选取值只限该模型 `supported_wires` 里声明过的值；未声明时可自由选，
// 界面标「未声明，靠自检」（票 23）。

import { useCallback, useEffect, useState } from 'react';

import { api, type BindingsView, type UpstreamModelView, type UpstreamView } from '../api';
import { ErrorBlock, json, Pill, type PageSlug } from '../components';
import type { ShellCtx } from '../App';

const SLOT_LABELS: Record<string, string> = {
  'claude-fable': 'claude-fable（最快档）',
  'claude-opus': 'claude-opus（最强档）',
  'claude-sonnet': 'claude-sonnet（日常档）',
  'claude-haiku': 'claude-haiku（最短档）',
};

type Draft = { upstreamId: string; realModel: string; wire: string };

export function ModelsPage({ ctx }: { ctx: ShellCtx }) {
  const [client, setClient] = useState('global');
  const [view, setView] = useState<BindingsView | null>(null);
  const [upstreams, setUpstreams] = useState<UpstreamView[]>([]);
  const [error, setError] = useState<unknown>(null);
  const [probe, setProbe] = useState<Record<string, string>>({});
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [models, setModels] = useState<Record<string, UpstreamModelView[]>>({});
  const [pricing, setPricing] = useState<{ upstream: string; model: string } | null>(null);
  const [newModel, setNewModel] = useState({ name: '', upstreamId: '', realModel: '', wire: 'anthropic' });
  /** 上一次没存下去的那一条（错误块的主按钮要能原样重试它）。 */
  const [failed, setFailed] = useState<{ slot: string; draft: Draft } | null>(null);

  const load = useCallback(async () => {
    try {
      const [b, u] = await Promise.all([api.bindings(client), api.upstreams()]);
      setView(b);
      setUpstreams(u.upstreams);
      const catalog: Record<string, UpstreamModelView[]> = {};
      for (const up of u.upstreams) {
        catalog[up.id] = (await api.upstreamModels(up.id)).models;
      }
      setModels(catalog);
      setFailed(null);
      setError(null);
    } catch (e) {
      setFailed(null);
      setError(e);
    }
  }, [client]);

  useEffect(() => {
    void load();
  }, [load]);

  const draftFor = (slot: string, fallback: { upstreamId: string; realModel: string; wire: string } | null): Draft =>
    drafts[slot] ?? fallback ?? { upstreamId: upstreams[0]?.id ?? '', realModel: '', wire: 'anthropic' };

  const save = async (slot: string, d: Draft) => {
    try {
      await api.saveBinding(slot, { upstreamId: d.upstreamId, realModel: d.realModel, wire: d.wire }, client);
      setDrafts((prev) => {
        const next = { ...prev };
        delete next[slot];
        return next;
      });
      await load();
      await ctx.refresh();
    } catch (e) {
      setFailed({ slot, draft: d });
      setError(e);
    }
  };

  const test = async (slot: string) => {
    setProbe((p) => ({ ...p, [slot]: '测试中…' }));
    try {
      const r = await api.probeSlot(slot, client);
      setProbe((p) => ({
        ...p,
        [slot]: `${r.ok ? '✓' : '✗'} ${r.status} · ${r.elapsedMs} ms${r.detail ? ' · ' + r.detail : ''}`,
      }));
    } catch (e) {
      setProbe((p) => ({ ...p, [slot]: `✗ ${e instanceof Error ? e.message : String(e)}` }));
    }
  };

  const addVirtual = async () => {
    try {
      await api.saveVirtualModel(newModel, client);
      setNewModel({ name: '', upstreamId: '', realModel: '', wire: 'anthropic' });
      await load();
    } catch (e) {
      setError(e);
    }
  };

  const removeVirtual = async (name: string) => {
    try {
      await api.deleteVirtualModel(name, client);
      await load();
    } catch (e) {
      setError(e);
    }
  };

  // 三条 wire 都实现了（票 65 接上 `openai-responses`）：下拉里没有禁选项，wire 名直接显示。
  const wireLabel = (wire: string) => wire;

  return (
    <section className="page">
      <div className="page-head">
        <h2>模型与映射</h2>
        <div className="tabs">
          {view?.profiles.map((p) => (
            <button
              key={p.id}
              className={p.id === client ? 'tab active' : 'tab'}
              onClick={() => {
                setClient(p.id);
                setDrafts({});
              }}
            >
              {p.label}
            </button>
          ))}
        </div>
      </div>
      <p className="muted">
        标签就是「每客户端可覆盖」的入口：切到某个客户端后改的绑定只影响它，没覆盖的落回「全局」。
      </p>

      <table className="table">
        <thead>
          <tr>
            <th>槽位</th>
            <th>上游</th>
            <th>真实模型</th>
            <th>协议</th>
            <th>测试</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {view?.slots.map((row) => {
            const d = draftFor(row.slot, row.binding);
            const catalog = models[d.upstreamId] ?? [];
            const entry = catalog.find((m) => m.modelId === d.realModel);
            // 这条模型声明过的 wire（空集合 = 未声明，照票 23 自由选）。
            const declared = entry && entry.supportedWires.length > 0 ? entry.supportedWires : [];
            // 受控 select 的候选里没有当前值时，浏览器会把**显示**回退到第一项，而 state 还是旧值：
            // 不在这儿把 draft 拨正，发出去的就是界面上没显示的那个 wire（票 60b：换模型后界面显示
            // openai-chat、实际发 anthropic → 400「协议不在这条模型的声明里」）。
            const draft: Draft = declared.length > 0 && !declared.includes(d.wire) ? { ...d, wire: declared[0] } : d;
            return (
              <tr key={row.slot}>
                <td>
                  <b>{SLOT_LABELS[row.slot] ?? row.slot}</b>
                  {row.overridden && <Pill tone="warn">已覆盖全局</Pill>}
                  {!row.binding && <div className="muted">未绑定</div>}
                </td>
                <td>
                  <select
                    value={d.upstreamId}
                    onChange={(e) => setDrafts({ ...drafts, [row.slot]: { ...d, upstreamId: e.target.value, realModel: '' } })}
                  >
                    <option value="">（选上游）</option>
                    {upstreams.map((up) => (
                      <option key={up.id} value={up.id}>
                        {up.name}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  <select
                    value={d.realModel}
                    onChange={(e) => setDrafts({ ...drafts, [row.slot]: { ...d, realModel: e.target.value } })}
                  >
                    <option value="">（从目录里选）</option>
                    {catalog.map((m) => (
                      <option key={m.modelId} value={m.modelId}>
                        {m.modelId}
                      </option>
                    ))}
                  </select>
                  {catalog.length === 0 && <div className="muted">这个上游还没拉过模型</div>}
                </td>
                <td>
                  <select
                    value={draft.wire}
                    onChange={(e) => setDrafts({ ...drafts, [row.slot]: { ...d, wire: e.target.value } })}
                  >
                    {(declared.length > 0 ? declared : view?.wires ?? []).map(
                      (wire) => (
                        <option key={wire} value={wire}>
                          {wireLabel(wire)}
                        </option>
                      ),
                    )}
                  </select>
                  {entry && entry.supportedWires.length === 0 && <Pill tone="warn">未声明，靠自检</Pill>}
                </td>
                <td>
                  <button onClick={() => test(row.slot)}>测试</button>
                  {probe[row.slot] && <div className="muted">{probe[row.slot]}</div>}
                </td>
                <td>
                  <button className="primary" onClick={() => save(row.slot, draft)} disabled={!draft.upstreamId || !draft.realModel}>
                    存
                  </button>
                  {row.binding && (
                    <button
                      className="link"
                      onClick={() => setPricing({ upstream: row.binding!.upstreamId, model: row.binding!.realModel })}
                    >
                      价格
                    </button>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      <h3>普通虚拟模型{client === 'codex' ? '（Codex 标签下这张列表就是它的模型目录）' : ''}</h3>
      <table className="table">
        <thead>
          <tr>
            <th>名称</th>
            <th>上游</th>
            <th>真实模型</th>
            <th>协议</th>
            <th>价格</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {view?.virtualModels.map((m) => (
            <tr key={m.name}>
              <td>{m.name}</td>
              <td>{m.upstreamId}</td>
              <td>{m.realModel}</td>
              <td>{m.wire}</td>
              <td>
                <button className="link" onClick={() => setPricing({ upstream: m.upstreamId, model: m.realModel })}>
                  价格
                </button>
              </td>
              <td>
                <button className="link" onClick={() => removeVirtual(m.name)}>
                  删
                </button>
              </td>
            </tr>
          ))}
          {view?.virtualModels.length === 0 && (
            <tr>
              <td colSpan={6} className="muted">
                这个标签下还没有普通虚拟模型。
              </td>
            </tr>
          )}
        </tbody>
      </table>

      <div className="row">
        <input placeholder="名称" value={newModel.name} onChange={(e) => setNewModel({ ...newModel, name: e.target.value })} />
        <select value={newModel.upstreamId} onChange={(e) => setNewModel({ ...newModel, upstreamId: e.target.value, realModel: '' })}>
          <option value="">（选上游）</option>
          {upstreams.map((up) => (
            <option key={up.id} value={up.id}>
              {up.name}
            </option>
          ))}
        </select>
        <select value={newModel.realModel} onChange={(e) => setNewModel({ ...newModel, realModel: e.target.value })}>
          <option value="">（选真实模型）</option>
          {(models[newModel.upstreamId] ?? []).map((m) => (
            <option key={m.modelId} value={m.modelId}>
              {m.modelId}
            </option>
          ))}
        </select>
        <select value={newModel.wire} onChange={(e) => setNewModel({ ...newModel, wire: e.target.value })}>
          {(view?.wires ?? []).map((wire) => (
            <option key={wire} value={wire}>
              {wireLabel(wire)}
            </option>
          ))}
        </select>
        <button onClick={addVirtual} disabled={!newModel.name || !newModel.upstreamId || !newModel.realModel}>
          加一条
        </button>
      </div>

      {pricing && <PriceDialog target={pricing} onClose={() => setPricing(null)} />}
      <ErrorBlock
        error={error}
        onAction={(kind, page) => {
          if (kind === 'goto' && page) ctx.goto(page as PageSlug);
        }}
        fallbackAction={
          failed
            ? { label: '重试存这一条', onClick: () => void save(failed.slot, failed.draft) }
            : { label: '重新读一次', onClick: () => void load() }
        }
      />
    </section>
  );
}

/** 四项价（输入 / 输出 / 缓存读 / 缓存写）**可填可留空**；填了即锁定并标出逐项来源。 */
function PriceDialog({
  target,
  onClose,
}: {
  target: { upstream: string; model: string };
  onClose: () => void;
}) {
  const [prices, setPrices] = useState<Record<string, string>>({});
  const [sources, setSources] = useState<Record<string, { usdPerMtok: number | null; source: string }>>({});
  const [error, setError] = useState<unknown>(null);

  useEffect(() => {
    void api
      .prices(target.upstream, target.model)
      .then((r) => setSources(r.prices))
      .catch((e) => setError(e));
  }, [target]);

  const save = async () => {
    try {
      const body: Record<string, number | null> = {};
      for (const key of ['input', 'output', 'cacheRead', 'cacheWrite']) {
        const raw = prices[key];
        body[key] = raw === undefined || raw.trim() === '' ? null : Number(raw);
      }
      await api.setPrices(target.upstream, target.model, body);
      setSources((await api.prices(target.upstream, target.model)).prices);
      onClose();
    } catch (e) {
      setError(e);
    }
  };

  const rows: [string, string][] = [
    ['input', '输入'],
    ['output', '输出'],
    ['cacheRead', '缓存读'],
    ['cacheWrite', '缓存写'],
  ];

  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h3>
          价格：{target.upstream} / {target.model}
        </h3>
        <div className="modal-body">
          <p className="muted">每百万 token 的美元数。四项可填可留空：填了就锁定（刷新快照不覆盖），留空回退 models.dev。</p>
          <table className="table">
            <thead>
              <tr>
                <th>项</th>
                <th>手工价</th>
                <th>现在的取值</th>
                <th>来源</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(([key, label]) => (
                <tr key={key}>
                  <td>{label}</td>
                  <td>
                    <input
                      value={prices[key] ?? ''}
                      onChange={(e) => setPrices({ ...prices, [key]: e.target.value })}
                      placeholder="留空"
                    />
                  </td>
                  <td>{sources[key]?.usdPerMtok ?? '未知'}</td>
                  <td>{sources[key]?.source ?? '—'}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <details>
            <summary>原始来源标记</summary>
            <pre className="error-raw">{json(sources)}</pre>
          </details>
          <div className="modal-actions">
            <button className="link" onClick={onClose}>
              取消
            </button>
            <button className="primary" onClick={save}>
              保存价格
            </button>
          </div>
        </div>
        <ErrorBlock error={error} />
      </div>
    </div>
  );
}
