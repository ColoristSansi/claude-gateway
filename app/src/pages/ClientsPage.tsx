// 客户端页（spec §10 第 4 节）：三张卡 + 四态徽章（未配置 / 已生效 / 值不匹配 / 已回官方）
// + 可折叠「当前值 vs 期望值」对照表 + 一键切换 / 一键还原 /（Desktop）1P↔3P。
//
// 状态**每次进页面实读配置文件比对**（后端每次真读，不缓存）。写文件与删数据一律二次确认，
// 弹窗里列出**绝对路径与备份路径**，主按钮写明动作，不提供「下次不再问」。
// 「未登录」不作阻断性错误（只在卡片上给一句说明）。

import { useCallback, useEffect, useState } from 'react';

import { api, type ClientCard, type ClientsView } from '../api';
import { ErrorBlock, PathList, Pill, stateTone } from '../components';
import type { ShellCtx } from '../App';

export function ClientsPage({ ctx }: { ctx: ShellCtx }) {
  const [view, setView] = useState<ClientsView | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await api.clients());
      setError(null);
    } catch (e) {
      setError(e);
    }
  }, []);

  useEffect(() => {
    void load(); // 每次进页面都实读
  }, [load]);

  const apply = async (card: ClientCard) => {
    setBusy(card.id);
    setError(null);
    setResult(null);
    try {
      const files = card.files.map((f) => f.path).join('；');
      const backups = card.files.map((f) => f.backupPreview).join('；');
      const ok = await ctx.confirm({
        title: `把 ${card.display} 切到网关？`,
        action: '写入并备份',
        body: (
          <div>
            <p>将改动的文件（绝对路径）：</p>
            <p>
              <code>{files || '（这个客户端没有配置文件）'}</code>
            </p>
            <p>备份路径：</p>
            <p>
              <code>{backups}</code>
            </p>
            <p className="muted">{card.notes[0]}</p>
          </div>
        ),
      });
      if (!ok) return;
      const res = await api.applyClient(card.id);
      setResult(`${card.display}：${res.report.message || '已写入'}${res.restartHint ? '；' + res.restartHint : ''}`);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const restore = async (card: ClientCard) => {
    setBusy(card.id);
    setError(null);
    setResult(null);
    try {
      const res = await api.backups(card.id);
      const ok = await ctx.confirm({
        title: `把 ${card.display} 还原到安装时？`,
        action: '还原到安装时',
        danger: true,
        body: (
          <div>
            <p>还原动作 = **摘掉网关接管键**（不是恢复某份备份）。将被改动的文件：</p>
            <PathList paths={card.files.map((f) => f.path)} />
            <p>现有备份（新到旧）：</p>
            <PathList paths={res.backups.slice(0, 8)} />
            <p className="muted">{res.restoreHint}</p>
          </div>
        ),
      });
      if (!ok) return;
      const out = await api.restoreClient(card.id);
      setResult(`${card.display}：${out.report.message || '已还原'}${out.restartHint ? '；' + out.restartHint : ''}`);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const keepMine = async (card: ClientCard) => {
    setBusy(card.id);
    try {
      const res = await api.keepMine(card.id);
      setResult(res.message);
      await load();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const claim = async (card: ClientCard) => {
    setBusy(card.id);
    try {
      const ok = await ctx.confirm({
        title: 'Codex 的模型目录不是网关写的那份，认领并覆盖？',
        action: '认领并覆盖',
        danger: true,
        body: (
          <div>
            <p>磁盘上的目录文件被外部改过。认领并覆盖会先备份，再只动网关的接管键。</p>
            <PathList paths={card.files.map((f) => f.path)} />
          </div>
        ),
      });
      if (!ok) return;
      const res = await api.claimOverwrite(card.id);
      setResult(res.report.message);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const desktopMode = async (mode: '1p' | '3p') => {
    setBusy('claude-desktop');
    try {
      const res = await api.desktopMode(mode);
      setResult(`Desktop 已切到 ${mode.toUpperCase()}：${res.report.message}`);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  const switchAll = async () => {
    setBusy('all');
    try {
      const ok = await ctx.confirm({
        title: '一键把可用客户端都切到网关？',
        action: '都切到网关',
        body: (
          <div>
            <p>已生效的会跳过；每个客户端的配置文件都会**先备份再改**。</p>
            <PathList paths={(view?.clients ?? []).flatMap((c) => c.files.map((f) => f.path))} />
          </div>
        ),
      });
      if (!ok) return;
      const res = await api.switchAll();
      setResult(res.message);
      await load();
      await ctx.refresh();
    } catch (e) {
      setError(e);
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className="page">
      <div className="page-head">
        <h2>客户端</h2>
        <div>
          <button onClick={() => void load()} disabled={busy !== null}>
            重新实读
          </button>{' '}
          <button className="primary" onClick={switchAll} disabled={busy !== null}>
            一键切换全部可用客户端
          </button>
        </div>
      </div>
      {view && (
        <p className="muted">
          网关地址 {view.gateway.baseUrl}（{view.gateway.apiKeyMask}）
          {view.warnings.map((w) => `　· ${w}`)}
        </p>
      )}
      {result && <p className="notice-line">{result}</p>}

      <div className="cards">
        {view?.clients.map((card) => (
          <div className="card client-card" key={card.id}>
            <div className="card-head">
              <h3>{card.display}</h3>
              <Pill tone={stateTone(card.state)}>{card.stateLabel}</Pill>
            </div>

            {!card.installed && <p className="muted">未检测到安装（配置目录还没建）。</p>}
            <ul className="kv">
              <li>
                <span>版本</span>
                <b>{card.version ?? '未检测到版本'}</b>
              </li>
              {card.files.map((f) => (
                <li key={f.path}>
                  <span>配置文件</span>
                  <b>
                    <code>{f.path}</code>
                    {!f.exists && <span className="muted">（不存在）</span>}
                  </b>
                </li>
              ))}
            </ul>

            {card.problem && <ErrorBlock error={{ what: card.problem, likelyCause: '文件被外部改成了非法 JSON / TOML。' }} />}

            <details>
              <summary>当前值 vs 期望值</summary>
              {card.files.map((f) => (
                <div key={f.path}>
                  <div className="muted">
                    <code>{f.path}</code>（备份预告：<code>{f.backupPreview}</code>）
                    {f.lastOp ? `　上次动作：${f.lastOp}` : ''}
                  </div>
                  <table className="table">
                    <thead>
                      <tr>
                        <th>接管键</th>
                        <th>当前值</th>
                        <th>期望值</th>
                      </tr>
                    </thead>
                    <tbody>
                      {f.fields.map((field) => (
                        <tr key={field.key} className={field.matches ? '' : 'mismatch'}>
                          <td>
                            <code>{field.key}</code>
                          </td>
                          <td>{short(field.current)}</td>
                          <td>{short(field.expected)}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ))}
            </details>

            <div className="actions">
              <button className="primary" onClick={() => apply(card)} disabled={busy !== null}>
                一键切换
              </button>
              <button onClick={() => restore(card)} disabled={busy !== null}>
                一键还原
              </button>
              {card.state === 'mismatch' && (
                <button onClick={() => keepMine(card)} disabled={busy !== null} title="把网关记录对齐磁盘现值，不写文件">
                  保留我的修改
                </button>
              )}
              {card.id === 'codex' && (
                <button onClick={() => claim(card)} disabled={busy !== null}>
                  认领并覆盖
                </button>
              )}
              {card.id === 'claude-desktop' && (
                <span className="switch">
                  <button onClick={() => desktopMode('1p')} disabled={busy !== null}>
                    切回 1P
                  </button>
                  <button onClick={() => desktopMode('3p')} disabled={busy !== null}>
                    切到 3P
                  </button>
                </span>
              )}
            </div>

            <ul className="notes muted">
              {card.notes.map((n) => (
                <li key={n}>{n}</li>
              ))}
            </ul>
          </div>
        ))}
      </div>

      <ErrorBlock error={error} />
    </section>
  );
}

function short(value: unknown): string {
  if (value === null || value === undefined) return '（缺席）';
  const text = typeof value === 'string' ? value : JSON.stringify(value);
  return text.length > 80 ? `${text.slice(0, 77)}…` : text;
}
