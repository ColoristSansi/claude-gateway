// 设置页（spec §10 第 7 条，票 59 本体）。
//
// 逐项（顺序即 spec 段内顺序）：端口、开机自启（开关本体归票 62）、托盘行为、日志保留期与
// body 开关（与日志页**同一个**）、数据库位置与打开目录、导出 / 导入（票 62 的后端）、
// models.dev 快照刷新、汇率手改、关于与版本（含回退指引）。
//
// **凭据不放这里**（票 17 第 66 条）：整页一个密钥输入框都没有——密钥只在上游页管。

import { useCallback, useEffect, useState } from 'react';

import {
  api,
  ApiError,
  type AboutView,
  type BodyRecording,
  type DistInfo,
  type ImportReport,
  type SettingsView,
  type UpdateStatus,
} from '../api';
import { ErrorBlock, formatTime } from '../components';
import { setCurrency } from '../currency';

/** 保存后的一句话回执（成功 / 失败都显示在这一行，不弹窗）。 */
function Notice({ text, bad }: { text: string | null; bad?: boolean }) {
  if (!text) return null;
  return <p className={bad ? 'error' : 'muted'}>{text}</p>;
}

export function SettingsPage() {
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [dist, setDist] = useState<DistInfo | null>(null);
  const [about, setAbout] = useState<AboutView | null>(null);
  const [body, setBody] = useState<BodyRecording | null>(null);
  const [error, setError] = useState<unknown>(null);

  // 可编辑的草稿值（保存前的本地态）。
  const [port, setPort] = useState('');
  const [retention, setRetention] = useState('');
  const [rate, setRate] = useState('');
  const [tray, setTray] = useState<'hide' | 'quit'>('hide');
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [noticeBad, setNoticeBad] = useState(false);

  // 导出 / 导入。
  const [exportPath, setExportPath] = useState('');
  const [exportMode, setExportMode] = useState<'plain' | 'password'>('plain');
  const [password, setPassword] = useState('');
  const [importPath, setImportPath] = useState('');
  const [importPassword, setImportPassword] = useState('');
  const [overwrite, setOverwrite] = useState(false);
  const [importReport, setImportReport] = useState<ImportReport | null>(null);

  // 检查更新（票 61）：设置页这一个按钮打的就是顶栏横幅用的那两个接口。
  const [update, setUpdate] = useState<UpdateStatus | null>(null);
  const [updateBusy, setUpdateBusy] = useState(false);
  const [updateNote, setUpdateNote] = useState<string | null>(null);
  const [updateConfirm, setUpdateConfirm] = useState(false);

  const apply = useCallback((s: SettingsView) => {
    setSettings(s);
    setBody(s.bodyRecording);
    setPort(String(s.port));
    setRetention(String(s.logRetentionDays));
    setRate(String(s.usdCnyRate));
    setTray(s.trayCloseAction);
    setExportPath((p) => p || `${s.dataDir}\\claude-gateway-export.json`);
  }, []);

  const load = useCallback(async () => {
    try {
      apply(await api.settings());
      setError(null);
    } catch (e) {
      setError(e);
    }
  }, [apply]);

  useEffect(() => {
    void load();
    void api.distInfo().then(setDist).catch(() => setDist(null));
    void api.about().then(setAbout).catch(() => setAbout(null));
    void api.updateStatus().then(setUpdate).catch(() => setUpdate(null));
  }, [load]);

  /** 检查更新（票 61）：查不动就把后端那句人话原样显示，不吞。 */
  async function checkUpdate() {
    setUpdateBusy(true);
    setUpdateNote(null);
    try {
      const s = await api.updateCheck();
      setUpdate(s);
      setUpdateNote(
        s.available
          ? `发现新版本 ${s.available.version}（当前 ${s.currentVersion}）。`
          : `已经是最新版本（当前 ${s.currentVersion}）。`,
      );
    } catch (e) {
      setUpdateNote(e instanceof Error ? e.message : String(e));
    } finally {
      setUpdateBusy(false);
    }
  }

  /** 下载 + 验签 + 静默安装：**立刻应答**，之后进程会被安装器接管并自动重启。 */
  async function installUpdate() {
    setUpdateConfirm(false);
    setUpdateBusy(true);
    setUpdateNote(null);
    try {
      setUpdate(await api.updateInstall());
      setUpdateNote('正在下载并静默安装，装完会自动重启（数据 / 端口 / 客户端配置都不动）。');
    } catch (e) {
      setUpdateNote(e instanceof Error ? e.message : String(e));
    } finally {
      setUpdateBusy(false);
    }
  }

  /** 一次保存：写回后把草稿值对齐后端真值（不乐观更新）。 */
  async function save(what: string, patch: Parameters<typeof api.saveSettings>[0]) {
    setBusy(what);
    setNotice(null);
    try {
      const s = await api.saveSettings(patch);
      apply(s);
      setNoticeBad(false);
      setNotice(
        what === 'port'
          ? `端口已改为 ${s.port}；已生效客户端的地址已同步，Desktop / Codex 要完全退出重开才读到新端口`
          : '已保存。',
      );
    } catch (e) {
      setNoticeBad(true);
      setNotice(e instanceof ApiError ? `${e.shape.what}：${e.shape.likelyCause}` : String(e));
    } finally {
      setBusy(null);
    }
  }

  /** 探测一个空闲端口预填（**不写入**：写入要用户点保存）。 */
  async function probePort() {
    setBusy('probe');
    setNotice(null);
    try {
      const p = await api.probePort();
      setPort(String(p.suggestedPort));
      setNoticeBad(!p.currentPortFree);
      setNotice(
        `当前端口 ${p.currentPort}${p.currentPortFree ? '空闲' : '（被占用）'}；已预填 ${p.suggestedPort}，点「保存」才写入`,
      );
    } catch (e) {
      setNoticeBad(true);
      setNotice(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function toggleAutostart(next: boolean) {
    setBusy('autostart');
    setNotice(null);
    try {
      const a = await api.setAutostart(next);
      setSettings((s) => (s ? { ...s, autostart: { enabled: a.enabled, valueName: a.valueName } } : s));
      setNoticeBad(false);
      setNotice(a.enabled ? '开机自启已开启（写 HKCU 的 Run 键）' : '开机自启已关闭（Run 键已删）');
    } catch (e) {
      setNoticeBad(true);
      setNotice(e instanceof ApiError ? e.shape.what : String(e));
    } finally {
      setBusy(null);
    }
  }

  /** body 开关：与日志页同一个端点、同一个窗口；开启要一次隐私确认。 */
  async function toggleBody(next: boolean) {
    setBusy('body');
    setNotice(null);
    try {
      if (next) {
        const ok = window.confirm(
          '开启「记录完整 body」后：请求与响应体会以明文写进本机 SQLite 数据库，' +
            `随日志一起保留 ${body?.retention_days ?? 7} 天，之后自动清理。` +
            `开关在 ${Math.round((body?.window_seconds ?? 3600) / 60)} 分钟后自动关闭。\n\n确定开启吗？`,
        );
        if (!ok) return;
      }
      const b = await api.setBodyRecording(next, next);
      setBody(b);
      setSettings((s) => (s ? { ...s, bodyRecording: b } : s));
      setNoticeBad(false);
      setNotice(next ? '已开启（1 小时后自动关）' : '已关闭');
    } catch (e) {
      setNoticeBad(true);
      setNotice(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function openDir(which: 'data' | 'logs') {
    setNotice(null);
    try {
      const r = await api.openDbDir(which);
      setNoticeBad(false);
      setNotice(`已打开 ${r.opened}`);
    } catch (e) {
      setNoticeBad(true);
      setNotice(e instanceof ApiError ? `${e.shape.what}：${e.shape.likelyCause}` : String(e));
    }
  }

  async function refreshSnapshot() {
    setBusy('catalog');
    setNotice(null);
    try {
      const r = await api.refreshCatalog();
      setNoticeBad(!r.refreshed);
      setNotice(r.message);
      await load();
    } catch (e) {
      setNoticeBad(true);
      setNotice(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function doExport() {
    setBusy('export');
    setNotice(null);
    try {
      const r = await api.exportConfig({
        path: exportPath,
        password: exportMode === 'password' ? password : null,
        includeSecrets: exportMode === 'password' && password.length > 0,
      });
      setNoticeBad(false);
      setNotice(
        `已导出 ${r.bytes} 字节到 ${r.path}` +
          (r.encrypted ? '（带口令加密，可跨机迁移密钥）' : '（不含明文密钥，导入后需重填）'),
      );
    } catch (e) {
      setNoticeBad(true);
      setNotice(e instanceof ApiError ? `${e.shape.what}：${e.shape.likelyCause}` : String(e));
    } finally {
      setBusy(null);
    }
  }

  async function doImport() {
    setBusy('import');
    setNotice(null);
    setImportReport(null);
    try {
      const r = await api.importConfig({
        path: importPath,
        password: importPassword || null,
        overwrite,
      });
      setImportReport(r);
      setNoticeBad(false);
      setNotice(`已导入（格式 v${r.version} / 应用 ${r.appVersion}）`);
      await load();
    } catch (e) {
      setNoticeBad(true);
      setNotice(e instanceof ApiError ? `${e.shape.what}：${e.shape.likelyCause}` : String(e));
    } finally {
      setBusy(null);
    }
  }

  if (error !== null && !settings) {
    return (
      <section className="page">
        <header className="page-head">
          <h2>设置</h2>
        </header>
        <ErrorBlock error={error} />
        <p className="muted">网关没在跑时设置读不出来；配置类页面可以在网关起来后再改。</p>
      </section>
    );
  }

  return (
    <section className="page">
      <header className="page-head">
        <h2>设置</h2>
        <span className="muted">凭据不在这里管（只在上游页）</span>
      </header>

      <Notice text={notice} bad={noticeBad} />

      {settings && (
        <div className="cards">
          <div className="card">
            <h3>端口</h3>
            <div className="row">
              <input
                type="number"
                min={1}
                max={65535}
                value={port}
                onChange={(e) => setPort(e.target.value)}
                style={{ width: 110 }}
              />
              <button type="button" onClick={() => void probePort()} disabled={busy !== null}>
                {busy === 'probe' ? '探测中…' : '探测空闲端口'}
              </button>
              <button
                type="button"
                className="primary"
                disabled={busy !== null || Number(port) === settings.port}
                onClick={() => void save('port', { port: Number(port) })}
              >
                换端口并同步客户端
              </button>
            </div>
            <p className="muted">
              不静默换端口：先探一个空闲的预填，确认后才写入；换完自动重写<b>已生效</b>客户端的地址，
              未生效的不动。Desktop 与 Codex 要完全退出重开才读到新端口。
            </p>
          </div>

          <div className="card">
            <h3>开机自启</h3>
            <p className="muted">
              默认关闭。开启后写 HKCU 的 Run 键（{settings.autostart ? settings.autostart.valueName : 'Claude Gateway'}），
              开机时直接进托盘。
            </p>
            {settings.autostart ? (
              <label className="inline">
                <input
                  type="checkbox"
                  checked={settings.autostart.enabled}
                  disabled={busy !== null}
                  onChange={(e) => void toggleAutostart(e.target.checked)}
                />
                {settings.autostart.enabled ? '开机自启：已开启（点一下关闭）' : '开机自启：已关闭（点一下开启）'}
              </label>
            ) : (
              <p className="muted">维护面没挂上（无头模式）：自启开关不可用。</p>
            )}
          </div>

          <div className="card">
            <h3>托盘行为</h3>
            <p className="muted">关窗口时做什么（托盘右键菜单本身固定：显示主窗口 / 1P↔3P 快切 / 网关状态 / 打开日志目录 / 退出）。</p>
            <label className="inline">
              <input
                type="radio"
                name="tray"
                checked={tray === 'hide'}
                onChange={() => setTray('hide')}
              />
              关窗隐藏到托盘（网关继续跑）
            </label>
            <label className="inline">
              <input type="radio" name="tray" checked={tray === 'quit'} onChange={() => setTray('quit')} />
              关窗退出应用（网关跟着停）
            </label>
            <div className="actions">
              <button
                type="button"
                className="primary"
                disabled={busy !== null || tray === settings.trayCloseAction}
                onClick={() => void save('tray', { trayCloseAction: tray })}
              >
                保存托盘行为
              </button>
            </div>
          </div>

          <div className="card">
            <h3>日志保留期与 body 开关</h3>
            <div className="row">
              <label>
                保留期（天）
                <input
                  type="number"
                  min={0}
                  max={3650}
                  value={retention}
                  onChange={(e) => setRetention(e.target.value)}
                  style={{ width: 90 }}
                />
              </label>
              <button
                type="button"
                className="primary"
                disabled={busy !== null || Number(retention) === settings.logRetentionDays}
                onClick={() => void save('retention', { logRetentionDays: Number(retention) })}
              >
                保存保留期
              </button>
            </div>
            <p className="muted">超过保留期的请求日志（含 body）自动清理；0 = 不自动清理。</p>
            <label className="inline">
              <input
                type="checkbox"
                checked={body?.on ?? false}
                disabled={body === null || busy !== null}
                onChange={(e) => void toggleBody(e.target.checked)}
              />
              记录完整 body（与日志页同一个开关）
            </label>
            <p className="muted">
              {body?.on
                ? `开着：${Math.floor((body.seconds_remaining ?? 0) / 60)} 分 ${(body.seconds_remaining ?? 0) % 60} 秒后自动关`
                : '默认关。开启需一次隐私确认：请求与响应体会明文存进本机 SQLite，随日志保留后自动清理。'}
            </p>
          </div>

          <div className="card">
            <h3>数据库位置</h3>
            <ul className="kv">
              <li>
                <span>数据目录</span>
                <b><code>{settings.dataDir}</code></b>
              </li>
              <li>
                <span>库文件</span>
                <b><code>{settings.dbPath}</code></b>
              </li>
            </ul>
            <div className="row">
              <button type="button" onClick={() => void openDir('data')}>打开目录</button>
              <button type="button" onClick={() => void openDir('logs')}>打开日志目录</button>
            </div>
            <p className="muted">只显示路径 + 打开，不提供数据目录搬家（票 18）。</p>
          </div>

          <div className="card">
            <h3>汇率（美元 → 人民币）</h3>
            <div className="row">
              <input
                type="number"
                step="0.01"
                min={0.01}
                value={rate}
                onChange={(e) => setRate(e.target.value)}
                style={{ width: 110 }}
              />
              <button
                type="button"
                className="primary"
                disabled={busy !== null || Number(rate) === settings.usdCnyRate}
                onClick={() => void save('rate', { usdCnyRate: Number(rate) })}
              >
                保存汇率
              </button>
              <button type="button" onClick={() => setCurrency('CNY')}>
                界面切回人民币
              </button>
            </div>
            <p className="muted">
              随包内置初值 + 这里手改，<b>不联网</b>。库内以美元为锚，改汇率会让历史人民币数字跟着变
              （改价不会：价格在请求时冻结）。
            </p>
          </div>

          <div className="card">
            <h3>models.dev 价格快照</h3>
            <p className="muted">
              上次刷新：{settings.catalogSnapshotAt ? formatTime(settings.catalogSnapshotAt) : '从没手动刷过（用随包内置那份）'}
            </p>
            <div className="row">
              <button type="button" className="primary" disabled={busy !== null} onClick={() => void refreshSnapshot()}>
                {busy === 'catalog' ? '刷新中…' : '刷新快照'}
              </button>
            </div>
            <p className="muted">
              手动拉一次 models.dev；<b>失败静默沿用旧快照</b>，不自动重试、不启动时自动拉。
              手工价锁定的条目不覆盖。
            </p>
          </div>

          <div className="card">
            <h3>导出 / 导入</h3>
            {dist && <p className="muted">{dist.dpapiWarning}</p>}
            <div className="row">
              <label className="grow">
                导出到
                <input type="text" value={exportPath} onChange={(e) => setExportPath(e.target.value)} />
              </label>
            </div>
            <label className="inline">
              <input
                type="radio"
                name="export"
                checked={exportMode === 'plain'}
                onChange={() => setExportMode('plain')}
              />
              不含密钥（默认，导入后需重填）
            </label>
            <label className="inline">
              <input
                type="radio"
                name="export"
                checked={exportMode === 'password'}
                onChange={() => setExportMode('password')}
              />
              带口令加密（可跨机迁移密钥）
            </label>
            {exportMode === 'password' && (
              <label className="inline">
                口令
                <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
              </label>
            )}
            <div className="actions">
              <button type="button" className="primary" disabled={busy !== null} onClick={() => void doExport()}>
                {busy === 'export' ? '导出中…' : '导出'}
              </button>
            </div>

            <div className="row">
              <label className="grow">
                从文件导入
                <input type="text" value={importPath} onChange={(e) => setImportPath(e.target.value)} />
              </label>
              <label className="inline">
                口令（加密导出才要）
                <input type="password" value={importPassword} onChange={(e) => setImportPassword(e.target.value)} />
              </label>
              <label className="inline">
                <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} />
                覆盖重复上游
              </label>
              <button type="button" disabled={busy !== null} onClick={() => void doImport()}>
                {busy === 'import' ? '导入中…' : '导入'}
              </button>
            </div>

            {importReport && (
              <ul className="kv">
                <li>
                  <span>版本</span>
                  <b>{importReport.format} v{importReport.version} · 应用 {importReport.appVersion} · {formatTime(Date.parse(importReport.exportedAt) / 1000)}</b>
                </li>
                <li>
                  <span>上游</span>
                  <b>新建 {importReport.upstreamsCreated} / 覆盖 {importReport.upstreamsReplaced} / 跳过 {importReport.upstreamsSkipped}</b>
                </li>
                <li>
                  <span>重复上游</span>
                  <b>{importReport.duplicates.length ? importReport.duplicates.join('、') : '（没有）'}</b>
                </li>
                <li>
                  <span>模型 / 绑定 / 设置</span>
                  <b>
                    {importReport.upstreamModelsImported} / {importReport.virtualModelsImported} / {importReport.settingsImported}
                  </b>
                </li>
                <li>
                  <span>密钥</span>
                  <b>
                    导入 {importReport.credentialsImported}，跳过 {importReport.credentialsSkipped}
                    {importReport.credentialsSkipped > 0 ? '（这些凭据没有跟着来，请重新填写）' : ''}
                  </b>
                </li>
              </ul>
            )}
            <p className="muted">
              导入的设置里含端口：跨机导入会把端口一起带过来，要重启网关才生效。
            </p>
          </div>

          <div className="card">
            <h3>关于与版本</h3>
            <ul className="kv">
              <li>
                <span>版本</span>
                <b>{settings.version}</b>
              </li>
              <li>
                <span>数据目录</span>
                <b><code>{about?.dataDir ?? settings.dataDir}</code></b>
              </li>
              <li>
                <span>更新包目录</span>
                <b><code>{about?.updatesDir ?? '（读不到）'}</code></b>
              </li>
            </ul>
            {about && (
              <>
                <p className="muted">{about.portable.note}</p>
                <p className="muted">{about.update.note}</p>
                <p>
                  <b>回到上一个版本：</b>
                  {about.rollback.guidance}
                </p>
                {about.packages.length > 0 ? (
                  <ul className="paths">
                    {about.packages.map((p) => (
                      <li key={p}><code>{p}</code></li>
                    ))}
                  </ul>
                ) : (
                  <p className="muted">更新包目录里还没有包（升级过一次之后才会有）。</p>
                )}
              </>
            )}
            <div className="actions">
              {update?.available ? (
                updateConfirm ? (
                  <>
                    <span className="muted">
                      确认后静默安装 {update.available.version} 并自动重启（数据、端口、客户端配置都不动）？
                    </span>
                    <button type="button" onClick={() => void installUpdate()} disabled={updateBusy}>
                      确认安装
                    </button>
                    <button type="button" onClick={() => setUpdateConfirm(false)}>取消</button>
                  </>
                ) : (
                  <button type="button" onClick={() => setUpdateConfirm(true)} disabled={updateBusy}>
                    下载并安装 {update.available.version}
                  </button>
                )
              ) : (
                <button type="button" onClick={() => void checkUpdate()} disabled={updateBusy}>
                  {updateBusy ? '检查中…' : '检查更新'}
                </button>
              )}
              {update?.installing ? (
                <span className="muted">
                  正在{update.totalBytes ? `下载 ${Math.round((update.downloadedBytes / update.totalBytes) * 100)}%` : '下载'}
                  {update.installingPackage ? ` · 正在静默安装 ${update.installingPackage}` : ''}
                  ，装完会自动重启。
                </span>
              ) : null}
              <Notice text={updateNote ?? update?.error ?? null} bad={Boolean(update?.error)} />
            </div>
          </div>
        </div>
      )}
    </section>
  );
}
