// 跨页共用的小件：错误模板四件套、二次确认弹窗、徽章、格式化。
//
// 错误模板逐字照 spec §10 错误表达：**发生了什么 + 可能原因 + 一个主按钮 + 复制诊断**，
// 原始 JSON 可展开。

import { useState, type ReactNode } from 'react';

import { api, ApiError, type ErrorShape } from './api';
import { setCurrency, useCurrency, type Currency } from './currency';

/** 后端的错误模板长这样；别的东西（字符串 / Error）也接得住。 */
function isShape(value: unknown): value is ErrorShape {
  return typeof value === 'object' && value !== null && 'what' in value && 'likelyCause' in value;
}

export type PageSlug = 'overview' | 'upstreams' | 'models' | 'clients' | 'logs' | 'usage' | 'settings';

export function ErrorBlock({
  error,
  onAction,
  compact = false,
}: {
  /** 后端给的错误模板、`ApiError`，或者任何被 catch 到的东西（统一渲染成模板）。 */
  error: unknown;
  onAction?: (kind: string, page?: string | null) => void;
  compact?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  if (!error) return null;
  const shape: ErrorShape =
    error instanceof ApiError
      ? error.shape
      : isShape(error)
        ? error
        : { what: String(error), likelyCause: '看下面的原始错误。', raw: null };
  const [busy, setBusy] = useState(false);

  const copyDiagnostics = async () => {
    setBusy(true);
    try {
      const { text } = await api.diagnostics({ error: shape.raw ?? shape.what });
      await navigator.clipboard?.writeText(text);
      setCopied(true);
    } catch {
      setCopied(false);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={compact ? 'error-block compact' : 'error-block'}>
      <div className="error-what">{shape.what}</div>
      <div className="error-cause">可能原因：{shape.likelyCause}</div>
      <div className="error-actions">
        {shape.action && onAction && (
          <button className="primary" onClick={() => onAction(shape.action!.kind, shape.action!.page)}>
            {shape.action.label}
          </button>
        )}
        <button onClick={copyDiagnostics} disabled={busy}>
          {copied ? '已复制诊断' : '复制诊断'}
        </button>
        <button className="link" onClick={() => setOpen(!open)}>
          {open ? '收起原始错误' : '展开原始错误'}
        </button>
      </div>
      {open && <pre className="error-raw">{shape.raw ?? '（没有更多原文）'}</pre>}
    </div>
  );
}

export type ConfirmRequest = {
  title: string;
  /** 主按钮的动作文案（「写入并备份」「还原到安装时」…）。 */
  action: string;
  body: ReactNode;
  danger?: boolean;
};

/** 二次确认弹窗：主按钮写明动作，**不提供「下次不再问」**（票 17 第 86 条）。 */
export function ConfirmDialog({
  request,
  onDone,
}: {
  request: ConfirmRequest | null;
  onDone: (ok: boolean) => void;
}) {
  if (!request) return null;
  return (
    <div className="modal-backdrop">
      <div className="modal">
        <h3>{request.title}</h3>
        <div className="modal-body">{request.body}</div>
        <div className="modal-actions">
          <button className="link" onClick={() => onDone(false)}>
            取消
          </button>
          <button className={request.danger ? 'primary danger' : 'primary'} onClick={() => onDone(true)}>
            {request.action}
          </button>
        </div>
      </div>
    </div>
  );
}

export function Pill({ tone, children }: { tone: 'ok' | 'warn' | 'bad' | 'muted'; children: ReactNode }) {
  return <span className={`pill ${tone}`}>{children}</span>;
}

/** 客户端四态的徽章颜色（逐字四态由后端给，这里只上色）。 */
export function stateTone(state: string): 'ok' | 'warn' | 'bad' | 'muted' {
  switch (state) {
    case 'effective':
      return 'ok';
    case 'mismatch':
      return 'warn';
    case 'official':
      return 'muted';
    default:
      return 'muted';
  }
}

export function PathList({ paths }: { paths: string[] }) {
  if (paths.length === 0) return <span className="muted">（还没有备份）</span>;
  return (
    <ul className="paths">
      {paths.map((p) => (
        <li key={p}>
          <code>{p}</code>
        </li>
      ))}
    </ul>
  );
}

/** Unix 秒 → 本地时间（`reuseAt` / 自检时间这些都用它）。 */
export function formatTime(secs: number | null | undefined): string {
  if (!secs) return '—';
  return new Date(secs * 1000).toLocaleString();
}

/**
 * 美元 → 当前币种（票 33 第 2 节：库内美元为锚、界面人民币为主、可切美元）。
 * 金额**两位小数**；不足一个最小单位显示 `<¥0.01` / `<$0.01`；没价显示「未知」（不是 0）。
 */
export function money(usd: number | null | undefined, rate: number | null, currency: 'CNY' | 'USD' = 'CNY'): string {
  if (usd === null || usd === undefined) return '未知';
  const [symbol, value] = currency === 'USD' ? ['$', usd] : ['¥', usd * (rate ?? 7.1)];
  if (value > 0 && value < 0.01) return `<${symbol}0.01`;
  return `${symbol}${value.toFixed(2)}`;
}

export function json(value: unknown): string {
  try {
    const text = JSON.stringify(value);
    return text === undefined ? String(value) : text;
  } catch {
    return String(value);
  }
}

/** 币种切换（全局显示态，票 33 第 2 节；用量 / 概览 / 日志三处跟随同一个选择）。 */
export function CurrencySwitch() {
  const currency = useCurrency();
  return (
    <label title="库内以美元为锚；人民币按设置页的汇率换算（不联网）">
      币种
      <select value={currency} onChange={(e) => setCurrency(e.target.value as Currency)}>
        <option value="CNY">人民币</option>
        <option value="USD">美元</option>
      </select>
    </label>
  );
}
