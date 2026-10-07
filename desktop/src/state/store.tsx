// 全局状态：Context + useReducer。
// 数据源三路合流（以 5s task_list 轮询为权威 reconcile，事件流做即时增量）：
//   1. maestro://event   —— Rust 事件桥（subscribe 全量重放 + 增量）
//   2. maestro://sync    —— Rust 5s 兜底轮询（server_status + task_list）
//   3. maestro://daemon  —— daemon 存活状态（事件桥探活）

import {
  createContext,
  useContext,
  useEffect,
  useReducer,
  type Dispatch,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  DaemonStatus,
  Envelope,
  InboxItem,
  ServerStatus,
  SavingsSummary,
  TaskSummary,
  WorkerItem,
} from "../types";
import { applyEventToTask } from "./events";
import * as api from "../api";

const MAX_EVENTS = 500;

export interface AppState {
  daemon: DaemonStatus;
  tasks: Record<string, TaskSummary>;
  taskOrder: string[]; // 最新创建在前
  events: Envelope[]; // 环形缓冲，最新在后
  inbox: InboxItem[];
  workers: WorkerItem[];
  emergency: { active: boolean; reason: string } | null;
  selectedTask: string | null;
  dialog: null | "new-task" | "settings" | "inbox";
  toast: { text: string; kind: "ok" | "err" } | null;
  /** C6 省 token 报告（低频轮询更新；null = 尚未拉到） */
  savings: SavingsSummary | null;
}

type Action =
  | { type: "daemon"; status: DaemonStatus }
  | { type: "sync"; status: ServerStatus | null; tasks: TaskSummary[]; inbox: InboxItem[]; workers: WorkerItem[]; managed?: boolean; engine_error?: string | null }
  | { type: "engine-error"; message: string }
  | { type: "event"; env: Envelope }
  | { type: "select"; id: string | null }
  | { type: "dialog"; dialog: AppState["dialog"] }
  | { type: "toast"; text: string; kind: "ok" | "err" }
  | { type: "clear-toast" }
  | { type: "clear-emergency" }
  | { type: "savings"; summary: SavingsSummary };

const initial: AppState = {
  daemon: {
    running: false,
    managed: false,
    version: "",
    pid: 0,
    uptime_secs: 0,
    event_seq: 0,
    error: null,
  },
  tasks: {},
  taskOrder: [],
  events: [],
  inbox: [],
  workers: [],
  emergency: null,
  selectedTask: null,
  dialog: null,
  toast: null,
  savings: null,
};

function upsertTask(state: AppState, id: string, patch: Partial<TaskSummary>) {
  const prev = state.tasks[id];
  const next: TaskSummary = { ...(prev ?? emptyTask(id)), ...patch, id };
  const order =
    prev == null ? [id, ...state.taskOrder] : state.taskOrder;
  return { ...state, tasks: { ...state.tasks, [id]: next }, taskOrder: order };
}

function emptyTask(id: string): TaskSummary {
  return {
    id,
    title: id,
    state: "queued",
    round: 0,
    narrative: "",
    blocked_kind: null,
    suspend_reason: null,
    acceptance_failures: 0,
  };
}

function reducer(state: AppState, a: Action): AppState {
  switch (a.type) {
    case "daemon":
      // 只更新 running：daemon 探活事件以 initial 为底构造，整体合并会把
      // sync 填入的 version/pid/uptime 用零值覆盖（顶栏版本号闪 "?" 最多 5s）
      return { ...state, daemon: { ...state.daemon, running: a.status.running } };

    case "sync": {
      // 权威 reconcile：以 RPC 快照覆盖本地推导状态。
      // 服务端 tasks 是 HashMap（顺序不稳定）→ 本地按状态等级 + id 排序，
      // 活跃在前、终态在后，同组内按 id（≈创建顺序）稳定排序
      const tasks: Record<string, TaskSummary> = {};
      for (const t of a.tasks) tasks[t.id] = t;
      const rank = (s: string) =>
        s === "working" || s === "planning" ? 0
        : s === "queued" ? 1
        : s === "blocked" || s === "suspended" ? 2
        : s === "done" ? 4
        : 3; // failed / cancelled
      const order = a.tasks
        .map((t) => t.id)
        .sort((x, y) => {
          const rx = rank(tasks[x]?.state ?? "queued");
          const ry = rank(tasks[y]?.state ?? "queued");
          return rx !== ry ? rx - ry : x.localeCompare(y);
        });
      return {
        ...state,
        tasks,
        taskOrder: order,
        inbox: a.inbox,
        workers: a.workers,
        daemon: a.status
          ? { ...state.daemon, running: true, version: a.status.version, pid: a.status.pid, uptime_secs: a.status.uptime_secs, event_seq: a.status.event_seq, managed: a.managed ?? state.daemon.managed, error: null }
          : { ...state.daemon, running: false, error: a.engine_error ?? state.daemon.error },
      };
    }

    case "engine-error":
      return {
        ...state,
        daemon: { ...state.daemon, running: false, error: a.message },
      };

    case "event": {
      const env = a.env;
      const events = [...state.events, env].slice(-MAX_EVENTS);
      let next = { ...state, events };

      // 全局急停横幅
      if (env.event.type === "emergency_stopped") {
        next = {
          ...next,
          emergency: { active: true, reason: String(env.event.reason ?? "") },
        };
      }
      // 急停解除（守护事件）：空冻结场景 resume_all 无任何 Resumed，
      // 只有本事件能清横幅；历史事件库里无此类型 → 对旧数据无害
      if (env.event.type === "emergency_resumed") {
        next = { ...next, emergency: null };
      }
      if (env.event.type === "resumed") {
        // 只认 resume_all（via=resume_all）：自动恢复/单任务恢复的
        // resumed 事件不得清全局急停横幅（否则无关任务的网络自动恢复
        // 会把横幅误清，用户以为急停已解除）
        if (state.emergency && env.event.via === "resume_all") {
          next = { ...next, emergency: null };
        }
      }

      const patch = applyEventToTask(env, undefined);
      const t = env.event.task;
      const taskId =
        typeof t === "string" ? t : t && typeof t === "object" && typeof (t as { id?: unknown }).id === "string" ? (t as { id: string }).id : null;
      if (patch && taskId) {
        next = upsertTask(next, taskId, patch);
      }
      return next;
    }

    case "select":
      return { ...state, selectedTask: a.id };
    case "dialog":
      return { ...state, dialog: a.dialog };
    case "toast":
      return { ...state, toast: { text: a.text, kind: a.kind } };
    case "clear-toast":
      return { ...state, toast: null };
    case "clear-emergency":
      return { ...state, emergency: null };
    case "savings":
      return { ...state, savings: a.summary };
    default:
      return state;
  }
}

interface StoreApi {
  state: AppState;
  dispatch: Dispatch<Action>;
}

const StoreCtx = createContext<StoreApi | null>(null);

export function StoreProvider({ children }: { children: ReactNode }) {
  const [state, dispatch] = useReducer(reducer, initial);

  useEffect(() => {
    // 事件桥
    const un1 = listen<Envelope>("maestro://event", (e) => {
      dispatch({ type: "event", env: e.payload });
      // C6 通知带节省数据：完成通知附「花费 X¢ 省 Y%」（任务级账本口径；
      // daemon 不在线等失败静默——通知是锦上添花不阻塞主流程）
      if (e.payload.event.type === "task_completed") {
        const id = e.payload.event.task as string;
        api
          .getLedger(id)
          .then((l) => {
            const pct =
              l.counterfactual_cost_cents > 0
                ? Math.round(
                    (l.saved_cents / l.counterfactual_cost_cents) * 100
                  )
                : null;
            dispatch({
              type: "toast",
              kind: "ok",
              text:
                pct !== null && pct > 0
                  ? `任务完成 · 花费 ${l.actual_cost_cents}¢ · 省 ${pct}%`
                  : `任务完成 · 花费 ${l.actual_cost_cents}¢`,
            });
          })
          .catch(() => {});
      }
    });
    // 兜底轮询（Rust 侧 5s 推 server_status + task_list 快照）
    const un2 = listen<{ status: ServerStatus | null; tasks: TaskSummary[]; inbox: InboxItem[]; workers: WorkerItem[]; managed?: boolean; engine_error?: string | null }>(
      "maestro://sync",
      (e) => dispatch({ type: "sync", ...e.payload })
    );
    // daemon 存活
    const un3 = listen<{ alive: boolean }>("maestro://daemon", (e) =>
      dispatch({
        type: "daemon",
        status: { ...initial.daemon, running: e.payload.alive },
      })
    );
    // 引擎拉起失败（restart 等监听已就位的场景即时呈现；setup 期首发
    // 事件早于本注册必丢 —— 兜底靠 sync 载荷的 engine_error 查询通道）
    const un4 = listen<{ message: string }>("maestro://daemon-error", (e) =>
      dispatch({ type: "engine-error", message: e.payload.message })
    );
    // All three listeners are now registered -> release the Rust bridge to do
    // its first subscribe. Without this, the daemon's instant replay burst on
    // the take-alive path is emitted before any listener exists and is lost.
    invoke("bridge_ready").catch(() => {});
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      un4.then((f) => f());
    };
  }, []);

  // toast 自动消失
  useEffect(() => {
    if (state.toast) {
      const t = setTimeout(() => dispatch({ type: "clear-toast" }), 2500);
      return () => clearTimeout(t);
    }
  }, [state.toast]);

  // C6 省 token 报告：mount 拉一次 + 60s 低频轮询（daemon 离线静默重试）
  useEffect(() => {
    const pull = () =>
      api.getSavingsSummary().then(
        (s) => dispatch({ type: "savings", summary: s }),
        () => {}
      );
    pull();
    const t = setInterval(pull, 60_000);
    return () => clearInterval(t);
  }, []);

  return (
    <StoreCtx.Provider value={{ state, dispatch }}>
      {children}
    </StoreCtx.Provider>
  );
}

export function useStore(): StoreApi {
  const ctx = useContext(StoreCtx);
  if (!ctx) throw new Error("useStore 必须在 StoreProvider 内使用");
  return ctx;
}
