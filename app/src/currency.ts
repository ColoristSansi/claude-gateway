// 币种是**全局显示态**（票 33 第 2 节）：用量页可切美元，概览与日志跟随当前选择。
//
// 库内一律以美元为锚；这里只存"显示成哪种货币"，选择记在 localStorage 里（刷新还在）。
// 二十来行的外部 store：比把 currency 穿过七页的 props 便宜，也不必引状态库（前端零依赖）。

import { useSyncExternalStore } from 'react';

export type Currency = 'CNY' | 'USD';

const KEY = 'cg.currency';

function stored(): Currency {
  try {
    return window.localStorage.getItem(KEY) === 'USD' ? 'USD' : 'CNY';
  } catch {
    return 'CNY';
  }
}

let current: Currency = stored();
const listeners = new Set<() => void>();

export function getCurrency(): Currency {
  return current;
}

export function setCurrency(next: Currency) {
  current = next;
  try {
    window.localStorage.setItem(KEY, next);
  } catch {
    // 隐私模式下写不了：这一次会话里照样切得动。
  }
  for (const l of listeners) l();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 当前币种（任何页面里都能用；切了立刻重渲染）。 */
export function useCurrency(): Currency {
  return useSyncExternalStore(subscribe, getCurrency, getCurrency);
}
