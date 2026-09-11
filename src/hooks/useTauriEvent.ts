// Tauri 事件订阅的统一封装（ADR-0009 D7）
//
// ## 为什么需要它
//
// `listen()` 是**异步**的（返回 `Promise<UnlistenFn>`）。此前各面板写成：
//
// ```ts
// let unlisten: (() => void) | undefined;
// (async () => { unlisten = await listenXxx(() => { ... }); })();
// return () => { unlisten?.(); };   // ← cleanup 与 await 竞态
// ```
//
// 问题：React 19 StrictMode 在 dev 下会「挂载 → 卸载 → 再挂载」，若 `listen()` 尚未
// resolve 就执行了 cleanup，`unlisten` 仍为 `undefined`，于是**该订阅永远不会被解绑**
// → 事件重复回调、监听器泄漏。少数写法还用 `.then(fn => unlisten = fn)` 且无 `.catch`，
// 一旦 `listen()` reject 就产生**未处理的 Promise 拒绝**。
//
// `LogPanel.tsx` 是当时唯一写对的实现（用 `cancelled` 标志兜住竞态），本模块把它抽成
// 唯一实现，全仓共用 —— 后续新增面板不再有机会复制该缺陷。
//
// ## 两个原语（按事件是否携带载荷区分，签名各自精确）
//
// - `useTauriEvent`：事件**带载荷**（`listen` / `listenProgress`）→ 回调收到 payload。
// - `useRefreshOnEvent`：事件**无载荷**（`listenXxxChanged`，Rust 端 emit `()`）→ 只触发刷新。
//
// 之所以拆成两个而非一个宽松签名：这两类订阅函数的**参数类型本就不同**
// （`(p: T) => void` vs `() => void`），强行合并会让 TS 的参数逆变检查失败，
// 或被迫退化成 `any` 而失去类型保护。

import { useEffect, useRef, type DependencyList } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";

/** 带载荷订阅：如 `listen` / `listenProgress` */
export type PayloadSubscriber<T> = (
  onPayload: (payload: T) => void,
) => Promise<UnlistenFn>;

/** 无载荷订阅：如 `listenVersionChanged` / `listenMcpChanged`（Rust 端 emit `()`） */
export type VoidSubscriber = (onEvent: () => void) => Promise<UnlistenFn>;

/**
 * 内部通用实现：订阅 → 卸载时**必定**解绑。
 *
 * - **竞态安全**：若 `subscribe` 在组件卸载后才 resolve，会立即调用解绑函数，
 *   不留悬挂监听器（对应旧实现里 `unlisten` 仍为 `undefined` 的泄漏场景）。
 * - **不吞错误**：`subscribe` 的 rejection 经 `console.error` 记录，不会变成未处理的
 *   Promise 拒绝（对应旧实现里 `.then()` 无 `.catch` 的场景）。
 * - **不会反复重订阅**：回调存入 ref 并在每次渲染后刷新，故调用方可安全内联箭头函数；
 *   只有 `deps` 变化才真正重新订阅。
 */
function useSubscription(
  subscribe: (fire: () => void) => Promise<UnlistenFn>,
  onFire: () => void,
  deps: DependencyList,
): void {
  const onFireRef = useRef(onFire);
  const subscribeRef = useRef(subscribe);
  useEffect(() => {
    onFireRef.current = onFire;
    subscribeRef.current = subscribe;
  });

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    (async () => {
      try {
        const fn = await subscribeRef.current(() => onFireRef.current());
        if (disposed) {
          // 订阅在卸载之后才就绪：立刻解绑，避免监听器泄漏
          fn();
        } else {
          unlisten = fn;
        }
      } catch (error) {
        console.error("订阅 Tauri 事件失败", error);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
    // deps 由调用方给出（与 useEffect 同一约定）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

/**
 * 订阅**带载荷**的 Tauri 事件，组件卸载时必定解绑。
 *
 * ```ts
 * useTauriEvent(listenProgress, (p) => { if (p.channel === "npm") setProgress(p); }, []);
 * ```
 */
export function useTauriEvent<T>(
  subscribe: PayloadSubscriber<T>,
  onPayload: (payload: T) => void,
  deps: DependencyList = [],
): void {
  const onPayloadRef = useRef(onPayload);
  useEffect(() => {
    onPayloadRef.current = onPayload;
  });
  useSubscription(
    (fire) => subscribe((payload) => {
      // 载荷事件：先转交调用方回调，再触发 fire（fire 仅用于保持统一实现）
      onPayloadRef.current(payload);
      fire();
    }),
    () => {},
    deps,
  );
}

/**
 * 订阅**无载荷**的 Tauri 事件（`listenXxxChanged`），收到即执行一次回调。
 *
 * ```ts
 * useRefreshOnEvent(listenMcpChanged, () => refresh(), [refresh]);
 * ```
 */
export function useRefreshOnEvent(
  subscribe: VoidSubscriber,
  onEvent: () => void,
  deps: DependencyList = [],
): void {
  useSubscription((fire) => subscribe(fire), onEvent, deps);
}
