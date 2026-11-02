// @ts-check
/** 平台共享 shim：react —— **显式命名导出**宿主应用实际在用的那一份（Vite 预打包 dep）。
 * 为什么显式：React 19 是 CJS-only，export * 经 Vite 转换后只剩 default，裸 ESM 消费者拿不到命名导出。
 * 为什么必须是宿主那一份：renderer（react-dom/client）就是它；hooks 的 dispatcher 由 renderer 写入。 */
import React from "react";
export default React;
export const {
  Children, Component, Fragment, Profiler, PureComponent, StrictMode, Suspense, cloneElement,
  createContext, createElement, createRef, forwardRef, isValidElement, lazy, memo, startTransition,
  use, useActionState, useCallback, useContext, useDebugValue, useDeferredValue, useEffect, useId,
  useImperativeHandle, useInsertionEffect, useLayoutEffect, useMemo, useOptimistic, useReducer,
  useRef, useState, useSyncExternalStore, useTransition, version,
} = React;
