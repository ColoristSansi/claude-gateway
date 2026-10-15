// 管理面 `/api/*` 的薄封装（spec §10：前端只调 `/api/*`，不参与转发）。
//
// 字段名与后端 snake_case 逐字对齐（`crates/gateway-core/src/obs/mod.rs` 的 `row_json` /
// `usage_summary`）：这里不再改名一次，省得两端对不上。
//
// 地址：网关固定监听回环端口（默认 8787，见 `gateway_core::service::DEFAULT_PORT`）。
// shortcut: 开发态前端在 1420、网关在 8787，属跨源，还没配 CORS（归票 52 的壳层）；
// 打包后由外壳决定怎么把 /api 暴露给 webview，那时这里应该换成相对路径或外壳注入的基址。

export type LogRow = {
  id: number;
  ts: number;
  request_id: string | null;
  virtual_model: string | null;
  client: string | null;
  upstream: string | null;
  real_model: string | null;
  wire: string | null;
  status: number | null;
  first_byte_ms: number | null;
  total_ms: number | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_read: number | null;
  aborted: boolean;
  error: string | null;
  cost_usd: number | null;
  body: string | null;
};

export type LogsResponse = {
  logs: LogRow[];
  limit: number;
  next_cursor: number | null;
  /** 美元→人民币汇率（票 33：库内美元为锚，界面按当前汇率换算）。 */
  rate: number | null;
  retention_days: number;
};

export type Filters = {
  clients: string[];
  upstreams: string[];
  virtual_models: string[];
  statuses: string[];
};

export type BodyRecording = {
  on: boolean;
  seconds_remaining: number | null;
  window_seconds: number;
  retention_days: number;
};

export type LogFilters = {
  client: string;
  upstream: string;
  virtual_model: string;
  status: string;
  q: string;
};

export const EMPTY_FILTERS: LogFilters = {
  client: '',
  upstream: '',
  virtual_model: '',
  status: '',
  q: '',
};

const BASE = (import.meta.env.VITE_API_BASE as string | undefined) ?? 'http://127.0.0.1:8787';

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  const resp = await fetch(`${BASE}${path}`, init);
  const text = await resp.text();
  let body: unknown = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = null;
  }
  if (!resp.ok) {
    const message =
      (body as { error?: { message?: string } } | null)?.error?.message ??
      `${resp.status} ${resp.statusText}`;
    throw new Error(message);
  }
  return body as T;
}

function logQuery(f: LogFilters, cursor: number | null): string {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(f)) {
    if (v) p.set(k, v);
  }
  if (cursor !== null) p.set('cursor', String(cursor));
  const s = p.toString();
  return s ? `?${s}` : '';
}

/**
 * 自动升级（票 61）：`/api/update/*` 的响应体。
 *
 * 检查 / 下载 / 安装都是**外壳**（Tauri updater 插件）干的：前端只发三个请求 + 轮询 status。
 * 安装时进程会被安装器接管（装完自动重启），所以 install 是「立刻应答」，进度看 status。
 */
export type AvailableUpdate = {
  version: string;
  notes: string | null;
  pubDate: string | null;
  downloadUrl: string;
};

export type LocalPackage = {
  fileName: string;
  path: string;
  bytes: number;
  version: string | null;
};

export type UpdateRollback = {
  /** 回退指引原文（设置页「关于」/帮助里直接显示）。 */
  guide: string;
  installersDir: string;
  packages: LocalPackage[];
  previous: LocalPackage | null;
  lastUpdate: { from: string; to: string; at: string; package: string } | null;
};

export type UpdateStatus = {
  currentVersion: string;
  endpoints: string[];
  checking: boolean;
  installing: boolean;
  lastCheckedAt: string | null;
  available: AvailableUpdate | null;
  downloadedBytes: number;
  totalBytes: number | null;
  installingPackage: string | null;
  error: string | null;
  rollback: UpdateRollback;
};

export const api = {
  logs: (f: LogFilters, cursor: number | null = null) =>
    req<LogsResponse>(`/api/logs${logQuery(f, cursor)}`),
  filters: () => req<Filters>('/api/logs/filters'),
  bodyRecording: () => req<BodyRecording>('/api/logs/body-recording'),
  /** 开启必须带 `consent: true`（后端的隐私确认闸门）。 */
  setBodyRecording: (enabled: boolean, consent = false, seconds?: number) =>
    req<BodyRecording>('/api/logs/body-recording', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(seconds === undefined ? { enabled, consent } : { enabled, consent, seconds }),
    }),
  /** 升级：查一次（查不动会抛错，错误原文来自后端）。 */
  updateCheck: () => req<UpdateStatus>('/api/update/check', { method: 'POST' }),
  /** 升级：下载 + 验签 + 静默安装（立刻应答；装完由安装器把应用拉起来）。 */
  updateInstall: () => req<UpdateStatus>('/api/update/install', { method: 'POST' }),
  updateStatus: () => req<UpdateStatus>('/api/update/status'),
};
