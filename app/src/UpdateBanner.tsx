// 升级横幅（票 61）：应用内的「检查更新 → 用户确认 → 静默安装并自动重启」。
//
// 位置：`app/src/App.tsx` 顶部（票 52 的正式壳层会把它挪进顶栏；逻辑照搬即可）。
// 设置页（票 59）要的东西也在这一个响应里：`status.rollback.guide` 是回退指引原文，
// `status.rollback.installersDir` 是本地留包目录，`status.error` 是失败原因。
//
// 轮询：安装阶段 2 秒一次（看进度 / 看它什么时候把我们重启掉），平时 60 秒一次。

import { useCallback, useEffect, useState } from 'react';
import { api, type UpdateStatus } from './api';

export function UpdateBanner() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.updateStatus());
    } catch {
      // 网关没起来（或正在被安装器重启）时静默：这不是用户要处理的事。
    }
  }, []);

  useEffect(() => {
    void refresh();
    const ms = status?.installing ? 2000 : 60000;
    const timer = window.setInterval(() => void refresh(), ms);
    return () => window.clearInterval(timer);
  }, [refresh, status?.installing]);

  const check = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.updateCheck());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const install = async () => {
    setConfirming(false);
    setBusy(true);
    setError(null);
    try {
      // 立刻应答（真装起来时这个进程随后就被安装器替换掉了）。
      setStatus(await api.updateInstall());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const available = status?.available ?? null;
  const installing = status?.installing ?? false;
  const percent =
    status && status.totalBytes
      ? Math.min(100, Math.round((status.downloadedBytes / status.totalBytes) * 100))
      : null;

  // 没更新、没在装、没出错 → 不占地方（设置页里有「检查更新」按钮那一格）。
  if (!available && !installing && !error && !status?.error) return null;

  return (
    <div className="update-banner">
      {installing ? (
        <span>
          正在{percent === null ? '下载' : `下载 ${percent}%`}
          {status?.installingPackage ? ` · 正在静默安装 ${status.installingPackage}` : ''}
          ，装完会自动重启（数据 / 端口 / 客户端配置都不动）。
        </span>
      ) : available ? (
        <span>
          有新版本 <b>{available.version}</b>（当前 {status?.currentVersion}）
          {available.notes ? `：${available.notes}` : ''}
        </span>
      ) : (
        <span>更新检查：</span>
      )}

      {error || status?.error ? (
        <span className="update-error">{error ?? status?.error}</span>
      ) : null}

      {installing ? null : available ? (
        confirming ? (
          <span className="update-actions">
            确认后静默安装并自动重启（数据、端口、客户端配置都不动）？
            <button onClick={() => void install()} disabled={busy}>
              确认安装
            </button>
            <button onClick={() => setConfirming(false)}>取消</button>
          </span>
        ) : (
          <button onClick={() => setConfirming(true)} disabled={busy}>
            下载并安装 {available.version}
          </button>
        )
      ) : (
        <button onClick={() => void check()} disabled={busy}>
          {busy ? '检查中…' : '检查更新'}
        </button>
      )}
    </div>
  );
}
