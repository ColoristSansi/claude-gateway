// 上游页（spec §10 第 2 节）：六列列表 + 四步向导（选供应商可跳过 → baseUrl 与 key →
// 拉模型可跳过、带 wire 徽章、手改锁定 → 强制自检并保存）。
//
// 凭据只在这里管；界面上只显示掩码（后端只回掩码，明文一个字节都不出库）。

import { useCallback, useEffect, useState } from 'react';

import { api, type ProviderPreset, type PullReport, type UpstreamModelView, type UpstreamView } from '../api';
import { ErrorBlock, formatTime, Pill } from '../components';
import type { ShellCtx } from '../App';

export function UpstreamsPage({ ctx }: { ctx: ShellCtx }) {
  const [list, setList] = useState<UpstreamView[]>([]);
  const [presets, setPresets] = useState<ProviderPreset[]>([]);
  /** 协议候选来自后端（`/api/providers`）：三条 wire 都可选（票 65 起没有未实现的面）。 */
  const [wires, setWires] = useState<string[]>([]);
  const [error, setError] = useState<unknown>(null);
  const [wizard, setWizard] = useState<Wizard | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [u, p] = await Promise.all([api.upstreams(), api.providers()]);
      setList(u.upstreams);
      setPresets(p.presets);
      setWires(p.wires);
      setError(null);
    } catch (e) {
      setError(e);
    }
  }, []);

  useEffect(() => {
    void load();
    if (ctx.hint === 'wizard') setWizard(newWizard());
  }, [load, ctx.hint]);

  const remove = async (up: UpstreamView) => {
    try {
      const refs = await api.upstreamReferences(up.id);
      const listText = refs.references.map((r) => r.label).join('；') || '（后端说没人引用）';
      const ok = await ctx.confirm({
        title: `删除上游 ${up.name}？`,
        action: '删除上游',
        danger: true,
        body: (
          <div>
            <p>它被这些地方引用：</p>
            <p>{listText}</p>
            <p className="muted">确认后连那些绑定一起删（槽位会变成未绑定）。</p>
          </div>
        ),
      });
      if (ok) await api.deleteUpstream(up.id, true);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    }
  };

  return (
    <section className="page">
      <div className="page-head">
        <h2>上游</h2>
        <button className="primary" onClick={() => setWizard(newWizard())}>
          加上游
        </button>
      </div>

      <table className="table">
        <thead>
          <tr>
            <th>名称</th>
            <th>baseUrl</th>
            <th>协议</th>
            <th>上次自检</th>
            <th>上次拉取</th>
            <th>被引用</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {list.map((up) => (
            <tr key={up.id}>
              <td>
                <b>{up.name}</b>
                <div className="muted">{up.id}</div>
              </td>
              <td>
                <code>{up.baseUrl}</code>
                <div className="muted">
                  {up.credentials.map((c) => `${c.wire}：${c.mask}`).join('　') || '（没有凭据）'}
                </div>
              </td>
              <td>{up.protocols.join(' / ') || '—'}</td>
              <td>
                {up.lastSelfcheck ? (
                  <Pill tone={up.lastSelfcheck.ok ? 'ok' : 'bad'}>
                    {up.lastSelfcheck.ok ? '通过' : '未通过'} · {formatTime(up.lastSelfcheck.at)}
                  </Pill>
                ) : (
                  <Pill tone="warn">没自检过</Pill>
                )}
              </td>
              <td>
                {up.lastPull ? (
                  <>
                    <span>{formatTime(up.lastPull.at)}</span>
                    {up.lastPull.warning && (
                      <span title={up.lastPull.warning}>
                        <Pill tone="warn">⚠</Pill>
                      </span>
                    )}
                    <div className="muted">{up.lastPull.status}</div>
                  </>
                ) : (
                  <span className="muted">从没拉过</span>
                )}
              </td>
              <td>
                <button className="link" onClick={() => setExpanded(expanded === up.id ? null : up.id)}>
                  {up.references} 处
                </button>
              </td>
              <td>
                <button className="link" onClick={() => remove(up)}>
                  删除
                </button>
              </td>
            </tr>
          ))}
          {list.length === 0 && (
            <tr>
              <td colSpan={7} className="muted">
                还没有上游。点右上「加上游」走四步向导（选供应商可以跳过）。
              </td>
            </tr>
          )}
        </tbody>
      </table>

      {expanded && (
        <div className="card">
          <h3>引用处</h3>
          <ReferenceList id={expanded} />
        </div>
      )}

      {wizard && (
        <Wizard
          presets={presets}
          wires={wires}
          onClose={() => setWizard(null)}
          onSaved={async () => {
            setWizard(null);
            await load();
            await ctx.refresh();
          }}
          confirm={ctx.confirm}
        />
      )}

      <ErrorBlock error={error} />
    </section>
  );
}

function ReferenceList({ id }: { id: string }) {
  const [refs, setRefs] = useState<{ client: string; model: string; label: string }[]>([]);
  useEffect(() => {
    void api
      .upstreamReferences(id)
      .then((r) => setRefs(r.references))
      .catch(() => setRefs([]));
  }, [id]);
  return refs.length === 0 ? (
    <p className="muted">没有槽位或虚拟模型引用它。</p>
  ) : (
    <ul>
      {refs.map((r) => (
        <li key={r.label}>
          {r.label}（客户端档案 {r.client}）
        </li>
      ))}
    </ul>
  );
}

type Wizard = {
  step: 1 | 2 | 3 | 4;
  presetId: string;
  id: string;
  name: string;
  baseUrl: string;
  paths: Record<string, string>;
  quirks: string[];
  key: string;
  secondWire: string;
  secondKey: string;
  showSecondKey: boolean;
  models: UpstreamModelView[];
  selected: Record<string, boolean>;
  manualId: string;
  pull: PullReport | null;
  selfcheck: { ok: boolean; status: number; elapsedMs: number; detail: string } | null;
  saved: boolean;
};

function newWizard(): Wizard {
  return {
    step: 1,
    presetId: '',
    id: '',
    name: '',
    baseUrl: '',
    paths: {},
    quirks: [],
    key: '',
    secondWire: 'openai-chat',
    secondKey: '',
    showSecondKey: false,
    models: [],
    selected: {},
    manualId: '',
    pull: null,
    selfcheck: null,
    saved: false,
  };
}

function Wizard({
  presets,
  wires,
  onClose,
  onSaved,
  confirm,
}: {
  presets: ProviderPreset[];
  /** 协议候选（`/api/providers` 的 `wires`）：三条都可选。 */
  wires: string[];
  onClose: () => void;
  onSaved: () => Promise<void>;
  confirm: ShellCtx['confirm'];
}) {
  const [w, setW] = useState<Wizard>(newWizard());
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const set = (patch: Partial<Wizard>) => setW((prev) => ({ ...prev, ...patch }));

  const pickPreset = (id: string) => {
    const p = presets.find((x) => x.id === id);
    if (!p) {
      set({ presetId: '', paths: {}, quirks: [] });
      return;
    }
    set({
      presetId: id,
      id: p.id,
      name: p.name,
      baseUrl: p.baseUrl,
      paths: p.paths,
      quirks: p.quirks,
    });
  };

  /** 第①、②步先把上游存下来（目录与凭据都有外键），第③、④步再往上加东西。 */
  const saveUpstream = async (): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      const body = {
        id: w.id || slug(w.baseUrl),
        name: w.name || w.id || slug(w.baseUrl),
        baseUrl: w.baseUrl,
        paths: w.paths,
        quirks: w.quirks,
        credentials: [
          ...(w.key ? [{ wire: 'default', key: w.key }] : []),
          ...(w.secondKey ? [{ wire: w.secondWire, key: w.secondKey }] : []),
        ],
      };
      await api.createUpstream(body).catch(async (e) => {
        // 已存在就走编辑（向导可以重进）。
        if ((e as { status?: number }).status === 409) await api.updateUpstream(body.id, body);
        else throw e;
      });
      set({ saved: true, id: body.id, name: body.name });
      return true;
    } catch (e) {
      setError(e);
      return false;
    } finally {
      setBusy(false);
    }
  };

  const doPull = async () => {
    setBusy(true);
    setError(null);
    try {
      const report = await api.discovery(w.id);
      set({ pull: report, models: report.models });
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  };

  const addManual = async () => {
    if (!w.manualId.trim()) return;
    setBusy(true);
    try {
      const res = await api.addManualModel(w.id, { modelId: w.manualId.trim(), wires: [] });
      set({ models: res.models, manualId: '' });
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  };

  const doSelfcheck = async () => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.selftest(w.id);
      set({ selfcheck: res, step: 4 });
    } catch (e) {
      setError(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop">
      <div className="modal wide">
        <h3>加上游（四步）</h3>
        <ol className="steps">
          <li className={w.step === 1 ? 'active' : ''}>① 选供应商（可跳过）</li>
          <li className={w.step === 2 ? 'active' : ''}>② baseUrl 与 key</li>
          <li className={w.step === 3 ? 'active' : ''}>③ 拉模型（可跳过）</li>
          <li className={w.step === 4 ? 'active' : ''}>④ 强制自检并保存</li>
        </ol>

        {w.step === 1 && (
          <div className="modal-body">
            <label>
              供应商预填（models.dev + 随包表；选完仍可改）
              <select value={w.presetId} onChange={(e) => pickPreset(e.target.value)}>
                <option value="">（跳过：手填 baseUrl）</option>
                {presets.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            </label>
            {w.presetId && (
              <p className="muted">
                {presets.find((p) => p.id === w.presetId)?.note}
                <br />
                协议倾向：{presets.find((p) => p.id === w.presetId)?.wire}
                {presets.find((p) => p.id === w.presetId)?.docUrl ? (
                  <>
                    {' '}
                    <a href={presets.find((p) => p.id === w.presetId)?.docUrl} target="_blank" rel="noreferrer">
                      官方文档
                    </a>
                  </>
                ) : null}
              </p>
            )}
            <div className="modal-actions">
              <button className="link" onClick={onClose}>
                取消
              </button>
              <button className="primary" onClick={() => set({ step: 2 })}>
                下一步
              </button>
            </div>
          </div>
        )}

        {w.step === 2 && (
          <div className="modal-body">
            <label>
              id（库内标识）
              <input value={w.id} onChange={(e) => set({ id: e.target.value })} placeholder="my-upstream" />
            </label>
            <label>
              名称
              <input value={w.name} onChange={(e) => set({ name: e.target.value })} placeholder="我的上游" />
            </label>
            <label>
              baseUrl（**不要带 /v1**）
              <input value={w.baseUrl} onChange={(e) => set({ baseUrl: e.target.value })} placeholder="https://api.example.com" />
            </label>
            <label>
              密钥（default；界面只回掩码）
              <input
                type="password"
                value={w.key}
                onChange={(e) => set({ key: e.target.value })}
                placeholder="sk-…"
              />
            </label>
            <label className="inline">
              <input
                type="checkbox"
                checked={w.showSecondKey}
                onChange={(e) => set({ showSecondKey: e.target.checked })}
              />
              这家上游有**第二种 wire 面**、凭据不一样（展开第二个 key 输入框）
            </label>
            {w.showSecondKey && (
              <div className="row">
                <select value={w.secondWire} onChange={(e) => set({ secondWire: e.target.value })}>
                  {wires.map((wire) => (
                    <option key={wire} value={wire}>
                      {wire}
                    </option>
                  ))}
                </select>
                <input
                  type="password"
                  value={w.secondKey}
                  onChange={(e) => set({ secondKey: e.target.value })}
                  placeholder={`${w.secondWire} 那份 key`}
                />
              </div>
            )}
            <p className="muted">允许先保存未验证：第④步才强制自检。</p>
            <div className="modal-actions">
              <button className="link" onClick={() => set({ step: 1 })}>
                上一步
              </button>
              <button
                className="primary"
                disabled={busy || !w.baseUrl.trim()}
                onClick={async () => {
                  if (await saveUpstream()) set({ step: 3 });
                }}
              >
                保存并继续
              </button>
            </div>
          </div>
        )}

        {w.step === 3 && (
          <div className="modal-body">
            <div className="row">
              <button onClick={doPull} disabled={busy}>
                {busy ? '拉取中…' : '拉 /models'}
              </button>
              <button className="link" onClick={() => set({ step: 4 })}>
                跳过（稍后继续）
              </button>
            </div>
            {w.pull && (
              <div className={`card ${w.pull.disposition === 'failed' ? 'bad-card' : ''}`}>
                <p>{w.pull.message}</p>
                {w.pull.reuseAt && <p className="muted">沿用上次结果（{formatTime(w.pull.reuseAt)}）</p>}
                {w.pull.credential && <p className="muted">{w.pull.credential}</p>}
                {w.pull.warnings.map((warning) => (
                  <p className="muted" key={warning}>
                    ⚠ {warning}
                  </p>
                ))}
                <div className="actions">
                  {w.pull.actions.map((a) => (
                    <button key={a.action} className="link" onClick={() => (a.action === 'Retry' ? doPull() : addManual())}>
                      {a.label}
                    </button>
                  ))}
                </div>
              </div>
            )}
            <table className="table">
              <thead>
                <tr>
                  <th>勾选</th>
                  <th>模型</th>
                  <th>可用 wire</th>
                  <th>来源</th>
                </tr>
              </thead>
              <tbody>
                {w.models.map((m) => (
                  <tr key={m.modelId}>
                    <td>
                      <input
                        type="checkbox"
                        checked={w.selected[m.modelId] ?? false}
                        onChange={(e) => set({ selected: { ...w.selected, [m.modelId]: e.target.checked } })}
                      />
                    </td>
                    <td>{m.modelId}</td>
                    <td>
                      {m.supportedWires.length === 0 ? (
                        <Pill tone="warn">未声明，靠自检</Pill>
                      ) : (
                        m.supportedWires.map((wire) => (
                          <span className="wire-badge" key={wire}>
                            {wire}
                          </span>
                        ))
                      )}
                    </td>
                    <td>
                      {m.source === 'manual' ? <Pill tone="muted">手改锁定</Pill> : m.source}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="row">
              <input
                value={w.manualId}
                onChange={(e) => set({ manualId: e.target.value })}
                placeholder="手填模型 ID（这本上游没有 /models 时用）"
              />
              <button onClick={addManual} disabled={busy || !w.manualId.trim()}>
                手工添加模型
              </button>
            </div>
            <div className="modal-actions">
              <button className="link" onClick={() => set({ step: 2 })}>
                上一步
              </button>
              <button className="primary" onClick={() => set({ step: 4 })}>
                下一步
              </button>
            </div>
          </div>
        )}

        {w.step === 4 && (
          <div className="modal-body">
            <p>第④步是**强制**的：发一条最短真实请求过认证与路径，结果落库（就绪判据只看它）。</p>
            <button className="primary" onClick={doSelfcheck} disabled={busy}>
              {busy ? '自检中…' : '自检并保存'}
            </button>
            {w.selfcheck && (
              <div className={w.selfcheck.ok ? 'card ok-card' : 'card bad-card'}>
                <ul className="kv">
                  <li>
                    <span>结果</span>
                    <b>{w.selfcheck.ok ? '通过' : '未通过'}</b>
                  </li>
                  <li>
                    <span>状态码</span>
                    <b>{w.selfcheck.status}</b>
                  </li>
                  <li>
                    <span>耗时</span>
                    <b>{w.selfcheck.elapsedMs} ms</b>
                  </li>
                  <li>
                    <span>原始提示</span>
                    <b>{w.selfcheck.detail}</b>
                  </li>
                </ul>
                {!w.selfcheck.ok && (
                  <ErrorBlock
                    error={{
                      what: '自检没通过',
                      likelyCause: '凭据、baseUrl 或路径模板不对；也可能这家上游要求另一种密钥',
                      action: { label: '回到第②步改凭据', kind: 'back' },
                      raw: w.selfcheck.detail,
                    }}
                    onAction={() => set({ step: 2 })}
                  />
                )}
              </div>
            )}
            <div className="modal-actions">
              <button className="link" onClick={() => set({ step: 3 })}>
                上一步
              </button>
              <button
                className="primary"
                disabled={busy}
                onClick={async () => {
                  if (w.selfcheck?.ok) {
                    await onSaved();
                    return;
                  }
                  const ok = await confirm({
                    title: '自检还没通过，仍要保存这条上游吗？',
                    action: '保存（稍后再自检）',
                    body: <p>上游会存着，但概览三步清单的第①步不会绿。</p>,
                  });
                  if (ok) await onSaved();
                }}
              >
                完成
              </button>
            </div>
          </div>
        )}

        <ErrorBlock error={error} />
      </div>
    </div>
  );
}

function slug(text: string): string {
  return text
    .replace(/^https?:\/\//, '')
    .replace(/[^a-zA-Z0-9]+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, 32)
    .toLowerCase();
}
