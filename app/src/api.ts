// 管理面 `/api/*` 的薄封装（spec §10：前端只调 `/api/*`，不参与转发）。
//
// 字段名与后端 camelCase / snake_case 逐字对齐：票 53 的日志与用量面是 snake_case
// （`crates/gateway-core/src/obs/mod.rs`），票 52 的四页面是 camelCase
// （`crates/gateway-core/src/ingress/api/*`）。这里不两头改名，省得对不上。
//
// 地址：网关固定监听回环端口（默认 8787，见 `gateway_core::service::DEFAULT_PORT`）。
// 「换端口」成功后把新端口记进 localStorage，刷新后仍然找得到网关。

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
  /** 缓存写（票 58）：上游报了才有，没报是 null（不是 0）。 */
  cache_write: number | null;
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

// ───────────────────────── 四页要的形状（票 52） ─────────────────────────

/** 错误模板四件套（spec §10 错误表达）：发生了什么 + 可能原因 + 一个主按钮 + 复制诊断。 */
export type ErrorShape = {
  what: string;
  likelyCause: string;
  action?: { label: string; kind: string; page?: string | null } | null;
  raw?: string | null;
};

export class ApiError extends Error {
  status: number;
  shape: ErrorShape;

  constructor(status: number, shape: ErrorShape) {
    super(shape.what);
    this.status = status;
    this.shape = shape;
  }
}

export type GatewayStatus = {
  version: string;
  port: number;
  running: boolean;
  startedAt: string;
  uptimeSeconds: number;
  dbPath: string;
  dataDir: string | null;
};

export type Badges = {
  upstreamsFailedSelfcheck: number;
  upstreamsTotal: number;
  upstreamsWithPullWarning: number;
  clientsNotEffective: number;
  clientsNotEffectiveList: { id: string; state: string }[];
  ready: boolean;
};

export type RecentError = {
  at: number;
  requestId: string | null;
  client: string | null;
  virtualModel: string | null;
  upstream: string | null;
  realModel: string | null;
  wire: string | null;
  status: number;
  firstByteMs: number | null;
  totalMs: number | null;
  template: ErrorShape;
  raw: string;
};

export type ProviderPreset = {
  id: string;
  name: string;
  baseUrl: string;
  wire: string;
  paths: Record<string, string>;
  auth: Record<string, string>;
  quirks: string[];
  note: string;
  docUrl: string;
};

export type UpstreamView = {
  id: string;
  name: string;
  baseUrl: string;
  protocols: string[];
  paths: Record<string, string>;
  auth: Record<string, string>;
  quirks: string[];
  lastSelfcheck: { at: number; ok: boolean } | null;
  lastPull: { at: number; okAt: number | null; status: string; warning: string | null } | null;
  references: number;
  referenceList: string[];
  credentials: { wire: string; mask: string }[];
  modelCount: number;
  modelsUrl: string;
};

export type UpstreamModelView = {
  modelId: string;
  supportedWires: string[];
  source: string;
  note: string | null;
  prices: Record<string, { usdPerMtok: number | null; source: string }>;
};

export type PullReport = {
  disposition: 'fetched' | 'no-discovery' | 'failed' | 'no-credential' | 'credential-choice';
  message: string;
  credential: string | null;
  applied: { added: number; updated: number; removed: number; keptManual: number; undeclared: number };
  actions: { action: string; label: string }[];
  reuseAt: number | null;
  warnings: string[];
  chooseFrom: string[];
  models: UpstreamModelView[];
};

export type ProbeResult = {
  ok: boolean;
  status: number;
  elapsedMs: number;
  detail: string;
  upstream?: string;
  realModel?: string;
  wire?: string;
  requestId?: string;
};

export type BindingView = {
  profile: string;
  name: string;
  upstreamId: string;
  realModel: string;
  wire: string;
  isFamily: boolean;
  triple: string;
};

export type BindingsView = {
  profile: string;
  profileLabel: string;
  profiles: { id: string; label: string }[];
  slots: { slot: string; binding: BindingView | null; global: BindingView | null; overridden: boolean }[];
  virtualModels: BindingView[];
  wires: string[];
  /** 本版未实现的 wire（下拉里标出来并禁选；后端保存时也拒收）。 */
  unimplementedWires: string[];
};

export type UsageRow = {
  key: string | null;
  requests: number;
  cost_usd: number | null;
  input_tokens: number;
  output_tokens: number;
  cache_read: number;
  /** 缓存写（票 58）：上游报了才有，没报计 0。 */
  cache_write: number;
  /** 价格缺失的请求数（不计入总额）。 */
  no_price: number;
  /** token 未知的请求数（不计入总额）。 */
  token_unknown: number;
};

export type UsageTotals = Omit<UsageRow, 'key'>;

export type UsageSummary = {
  range: string;
  dim: string;
  from: number;
  totals: UsageTotals;
  rows: UsageRow[];
  /** 美元→人民币汇率（票 33：库内美元为锚，界面按当前汇率换算）。 */
  rate: number | null;
};

/** 按天明细（票 59）：`day` 是**本机本地**自然日 `YYYY-MM-DD`。 */
export type UsageDailyRow = UsageTotals & { day: string };

/** 用量页的完整一份（`/api/usage`，与 `/api/usage/summary` 同一个 handler）。 */
export type UsageReport = UsageSummary & {
  /** 右端（Unix 秒）；`null` = 一直到现在。 */
  to: number | null;
  daily: UsageDailyRow[];
};

/** 帧时间轴里的一帧（票 59）：时间戳 + 事件类型 + 摘要 + 可展开的原文。 */
export type LogFrame = {
  seq: number;
  at_ms: number;
  event: string | null;
  summary: string;
  raw: string | null;
};

/** 日志详情四段（票 59）：概要 / 请求与响应体 / 帧时间轴 / 错误信息。 */
export type LogDetail = {
  summary: LogRow;
  bodies: {
    request: string | null;
    response: string | null;
    recorded: boolean;
    recording_on: boolean;
    retention_days: number;
  };
  frames: {
    frames: LogFrame[];
    /** 上游原样的整条 SSE 文本（「切原始 SSE 文本」看的就是它）。 */
    sse: string | null;
    recorded: boolean;
  };
  error: { raw: string | null; template: ErrorShape | null };
};

export type SettingsView = {
  version: string;
  port: number;
  dbPath: string;
  dataDir: string;
  logRetentionDays: number;
  usdCnyRate: number;
  catalogSnapshotAt: number | null;
  /** 关窗行为：`hide` = 隐藏到托盘，`quit` = 退出应用。 */
  trayCloseAction: 'hide' | 'quit';
  bodyRecording: BodyRecording;
  /** 维护面没挂上时是 `null`（不假装「关着」）。 */
  autostart: { enabled: boolean; valueName: string } | null;
};

export type DistInfo = {
  exportFormat: string;
  exportVersion: number;
  dpapiWarning: string;
  usage: string;
  autostartDefault: boolean;
  restoreCommand: string;
};

export type ExportReport = {
  path: string;
  bytes: number;
  secrets: boolean;
  encrypted: boolean;
  dpapiWarning: string;
};

export type ImportReport = {
  format: string;
  version: number;
  appVersion: string;
  exportedAt: string;
  duplicates: string[];
  upstreamsCreated: number;
  upstreamsReplaced: number;
  upstreamsSkipped: number;
  upstreamModelsImported: number;
  virtualModelsImported: number;
  virtualModelsDropped: number;
  settingsImported: number;
  credentialsImported: number;
  credentialsSkipped: number;
};

export type AboutView = {
  version: string;
  dbPath: string;
  dataDir: string;
  updatesDir: string;
  packages: string[];
  rollback: { guidance: string; hasPrevious: boolean };
  portable: { isPortable: boolean; note: string };
  update: { supported: boolean; note: string };
};

export type CatalogRefresh = {
  refreshed: boolean;
  imported: number;
  at: number | null;
  reusedOldSnapshot: boolean;
  message: string;
};

export type ClientFileStatus = {
  path: string;
  exists: boolean;
  error: string | null;
  fields: { key: string; expected: unknown; current: unknown; matches: boolean }[];
  gatewayState: boolean;
  backupPreview: string;
  lastOp: string | null;
  firstBackup: string | null;
};

export type ClientCard = {
  id: string;
  display: string;
  state: 'not-configured' | 'effective' | 'mismatch' | 'official';
  stateLabel: string;
  installed: boolean;
  version: string | null;
  files: ClientFileStatus[];
  notes: string[];
  problem: string | null;
};

export type ClientsView = {
  clients: ClientCard[];
  gateway: { baseUrl: string; port: number; apiKeyMask: string };
  warnings: string[];
};

export type ActionReport = {
  client: string;
  action: string;
  message: string;
  files: { path: string; outcome: unknown; backup: string | null; notice: string | null }[];
};

export type AcceptanceResult = {
  ok: boolean;
  slot: string;
  upstream: string | null;
  realModel: string | null;
  wire: string | null;
  status: number | null;
  elapsedMs: number;
  firstByteMs: number | null;
  totalMs: number | null;
  requestId: string | null;
  logId: number | null;
  raw: string;
  template: ErrorShape | null;
};

const STORAGE_KEY = 'cg.apiBase';

function defaultBase(): string {
  const env = import.meta.env.VITE_API_BASE as string | undefined;
  if (env) return env;
  const saved = window.localStorage.getItem(STORAGE_KEY);
  if (saved) return saved;
  return 'http://127.0.0.1:8787';
}

export let API_BASE = defaultBase();

/** 换端口成功后调它：新端口记下来，刷新后前端还找得到网关。 */
export function setApiBase(port: number) {
  API_BASE = `http://127.0.0.1:${port}`;
  window.localStorage.setItem(STORAGE_KEY, API_BASE);
}

async function req<T>(path: string, init?: RequestInit): Promise<T> {
  let resp: Response;
  try {
    resp = await fetch(`${API_BASE}${path}`, {
      ...init,
      headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
    });
  } catch (e) {
    // 连不上 = 网关没在跑：这不是「接口报错」，是横幅要表达的那件事。
    throw new ApiError(0, {
      what: '连不上网关',
      likelyCause: `网关没在运行，或者不在 ${API_BASE}。`,
      action: { label: '启动网关', kind: 'start-gateway' },
      raw: String(e),
    });
  }
  const text = await resp.text();
  let body: unknown = null;
  try {
    body = text ? JSON.parse(text) : null;
  } catch {
    body = null;
  }
  if (!resp.ok) {
    const err = (body as { error?: Record<string, unknown> } | null)?.error ?? {};
    const shape: ErrorShape = {
      what: (err.what as string) ?? (err.message as string) ?? `${resp.status} ${resp.statusText}`,
      likelyCause: (err.likelyCause as string) ?? (err.type as string) ?? '看下面可展开的原始错误。',
      action: (err.action as ErrorShape['action']) ?? null,
      raw: (err.raw as string) ?? null,
    };
    throw new ApiError(resp.status, shape);
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

const json = (body: unknown): RequestInit => ({ method: 'POST', body: JSON.stringify(body ?? {}) });
const put = (body: unknown): RequestInit => ({ method: 'PUT', body: JSON.stringify(body ?? {}) });

/**
 * 自动升级（票 61）：`/api/update/*` 的响应体。
 *
 * 检查 / 下载 / 安装都由**外壳**（Tauri updater 插件）干：前端只发三个请求 + 轮询 status。
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
  // ── 票 53：日志与用量基础（合并时原样保留） ──
  logs: (f: LogFilters, cursor: number | null = null) =>
    req<LogsResponse>(`/api/logs${logQuery(f, cursor)}`),
  filters: () => req<Filters>('/api/logs/filters'),
  bodyRecording: () => req<BodyRecording>('/api/logs/body-recording'),
  /** 开启必须带 `consent: true`（后端的隐私确认闸门）。 */
  setBodyRecording: (enabled: boolean, consent = false, seconds?: number) =>
    req<BodyRecording>('/api/logs/body-recording', {
      ...json(seconds === undefined ? { enabled, consent } : { enabled, consent, seconds }),
    }),
  /** 票 59：详情四段（`id` 认行 id 也认 request_id）。 */
  logDetail: (id: number | string) => req<LogDetail>(`/api/logs/${encodeURIComponent(String(id))}`),
  /** 票 59：清空日志（二次确认由界面做）。 */
  clearLogs: () => req<{ deleted: number }>('/api/logs/clear', json({})),

  // ── 票 59：用量与成本页 ──
  /** 五个时间范围 × 三个维度 + 按天明细；自定义要配 `from` / `to`（本地日期，`to` 含当天）。 */
  usage: (
    range: 'today' | '7d' | '30d' | 'all' | 'custom',
    dim: 'virtual_model' | 'upstream' | 'client',
    from?: string,
    to?: string,
  ) => {
    const p = new URLSearchParams({ range, dim });
    if (from) p.set('from', from);
    if (to) p.set('to', to);
    return req<UsageReport>(`/api/usage?${p.toString()}`);
  },

  // ── 票 59：设置页 ──
  settings: () => req<SettingsView>('/api/settings'),
  saveSettings: (patch: Partial<{ port: number; logRetentionDays: number; usdCnyRate: number; trayCloseAction: 'hide' | 'quit' }>) =>
    req<SettingsView>('/api/settings', put(patch)),
  autostart: () => req<{ enabled: boolean; valueName: string; runKey: string; commandLine: string }>(
    '/api/settings/autostart',
  ),
  setAutostart: (enabled: boolean) =>
    req<{ enabled: boolean; valueName: string; runKey: string; commandLine: string }>(
      '/api/settings/autostart',
      put({ enabled }),
    ),
  openDbDir: (which: 'data' | 'logs') =>
    req<{ opened: string; which: string }>('/api/settings/db/open', json({ which })),
  refreshCatalog: () => req<CatalogRefresh>('/api/catalog/refresh', json({})),
  about: () => req<AboutView>('/api/about'),
  distInfo: () => req<DistInfo>('/api/dist/info'),
  /** 导出配置到文件（默认不含明文密钥；给口令 = 加密导出）。 */
  exportConfig: (body: { path: string; password?: string | null; includeSecrets?: boolean }) =>
    req<ExportReport>('/api/export', json(body)),
  importConfig: (body: { path: string; password?: string | null; overwrite?: boolean }) =>
    req<ImportReport>('/api/import', json(body)),

  // ── 通用 ──
  status: () => req<GatewayStatus>('/api/status'),
  badges: () => req<Badges>('/api/sidebar/badges'),
  recentErrors: (limit = 20) => req<{ errors: RecentError[] }>(`/api/errors/recent?limit=${limit}`),
  diagnostics: (payload: { error?: string; requestId?: string | number } = {}) =>
    req<{ text: string }>('/api/diagnostics/copy', json(payload)),
  /** 票 53 的统计层：字段是 snake_case，四页复用同一份实现（别自己再算一套）。 */
  usageSummary: (range: 'today' | '7d' | '30d' | 'all' = 'today', dim = 'virtual_model') =>
    req<UsageSummary>(`/api/usage/summary?range=${range}&dim=${dim}`),

  // ── 故障态：启动 / 换端口 ──
  gatewayStart: () => req<{ started: boolean; port: number }>('/api/gateway/start', json({})),
  probePort: () =>
    req<{ currentPort: number; currentPortFree: boolean; suggestedPort: number; note: string }>(
      '/api/gateway/port/probe',
      json({}),
    ),
  changePort: (port: number) =>
    req<{
      port: number;
      changed: boolean;
      previousPort: number;
      synced: { id: string; display: string }[];
      failed: { id: string; display: string; error: string }[];
      restart: string;
      message: string;
      restartHint: string;
    }>('/api/gateway/port', json({ port })),

  // ── 上游页 ──
  providers: () =>
    req<{
      presets: ProviderPreset[];
      snapshot: { id: string; modelCount: number }[];
      wires: string[];
      /** 本版未实现的 wire（向导里的下拉 / 徽章标出来，绑定时后端也会拒收）。 */
      unimplementedWires: string[];
    }>('/api/providers'),
  upstreams: () => req<{ upstreams: UpstreamView[] }>('/api/upstreams'),
  createUpstream: (body: Record<string, unknown>) => req<{ upstream: UpstreamView }>('/api/upstreams', json(body)),
  updateUpstream: (id: string, body: Record<string, unknown>) =>
    req<{ upstream: UpstreamView }>(`/api/upstreams/${encodeURIComponent(id)}`, put(body)),
  deleteUpstream: (id: string, force = false) =>
    req<{ deleted: boolean; bindingsRemoved: number }>(
      `/api/upstreams/${encodeURIComponent(id)}${force ? '?force=true' : ''}`,
      { method: 'DELETE' },
    ),
  upstreamReferences: (id: string) =>
    req<{ references: { client: string; model: string; label: string }[] }>(
      `/api/upstreams/${encodeURIComponent(id)}/references`,
    ),
  setCredential: (id: string, wire: string, key: string) =>
    req<{ saved: boolean; wire: string; mask: string | null }>(
      `/api/upstreams/${encodeURIComponent(id)}/credentials`,
      put({ wire, key }),
    ),
  discovery: (id: string, body: { wire?: string } = {}) =>
    req<PullReport>(`/api/upstreams/${encodeURIComponent(id)}/discovery`, json(body)),
  upstreamModels: (id: string) =>
    req<{ models: UpstreamModelView[] }>(`/api/upstreams/${encodeURIComponent(id)}/models`),
  addManualModel: (
    id: string,
    body: { modelId: string; wires: string[]; prices?: Record<string, number | null> },
  ) => req<{ saved: boolean; models: UpstreamModelView[] }>(`/api/upstreams/${encodeURIComponent(id)}/models`, json(body)),
  selftest: (id: string, body: { wire?: string; model?: string } = {}) =>
    req<ProbeResult>(`/api/upstreams/${encodeURIComponent(id)}/selftest`, json(body)),
  setPrices: (upstream: string, model: string, prices: Record<string, number | null>) =>
    req<{ saved: boolean; models: UpstreamModelView[] }>(
      `/api/upstreams/${encodeURIComponent(upstream)}/models/${encodeURIComponent(model)}/prices`,
      put(prices),
    ),

  // ── 模型与映射页 ──
  bindings: (client = 'global') => req<BindingsView>(`/api/bindings?client=${client}`),
  saveBinding: (slot: string, body: Record<string, unknown>, client = 'global') =>
    req<{ saved: boolean; binding: BindingView; wireNote: string | null }>(
      `/api/bindings/${encodeURIComponent(slot)}?client=${client}`,
      put(body),
    ),
  probeSlot: (slot: string, client = 'global') =>
    req<ProbeResult>(`/api/bindings/${encodeURIComponent(slot)}/probe?client=${client}`, json({})),
  virtualModels: (client = 'global') =>
    req<{ virtualModels: BindingView[] }>(`/api/virtual-models?client=${client}`),
  saveVirtualModel: (body: Record<string, unknown>, client = 'global') =>
    req<{ saved: boolean; binding: BindingView }>(`/api/virtual-models?client=${client}`, json(body)),
  deleteVirtualModel: (name: string, client = 'global') =>
    req<{ deleted: boolean }>(`/api/virtual-models/${encodeURIComponent(name)}?client=${client}`, {
      method: 'DELETE',
    }),
  prices: (upstream: string, model: string) =>
    req<{ prices: Record<string, { usdPerMtok: number | null; source: string }> }>(
      `/api/pricing/${encodeURIComponent(upstream)}/${encodeURIComponent(model)}`,
    ),

  // ── 客户端页 ──
  clients: () => req<ClientsView>('/api/clients'),
  applyClient: (id: string, claim = false) =>
    req<{ report: ActionReport; client: ClientCard; restartHint?: string }>(
      `/api/clients/${id}/apply`,
      json({ claim }),
    ),
  restoreClient: (id: string) =>
    req<{ report: ActionReport; client: ClientCard; restartHint?: string }>(
      `/api/clients/${id}/restore`,
      json({}),
    ),
  keepMine: (id: string) =>
    req<{ kept: string[]; message: string; client: ClientCard }>(`/api/clients/${id}/keep-mine`, json({})),
  claimOverwrite: (id: string) =>
    req<{ report: ActionReport; client: ClientCard }>(`/api/clients/${id}/claim-overwrite`, json({})),
  desktopMode: (mode: '1p' | '3p') =>
    req<{ report: ActionReport; client: ClientCard }>('/api/clients/desktop/mode', json({ mode })),
  switchAll: () =>
    req<{
      results: { client: string; display: string; skipped: boolean; message: string; error?: ErrorShape }[];
      skipped: string[];
      failed: string[];
      message: string;
    }>('/api/clients/switch-all', json({})),
  backups: (id: string) =>
    req<{ client: string; backups: string[]; backupRoot: string; restoreHint: string }>(
      `/api/clients/${id}/backups`,
    ),

  // ── 概览的端到端验收 ──
  acceptanceRun: () => req<AcceptanceResult>('/api/acceptance/run', json({})),

  // ── 自动升级（票 61）：检查 / 用户确认 / 静默安装 ──
  /** 查一次（查不动会抛错，错误原文来自后端）。 */
  updateCheck: () => req<UpdateStatus>('/api/update/check', { method: 'POST' }),
  /** 下载 + 验签 + 静默安装（**立刻应答**；装完由安装器把应用拉起来）。 */
  updateInstall: () => req<UpdateStatus>('/api/update/install', { method: 'POST' }),
  /** 轮询用：当前版本、进度、失败原因、回退指引与本地留包。 */
  updateStatus: () => req<UpdateStatus>('/api/update/status'),
};
