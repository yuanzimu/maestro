// 新建任务：标题 / 提示词 / 工作目录（记住上次）

import { useEffect, useState } from "react";
import { useStore } from "../state/store";
import * as api from "../api";

const LS_WORKDIR = "maestro.last-workdir";

export default function NewTaskDialog() {
  const { dispatch } = useStore();
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState("");
  const [workdir, setWorkdir] = useState(() => localStorage.getItem(LS_WORKDIR) ?? "");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") dispatch({ type: "dialog", dialog: null });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dispatch]);

  const submit = async () => {
    if (!title.trim() || !prompt.trim() || busy) return;
    setBusy(true);
    setErr(null);
    try {
      const r = await api.createTask(title.trim(), prompt.trim(), workdir.trim() || undefined);
      localStorage.setItem(LS_WORKDIR, workdir.trim());
      dispatch({
        type: "toast",
        kind: "ok",
        text: r.queued
          ? `已入队（${r.reason === "slots_full" ? "并发已满，排队等待" : r.reason === "emergency_frozen" ? "急停中，排队等待" : "排队中"}）`
          : `已派活：${r.task.id}`,
      });
      dispatch({ type: "dialog", dialog: null });
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="overlay" onClick={() => dispatch({ type: "dialog", dialog: null })}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h3>新建任务</h3>
        <label>
          标题 <span className="req">*</span>
          <input
            autoFocus
            value={title}
            maxLength={200}
            placeholder="例：修登录页 bug"
            onChange={(e) => setTitle(e.target.value)}
          />
        </label>
        <label>
          提示词 <span className="req">*</span>
          <textarea
            rows={6}
            value={prompt}
            placeholder="要 AI 做什么，写清楚验收标准。演示模式里写「结束」可让 mock worker 提前完成。"
            onChange={(e) => setPrompt(e.target.value)}
          />
        </label>
        <label>
          工作目录
          <input
            value={workdir}
            placeholder="例：C:\\Users\\you\\project（须是 git 仓库，留空 = daemon 当前目录）"
            onChange={(e) => setWorkdir(e.target.value)}
          />
        </label>
        {err && <div className="form-err">{err}</div>}
        <div className="modal-actions">
          <button onClick={() => dispatch({ type: "dialog", dialog: null })}>取消</button>
          <button className="primary" disabled={!title.trim() || !prompt.trim() || busy} onClick={submit}>
            {busy ? "派发中…" : "派活 ☕"}
          </button>
        </div>
      </div>
    </div>
  );
}
