// 布局蓝本 = maestro ui.rs：顶栏（状态 + 全局操作）+ 主区（任务卡/收件箱）
// + 右栏事件流；弹窗：新建任务 / 设置；详情页右侧滑出。

import { useStore } from "./state/store";
import StatusBar from "./components/StatusBar";
import TaskCard from "./components/TaskCard";
import EventFeed from "./components/EventFeed";
import NewTaskDialog from "./components/NewTaskDialog";
import SettingsDialog from "./components/SettingsDialog";
import InboxPanel from "./components/InboxPanel";
import TaskDetail from "./components/TaskDetail";

export default function App() {
  const { state } = useStore();

  return (
    <div className="app">
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
        <TaskDetail id={state.selectedTask} />
      ) : null)}
      {state.dialog === "new-task" && <NewTaskDialog />}
      {state.dialog === "settings" && <SettingsDialog />}
      {state.toast && (
        <div className={`toast ${state.toast.kind}`}>{state.toast.text}</div>
      )}
    </div>
  );
}
