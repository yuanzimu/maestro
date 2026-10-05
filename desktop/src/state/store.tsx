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
import type {
  DaemonStatus,
  Envelope,
  InboxItem,
  ServerStatus,
  TaskSummary,
  WorkerItem,
} from "../types";
import { applyEventToTask } from "./events";

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
}

type Action =
  | { type: "daemon"; status: DaemonStatus }
  | { type: "sync"; status: ServerStatus | null; tasks: TaskSummary[]; inbox: InboxItem[]; workers: WorkerItem[]; managed?: boolean }
  | { type: "event"; env: Envelope }
  | { type: "select"; id: string | null }
  | { type: "dialog"; dialog: AppState["dialog"] }
  | { type: "toast"; text: string; kind: "ok" | "err" }
  | { type: "clear-toast" }
  | { type: "clear-emergency" };

const initial: AppState = {
  daemon: {
    running: false,
    managed: false,
    version: "",
    pid: 0,
    uptime_secs: 0,
    event_seq: 0,
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
          ? { ...state.daemon, running: true, version: a.status.version, pid: a.status.pid, uptime_secs: a.status.uptime_secs, event_seq: a.status.event_seq, managed: a.managed ?? state.daemon.managed }
          : { ...state.daemon, running: false },
      };
    }

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
    const un1 = listen<Envelope>("maestro://event", (e) =>
      dispatch({ type: "event", env: e.payload })
    );
    // 兜底轮询（Rust 侧 5s 推 server_status + task_list 快照）
    const un2 = listen<{ status: ServerStatus | null; tasks: TaskSummary[]; inbox: InboxItem[]; workers: WorkerItem[]; managed?: boolean }>(
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
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
    };
  }, []);

  // toast 自动消失
  useEffect(() => {
    if (state.toast) {
      const t = setTimeout(() => dispatch({ type: "clear-toast" }), 2500);
      return () => clearTimeout(t);
    }
  }, [state.toast]);

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
