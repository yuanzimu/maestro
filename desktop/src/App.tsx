// 布局蓝本 = maestro ui.rs：顶栏（状态 + 全局操作）+ 主区（任务卡/收件箱）
// + 右栏事件流；弹窗：新建任务 / 设置；详情页右侧滑出。

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { useStore } from "./state/store";
import * as api from "./api";
import StatusBar from "./components/StatusBar";
import TaskCard from "./components/TaskCard";
import EventFeed from "./components/EventFeed";
import NewTaskDialog from "./components/NewTaskDialog";
import SettingsDialog from "./components/SettingsDialog";
import InboxPanel from "./components/InboxPanel";
import TaskDetail from "./components/TaskDetail";
import ErrorBoundary from "./components/ErrorBoundary";

export default function App() {
  const { state, dispatch } = useStore();
  // 托盘动作（C3）：窗口可能隐藏中 —— 动作到达时需最新 state，避免闭包旧值
  const stateRef = useRef(state);
  stateRef.current = state;

  useEffect(() => {
    const un = listen<{ action: string }>("maestro://tray", (e) => {
      const st = stateRef.current;
      switch (e.payload.action) {
        case "tray-new-task":
          dispatch({ type: "dialog", dialog: "new-task" });
          break;
        case "tray-inbox":
          dispatch({ type: "dialog", dialog: "inbox" });
          break;
        case "tray-settings":
          dispatch({ type: "dialog", dialog: "settings" });
          break;
        case "tray-emergency":
          // 急停/恢复二合一：菜单文本反映当前态，这里复用 StatusBar 同款确认流
          if (st.emergency) {
            if (!confirm("恢复全部被急停的任务？")) return;
            api.resumeAll("flush").then(
              () => dispatch({ type: "clear-emergency" }),
              (err: string) =>
                dispatch({ type: "toast", text: String(err), kind: "err" })
            );
          } else {
            if (!confirm("全局急停：冻结所有 AI worker 并保存现场？")) return;
            api.emergencyStop("tray").then(
              () => undefined,
              (err: string) =>
                dispatch({ type: "toast", text: String(err), kind: "err" })
            );
          }
          break;
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [dispatch]);

  return (
    <div className="app">
      <ErrorBoundary>
        <StatusBar />
      {state.emergency && (
        <div className="banner">
          ⛔ 全局急停中：{state.emergency.reason || "用户急停"} —— 现场已冻结保留。
          到 <b>收件箱</b> 或任务卡上逐个恢复，或在收件箱点「全部恢复」。
        </div>
      )}
      <main>
        <section>
          {state.dialog === "inbox" ? <InboxPanel /> : (
            <>
              {state.taskOrder.length === 0 && (
                <div className="empty">
                  {state.daemon.running
                    ? "还没有任务。点右上角「新建任务」派第一个活儿。"
                    : "引擎未连接 —— 请稍候（自动拉起中），或到「设置」检查 worker 配置。"}
                </div>
              )}
              {state.taskOrder.map((id) => (
                <TaskCard key={id} task={state.tasks[id]} />
              ))}
            </>
          )}
        </section>
        <aside className="events">
          <h3>实时动态</h3>
          <EventFeed />
        </aside>
      </main>

      {state.selectedTask && (state.tasks[state.selectedTask] ? (
        <ErrorBoundary>
          <TaskDetail id={state.selectedTask} />
        </ErrorBoundary>
      ) : null)}
      {state.dialog === "new-task" && <NewTaskDialog />}
      {state.dialog === "settings" && <SettingsDialog />}
      {state.toast && (
        <div className={`toast ${state.toast.kind}`}>{state.toast.text}</div>
      )}
      </ErrorBoundary>
    </div>
  );
}
