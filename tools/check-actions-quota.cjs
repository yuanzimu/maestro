#!/usr/bin/env node
// 干跑 / 发版前查 GitHub Actions 剩余计费分钟 —— 防 spending limit 掐断 CI
//（2026-10-06 事故：一次 dry-run 三平台构建烧掉约 200+ 计费分钟，
// 免费版私有仓库每月仅含 2000 分钟，烧尽后 job 全部秒失败且零日志）。
//
// 计费口径：
//   - 公共仓库 Actions 免费；私有仓库计入配额
//   - 免费版含 2000 分钟/自然月（每月 1 号重置）
//   - 倍率：Linux 1x / Windows 2x / macOS 10x（macOS job 是烧分钟大头）
//   - 一次三平台 dry-run ≈ 200+ 计费分钟；一次 CI 全矩阵 ≈ 100 计费分钟
//
// 用法（Node 22+，零依赖）：
//   node tools/check-actions-quota.cjs [--repo yuanzimu/maestro] [--min-left 300]
// 退出码：0 = 额度充足；1 = 低于阈值（应停手）；2 = API 错误
//
// 两种模式（自动选择）：
//   exact    —— 令牌带 admin:billing scope 时走 billing API（权威数字）
//   estimate —— 否则按本月该仓库 job 时长 × 倍率估算（账户还有其他活跃
//               私有仓库时会偏低，跑之前确认 --repo 覆盖了主要消耗源）
const https = require('https');
const { execSync } = require('child_process');

const argv = process.argv.slice(2);
function argOf(name, dflt) {
  const i = argv.indexOf(name);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : dflt;
}
const REPO = argOf('--repo', 'yuanzimu/maestro');
const MIN_LEFT = Number(argOf('--min-left', '300'));
const FREE_INCLUDED = 2000;
const MULT = { macos: 10, windows: 2 };

// 取令牌：走 git 凭据（不落盘）
const cred = execSync('git credential fill', {
  input: 'protocol=https\nhost=github.com\n\n',
}).toString();
const TOKEN = (cred.match(/password=(.+)/) || [])[1];
if (!TOKEN) { console.error('拿不到 github 令牌（git credential fill）'); process.exit(2); }

function api(path) {
  return new Promise((resolve, reject) => {
    const req = https.request({
      host: 'api.github.com', path, method: 'GET',
      headers: {
        authorization: 'Bearer ' + TOKEN.trim(),
        accept: 'application/vnd.github+json',
        'user-agent': 'check-actions-quota',
      },
    }, res => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => resolve({
        status: res.statusCode,
        scopes: res.headers['x-oauth-scopes'] || '',
        body: Buffer.concat(chunks).toString('utf8'),
      }));
    });
    req.on('error', reject);
    req.end();
  });
}

// runner 标签 → 计费倍率
function multiplier(labels) {
  const s = (labels || []).join(' ').toLowerCase();
  if (s.includes('macos')) return MULT.macos;
  if (s.includes('windows')) return MULT.windows;
  return 1;
}

(async () => {
  const me = await api('/user');
  if (me.status !== 200) { console.error('鉴权失败:', me.status); process.exit(2); }
  const user = JSON.parse(me.body);
  const planName = user.plan && user.plan.name;
  console.log(`账户: ${user.login}（${planName} 计划）`);

  let mode, included, used, breakdown = null;
  let perRun = [];

  const bill = await api(`/users/${user.login}/settings/billing/actions`);
  if (bill.status === 200) {
    // 权威模式：billing API
    const b = JSON.parse(bill.body);
    mode = 'exact（billing API 权威数字）';
    included = b.included_minutes;
    used = b.total_minutes_used;
    breakdown = b.minutes_used_breakdown || null;
  } else {
    // 估算模式：本月（自然月，1 号重置）该仓库全部 run 的全部 attempt
    mode = `estimate（按 ${REPO} 本月 job 时长估算；billing API ${bill.status}，令牌缺 admin:billing）`;
    const since = new Date().toISOString().slice(0, 8) + '01';
    let page = 1;
    const runs = [];
    for (;;) {
      const r = await api(`/repos/${REPO}/actions/runs?per_page=100&page=${page}&created=${encodeURIComponent('>=' + since)}`);
      if (r.status !== 200) { console.error('runs 查询失败:', r.status); process.exit(2); }
      const batch = JSON.parse(r.body).workflow_runs;
      runs.push(...batch);
      if (batch.length < 100) break;
      page++;
    }
    let usedSec = 0;
    for (const run of runs) {
      let runSec = 0;
      // 重跑的每个 attempt 都计费，逐个拉
      for (let a = 1; a <= (run.run_attempt || 1); a++) {
        const j = await api(`/repos/${REPO}/actions/runs/${run.id}/attempts/${a}/jobs?per_page=100`);
        if (j.status !== 200) continue;
        for (const job of JSON.parse(j.body).jobs || []) {
          if (!job.started_at || !job.completed_at) continue;
          const sec = (Date.parse(job.completed_at) - Date.parse(job.started_at)) / 1000;
          if (sec <= 0) continue; // 计费拦截的 job 零时长，不计
          runSec += sec * multiplier(job.labels);
        }
      }
      if (runSec > 0) perRun.push({ name: run.name, no: run.run_number, min: Math.ceil(runSec / 60) });
      usedSec += runSec;
    }
    // GitHub 按 job 向上取整到分钟
    used = Math.ceil(usedSec / 60);
    included = planName === 'free' ? FREE_INCLUDED : null;
    if (included === null) {
      console.log('注意：非 free 计划的含费额度需要 billing API（当前令牌无 admin:billing scope）');
    }
  }

  const remaining = included === null ? null : included - used;
  console.log(`模式: ${mode}`);
  console.log(`本计费月已用: ${used} 分钟${included === null ? '' : ` / 含 ${included} 分钟`}`);
  if (breakdown) {
    console.log(`分平台: ${Object.entries(breakdown).map(([k, v]) => `${k}=${v}`).join(' ')}`);
  }
  if (perRun.length) {
    perRun.sort((a, b) => b.min - a.min);
    console.log('消耗 Top（分钟）: ' + perRun.slice(0, 3).map(r => `${r.name}#${r.no}=${r.min}`).join(', '));
  }
  if (remaining === null) {
    console.log('无法计算剩余额度（缺 billing API），只报用量，请人工判断后继续');
    process.exit(0);
  }
  console.log(`剩余: ${remaining} 分钟（阈值 ${MIN_LEFT}，每月 1 号重置）`);
  if (remaining < MIN_LEFT) {
    console.error(`⛔ 额度不足：剩余 ${remaining} < ${MIN_LEFT}。建议：暂停 dry-run/CI，或到 Billing & plans 提高 spending limit，或临时公开仓库（公共仓库免费）`);
    process.exit(1);
  }
  console.log('✅ 额度充足，可以跑');
  process.exit(0);
})().catch(e => { console.error('FAIL:', e.message); process.exit(2); });
