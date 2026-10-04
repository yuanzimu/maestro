//! `maestro ui`：本地 Web 指挥台（R54）。
//!
//! 零依赖 HTTP 服务器（std::net 手写 —— 二进制小、离线可用）+ 内嵌单页
//! 前端（inline CSS/JS，无构建工具、无 CDN）。CLI 入口：`maestro ui` →
//! 浏览器 http://localhost:7331。
//!
//! 可用性原则（「一看就懂」）：
//! - 任务卡片 = 状态色点 + 标题 + 一句话进度（narrative）+ 就地操作按钮
//! - 事件流右栏实时滚动；空态给下一步命令提示
//! - 2s 轮询（SSE 的简单替代 —— 无依赖、代理友好）
//!
//! 架构：事件缓冲线程（subscribe from 0 → VecDeque 滚动窗口）+
//! 每连接一线程（UI 连接数是个位数，足够）；API 经 Unix socket 代理 daemon。

use crate::MaestroClient;
use maestro_protocol::api::Method;
use maestro_protocol::events::Envelope;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// 事件缓冲上限（滚动；UI 只看「最近」）
const MAX_EVENTS: usize = 2000;

/// UI 服务器共享状态（listener 循环持有）
struct UiState {
    client: MaestroClient,
    events: Arc<Mutex<VecDeque<Envelope>>>,
}

/// 起服务并阻塞（CLI 入口）
pub fn serve_forever(client: MaestroClient, port: u16) -> std::io::Result<()> {
    let (listener, state) = bind(client, port)?;
    accept_loop(listener, state)
}

/// 起服务到后台线程，返回实际端口（测试/e2e 用）
pub fn spawn(client: MaestroClient) -> std::io::Result<u16> {
    let (listener, state) = bind(client, 0)?;
    let port = listener.local_addr()?.port();
    std::thread::spawn(move || {
        let _ = accept_loop(listener, state);
    });
    Ok(port)
}

fn bind(client: MaestroClient, port: u16) -> std::io::Result<(TcpListener, Arc<UiState>)> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    // 事件缓冲：订阅全量重放 + 增量（滚动窗口）。daemon 未启动/重启时
    // 1s 重连 —— UI 常驻，daemon 可后起
    let events: Arc<Mutex<VecDeque<Envelope>>> = Arc::new(Mutex::new(VecDeque::new()));
    let sub_client = client.clone();
    let ev_buf = events.clone();
    std::thread::spawn(move || loop {
        let buf = ev_buf.clone();
        // from_seq=1：触发 daemon 全量重放（hub 语义 from_seq>0 才重放）——
        // UI 无论何时连上（含 daemon 后起/重启）都能拿到完整历史 + 增量
        let _ = sub_client.subscribe(1, move |env| {
            if let Ok(mut q) = buf.lock() {
                q.push_back(env);
                while q.len() > MAX_EVENTS {
                    q.pop_front();
                }
            }
            true
        });
        std::thread::sleep(std::time::Duration::from_secs(1));
    });
    Ok((listener, Arc::new(UiState { client, events })))
}

fn accept_loop(listener: TcpListener, state: Arc<UiState>) -> std::io::Result<()> {
    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let st = state.clone();
                std::thread::spawn(move || {
                    let _ = handle_connection(s, &st);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

/// 处理单个 HTTP 连接（解析 → 路由 → 响应）
fn handle_connection(mut stream: TcpStream, state: &UiState) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    // 头部（读 Content-Length）
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let l = line.trim();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
            .map(str::trim)
        {
            content_length = v.parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let (code, ctype, resp_body) = route(&method, &path, &body, state);
    let status_text = match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        _ => "Internal Server Error",
    };
    let resp = format!(
        "HTTP/1.1 {code} {status_text}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        resp_body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.write_all(&resp_body)?;
    stream.flush()
}

/// 路由：返回 (状态码, Content-Type, body)
fn route(method: &str, path: &str, body: &[u8], state: &UiState) -> (u16, &'static str, Vec<u8>) {
    match (method, path) {
        ("GET", "/") => (200, "text/html; charset=utf-8", INDEX_HTML.as_bytes().to_vec()),
        ("GET", "/api/status") => api_json(state, Method::ServerStatus, serde_json::json!({})),
        ("GET", "/api/tasks") => api_json(state, Method::TaskList, serde_json::json!({})),
        ("GET", "/api/inbox") => api_json(state, Method::InboxList, serde_json::json!({})),
        ("GET", p) if p.starts_with("/api/task") => {
            if let Some(id) = query_param(p, "id") {
                api_json(state, Method::TaskGet, serde_json::json!({ "task": id }))
            } else {
                (400, "application/json", b"{\"error\":\"missing ?id=\"}".to_vec())
            }
        }
        ("GET", p) if p.starts_with("/api/events") => {
            let from = query_param(p, "from")
                .and_then(|f| f.parse::<u64>().ok())
                .unwrap_or(0);
            let q = state.events.lock().unwrap();
            let items: Vec<serde_json::Value> = q
                .iter()
                .filter(|e| e.seq > from)
                .map(|e| {
                    serde_json::json!({
                        "seq": e.seq,
                        "ts": e.ts,
                        "event": e.event,
                    })
                })
                .collect();
            let body = serde_json::to_vec(&serde_json::json!({ "events": items })).unwrap();
            (200, "application/json", body)
        }
        ("POST", "/api/steer") => {
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
                return (400, "application/json", b"{\"error\":\"bad json\"}".to_vec());
            };
            let (Some(task), Some(message)) = (
                v["task"].as_str().map(String::from),
                v["message"].as_str().map(String::from),
            ) else {
                return (
                    400,
                    "application/json",
                    b"{\"error\":\"need task+message\"}".to_vec(),
                );
            };
            api_json(
                state,
                Method::TaskSteer,
                serde_json::json!({ "task": task, "message": message }),
            )
        }
        ("POST", "/api/task/pause") => post_task_action(state, body, Method::TaskPause),
        ("POST", "/api/task/resume") => post_task_action(state, body, Method::TaskResume),
        ("POST", "/api/task/cancel") => post_task_action(state, body, Method::TaskCancel),
        ("GET", _) => (404, "text/plain; charset=utf-8", b"not found".to_vec()),
        _ => (405, "text/plain; charset=utf-8", b"method not allowed".to_vec()),
    }
}

/// POST {task: "..."} → daemon 方法
fn post_task_action(state: &UiState, body: &[u8], method: Method) -> (u16, &'static str, Vec<u8>) {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (400, "application/json", b"{\"error\":\"bad json\"}".to_vec());
    };
    let Some(task) = v["task"].as_str() else {
        return (400, "application/json", b"{\"error\":\"need task\"}".to_vec());
    };
    api_json(state, method, serde_json::json!({ "task": task }))
}

/// 代理到 daemon；失败 → 502 + JSON 错误（UI 显示「daemon 未连接」而非白屏）
fn api_json(
    state: &UiState,
    method: Method,
    params: serde_json::Value,
) -> (u16, &'static str, Vec<u8>) {
    match state.client.call("ui", method, params) {
        Ok(v) => (
            200,
            "application/json",
            serde_json::to_vec(&v).unwrap_or_default(),
        ),
        Err(e) => (
            502,
            "application/json",
            serde_json::to_vec(&serde_json::json!({ "error": e.to_string() }))
                .unwrap_or_default(),
        ),
    }
}

/// /path?k=v 解析（最小实现：值不转义，id/seq 场景够用）
fn query_param(path: &str, key: &str) -> Option<String> {
    let q = path.split('?').nth(1)?;
    for pair in q.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some(key) {
            return kv.next().map(String::from);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 内嵌单页前端（「一看就懂」：任务卡 + 一句话进度 + 就地操作 + 事件流）
// ---------------------------------------------------------------------------

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Maestro 指挥台</title>
<style>
  :root { --bg:#0f1117; --card:#1a1d27; --line:#2a2e3a; --fg:#e6e8ee; --dim:#8b90a0;
          --green:#3fb96b; --blue:#4a9eff; --yellow:#e8b93e; --red:#ef5b5b; --gray:#6b7280; }
  * { box-sizing:border-box; margin:0; }
  body { background:var(--bg); color:var(--fg); font:14px/1.6 -apple-system,"PingFang SC","Microsoft YaHei",sans-serif; }
  header { display:flex; align-items:center; gap:12px; padding:14px 22px; border-bottom:1px solid var(--line); position:sticky; top:0; background:var(--bg); z-index:5; }
  .logo { font-size:17px; font-weight:700; }
  .dot { width:10px; height:10px; border-radius:50%; background:var(--gray); display:inline-block; }
  .dot.on { background:var(--green); box-shadow:0 0 6px var(--green); }
  .counts { margin-left:auto; color:var(--dim); font-size:13px; }
  .counts b { color:var(--fg); }
  main { display:grid; grid-template-columns:minmax(0,1fr) 340px; gap:18px; padding:18px 22px; max-width:1280px; margin:0 auto; }
  @media (max-width:900px){ main { grid-template-columns:1fr; } }
  .hint { color:var(--dim); font-size:13px; }
  .hint code { background:var(--card); padding:1px 6px; border-radius:4px; font-size:12px; }
  .card { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:14px 16px; margin-bottom:12px; }
  .card .row1 { display:flex; align-items:center; gap:9px; }
  .st { width:10px; height:10px; border-radius:50%; flex:none; }
  .st.done{background:var(--green);} .st.working{background:var(--blue);animation:pulse 1.5s infinite;}
  .st.queued{background:var(--gray);} .st.blocked,.st.suspended{background:var(--yellow);}
  .st.failed,.st.cancelled{background:var(--red);}
  @keyframes pulse{50%{opacity:.35;}}
  .title { font-weight:600; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .tid { color:var(--dim); font-size:12px; margin-left:auto; flex:none; }
  .narrative { color:var(--dim); font-size:13px; margin:7px 0 10px; }
  .btns { display:flex; gap:8px; flex-wrap:wrap; align-items:center; }
  button { background:#242938; color:var(--fg); border:1px solid var(--line); border-radius:6px;
           padding:5px 12px; font-size:13px; cursor:pointer; }
  button:hover{ border-color:var(--blue); }
  button.warn:hover{ border-color:var(--red); }
  input.steer { background:var(--bg); border:1px solid var(--line); color:var(--fg); border-radius:6px;
                padding:5px 10px; font-size:13px; width:260px; }
  .events { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:12px 14px; align-self:start; }
  .events h3 { font-size:13px; color:var(--dim); margin-bottom:8px; font-weight:600; }
  .ev { font-size:12.5px; padding:4px 0; border-bottom:1px dashed var(--line); display:flex; gap:8px; }
  .ev:last-child{border:none;}
  .ev .t { color:var(--dim); flex:none; font-variant-numeric:tabular-nums; }
  .ev .taskid { color:var(--dim); font-size:11px; margin-left:auto; flex:none; }
  .empty { color:var(--dim); text-align:center; padding:48px 0; line-height:2; }
</style>
</head>
<body>
<header>
  <span class="dot" id="ddot"></span>
  <span class="logo">Maestro 指挥台</span>
  <span class="hint">派活给 AI · 看进度 · 给指示</span>
  <span class="counts" id="counts"></span>
</header>
<main>
  <section>
    <div id="tasks"></div>
    <div class="empty" id="empty" hidden>
      还没有任务。<br>创建第一个（终端里）：<br>
      <span style="font-size:12.5px"><code>maestro task create --title "修登录 bug" --prompt "定位并修复" --workdir ~/myproject</code></span>
    </div>
  </section>
  <aside class="events">
    <h3>实时动态</h3>
    <div id="events"></div>
  </aside>
</main>
<script>
"use strict";
let lastSeq = 0;
const $ = id => document.getElementById(id);
const ST = {done:"完成",working:"进行中",queued:"排队中",blocked:"待处理",
  failed:"失败",cancelled:"已取消",suspended:"已挂起"};
const stName = s => ST[s] || s;

async function fetchJson(url, opts){
  const r = await fetch(url, opts);
  return r.json().catch(() => ({}));
}

async function refreshTasks(){
  const v = await fetchJson("/api/tasks");
  const tasks = (v && v.tasks) || null;
  $("ddot").classList.toggle("on", !!tasks);
  if(!tasks){ $("counts").textContent = "daemon 未连接 —— 先启动 maestro-daemon"; return; }
  const n = {}; tasks.forEach(t => n[t.state] = (n[t.state]||0)+1);
  $("counts").textContent =
    `进行中 ${n.working||0} · 排队 ${n.queued||0} · 待处理 ${n.blocked||0} · 完成 ${n.done||0} · 失败 ${(n.failed||0)+(n.cancelled||0)}`;
  $("empty").hidden = tasks.length > 0;
  const box = $("tasks"); box.textContent = "";
  for(const t of tasks){ box.appendChild(card(t)); }
}

function card(t){
  const c = document.createElement("div"); c.className = "card";
  const row = document.createElement("div"); row.className = "row1";
  const st = document.createElement("span"); st.className = "st " + t.state; st.title = stName(t.state);
  const title = document.createElement("span"); title.className = "title"; title.textContent = t.title;
  const tid = document.createElement("span"); tid.className = "tid";
  tid.textContent = t.id + " · 第 " + (t.round||0) + " 轮";
  row.append(st, title, tid); c.appendChild(row);
  if(t.narrative){
    const p = document.createElement("div"); p.className = "narrative"; p.textContent = t.narrative;
    c.appendChild(p);
  }
  const btns = document.createElement("div"); btns.className = "btns";
  if(t.state === "working"){
    const input = document.createElement("input"); input.className = "steer"; input.maxLength = 500;
    input.placeholder = "给 AI 补充指示（下一轮生效）…";
    const send = document.createElement("button"); send.textContent = "发送";
    send.onclick = async () => {
      if(!input.value.trim()) return;
      send.disabled = true; send.textContent = "…";
      await fetchJson("/api/steer", {method:"POST", body: JSON.stringify({task: t.id, message: input.value})});
      send.textContent = "已发送 ✓";
      setTimeout(() => { send.textContent = "发送"; input.value = ""; send.disabled = false; }, 1200);
    };
    btns.append(input, send);
  }
  if(t.state === "suspended") btns.append(note("挂起中 —— 网络类会自动恢复"), btn_resume(t.id));
  if(t.state === "blocked") btns.append(note("⚑ 需要你的决定"), btn_resume(t.id), btn_cancel(t.id));
  if(t.state === "failed") btns.append(btn_resume(t.id), btn_cancel(t.id));
  if(t.state === "done") btns.append(note("✓ 完成 · 花费见 maestro task ledger " + t.id));
  if(btns.children.length) c.appendChild(btns);
  return c;
}
function note(txt){ const s = document.createElement("span"); s.className="hint"; s.textContent = txt; return s; }
function btn_resume(id){
  const b = document.createElement("button"); b.textContent = "▶ 恢复";
  b.onclick = async () => { b.disabled = true; await fetchJson("/api/task/resume", {method:"POST", body: JSON.stringify({task:id})}); refreshTasks(); };
  return b;
}
function btn_cancel(id){
  const b = document.createElement("button"); b.className="warn"; b.textContent = "✕ 放弃";
  b.onclick = async () => { b.disabled = true; await fetchJson("/api/task/cancel", {method:"POST", body: JSON.stringify({task:id})}); refreshTasks(); };
  return b;
}

function evText(d){
  switch(d.type){
    case "task_created": return `新任务「${(d.task&&d.task.title)||""}」`;
    case "task_started": return "开始执行";
    case "round_progress": return `第 ${d.round} 轮：${(d.summary||"").slice(0,40)}`;
    case "task_completed": return "✓ 完成";
    case "task_failed": return `✗ 失败：${(d.error||"").slice(0,50)}`;
    case "task_cancelled": return "已取消";
    case "task_requeued": return "已重新排队（重试）";
    case "steering_queued": return `💬 轻推：${(d.message||"").slice(0,30)}`;
    case "steering_delivered": return "轻推已投递（本轮生效）";
    case "steering_dropped": return "轻推已丢弃（任务终态）";
    case "suspended": return `挂起（${d.reason||""}）`;
    case "resumed": return "已恢复";
    case "rounds_exhausted": return `⚑ ${d.rounds} 轮预算耗尽，等你决定`;
    case "acceptance_gate_failed": return `⚠ 假完成被拦（第 ${d.failures} 次，3 次出局）`;
    case "acceptance_gate_passed": return "验收通过";
    case "auto_recovery_exhausted": return "⚑ 自动恢复耗尽，等你决定";
    case "cost_drift": return `⚠ 费用对账漂移 ${d.ledger_cents}¢ vs ${d.cli_cents}¢`;
    case "context_compacted": return "上下文已压缩，继续";
    case "checkpoint_created": return "快照已存";
    case "goal_progress": return d.blocked_condition ? `⚑ 目标受阻：${(d.blocked_condition||"").slice(0,30)}` : "目标推进中";
    case "ledger_entry": return "轮账已记";
    case "worker_spawned": return "worker 已启动";
    case "emergency_stopped": return "⛔ 全局急停（现场冻结保留）";
    default: return d.type || "";
  }
}
async function refreshEvents(){
  const v = await fetchJson("/api/events?from=" + lastSeq);
  const box = $("events");
  if(!v || !v.events || !v.events.length) return;
  const rows = [];
  for(const e of v.events.slice(-60)){
    lastSeq = Math.max(lastSeq, e.seq);
    const div = document.createElement("div"); div.className = "ev";
    const t = document.createElement("span"); t.className = "t";
    t.textContent = new Date(e.ts).toLocaleTimeString("zh-CN", {hour12:false});
    const s = document.createElement("span"); s.textContent = evText(e.event || {});
    div.append(t, s);
    const tid = e.event && typeof e.event.task === "string" ? e.event.task
              : e.event && e.event.task && e.event.task.id;
    if(typeof tid === "string" && tid){
      const id = document.createElement("span"); id.className = "taskid"; id.textContent = tid;
      div.append(id);
    }
    rows.push(div);
  }
  box.textContent = "";
  rows.reverse().forEach(x => box.appendChild(x));  // 最新在上
}

refreshTasks(); refreshEvents();
setInterval(refreshTasks, 2000);
setInterval(refreshEvents, 2000);
</script>
</body>
</html>
"#;
