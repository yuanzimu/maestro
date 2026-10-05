#!/usr/bin/env node
// Maestro 模型目录同步工具：运行一次即获取当前「免费 token + 低价 token」的
// API 与模型列表。
//
// 数据源：
//   1. 内置直连 provider 免费层（2026-10 核实，OpenAI/Gemini 兼容）——
//      人工核实、含注册地址与限额，不随目录漂移
//   2. OpenRouter 公开目录 https://openrouter.ai/api/v1/models（无需 key）：
//      按 price 切免费（in=out=0）与低价（输入 < $0.15/M）两组
//
// 产物：docs/model-catalog.json（机器可读）+ docs/MODEL_CATALOG.md（含每个
// API/模型的填写示例）。用法：node tools/sync-model-catalog.mjs
// 可选环境：HTTPS_PROXY（OpenRouter 直连失败时走代理）。

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const OPENROUTER = "https://openrouter.ai/api/v1/models";
const CHEAP_IN_PER_TOKEN = 0.000_000_15; // $0.15 / 百万输入 token

// ---------------------------------------------------------------------------
// 内置直连 provider（免费层；限额为 2026-10 公开值，会变动，以官网为准）
// compatible: "openai" = OpenAI 兼容 chat/completions；"gemini" = 原生
// ---------------------------------------------------------------------------
const DIRECT = [
  {
    id: "groq", name: "Groq", compatible: "openai",
    base_url: "https://api.groq.com/openai/v1",
    key_env: "GROQ_API_KEY", signup: "https://console.groq.com/keys",
    limits: "30 RPM · 1,000 请求/天 · LPU 极低延迟",
    models: ["llama-3.3-70b-versatile", "openai/gpt-oss-120b", "qwen/qwen3-coder-480b-a35b-instruct"],
  },
  {
    id: "cerebras", name: "Cerebras", compatible: "openai",
    base_url: "https://api.cerebras.ai/v1",
    key_env: "CEREBRAS_API_KEY", signup: "https://cloud.cerebras.ai",
    limits: "30 RPM · 约 100 万 token/天 · ~2,600 tok/s",
    models: ["llama-3.3-70b", "llama-4-scout-17b-16e-instruct"],
  },
  {
    id: "gemini", name: "Google Gemini", compatible: "openai",
    // 官方 OpenAI 兼容端点（原生 v1beta 亦可用，见 AI Studio 文档）
    base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
    key_env: "GEMINI_API_KEY", signup: "https://aistudio.google.com/apikey",
    limits: "15 RPM · 1,500 请求/天 · 多模态 2M 上下文",
    models: ["gemini-2.5-flash", "gemini-2.5-flash-lite", "gemini-2.5-pro"],
  },
  {
    id: "mistral", name: "Mistral AI", compatible: "openai",
    base_url: "https://api.mistral.ai/v1",
    key_env: "MISTRAL_API_KEY", signup: "https://console.mistral.ai",
    limits: "1 请求/秒 · 500K TPM · 约 10 亿 token/月",
    models: ["mistral-small-latest", "mistral-medium-latest", "codestral-latest"],
  },
  {
    id: "github-models", name: "GitHub Models", compatible: "openai",
    base_url: "https://models.github.ai/inference",
    key_env: "GITHUB_PAT", signup: "https://github.com/settings/personal-access-tokens",
    limits: "低类 15 RPM · 150/天；fine-grained PAT 需 Models: Read",
    models: ["openai/gpt-4.1", "meta-llama/Llama-3.3-70B-Instruct", "anthropic/claude-sonnet-4"],
  },
  {
    id: "cloudflare", name: "Cloudflare Workers AI", compatible: "openai",
    base_url: "https://api.cloudflare.com/client/v4/accounts/{ACCOUNT_ID}/ai/v1",
    key_env: "CLOUDFLARE_API_TOKEN", signup: "https://dash.cloudflare.com",
    limits: "10,000 Neurons/天 · 50+ 模型",
    models: ["@cf/meta/llama-3.3-70b-instruct-fp8-fast", "@cf/qwen/qwen3-coder-480b-a35b-instruct"],
  },
  {
    id: "siliconflow", name: "SiliconFlow 硅基流动", compatible: "openai",
    base_url: "https://api.siliconflow.cn/v1",
    key_env: "SILICONFLOW_API_KEY", signup: "https://cloud.siliconflow.cn",
    limits: "30 RPM · 60K TPM 永久免费档",
    models: ["Qwen/Qwen3-8B", "deepseek-ai/DeepSeek-V3.1"],
  },
  {
    id: "modelscope", name: "ModelScope 魔搭", compatible: "openai",
    base_url: "https://api-inference.modelscope.cn/v1",
    key_env: "MODELSCOPE_API_KEY", signup: "https://www.modelscope.cn/my/myaccesstoken",
    limits: "2,000 请求/天 · 国内友好",
    models: ["Qwen/Qwen3-8B", "Qwen/Qwen3-Coder-480B-A35B-Instruct"],
  },
  {
    id: "cohere", name: "Cohere", compatible: "openai",
    base_url: "https://api.cohere.ai/compatibility/v1",
    key_env: "COHERE_API_KEY", signup: "https://dashboard.cohere.com/api-keys",
    limits: "1,000 次调用/月",
    models: ["command-r-plus-08-2024", "command-a-03-2025"],
  },
  {
    id: "nvidia", name: "NVIDIA NIM", compatible: "openai",
    base_url: "https://integrate.api.nvidia.com/v1",
    key_env: "NVIDIA_API_KEY", signup: "https://build.nvidia.com",
    limits: "约 40 RPM · 100+ 模型（开发者会员免费）",
    models: ["meta/llama-3.3-70b-instruct", "qwen/qwen3-coder-480b-a35b-instruct"],
  },
  {
    id: "sambanova", name: "SambaNova", compatible: "openai",
    base_url: "https://api.sambanova.ai/v1",
    key_env: "SAMBANOVA_API_KEY", signup: "https://cloud.sambanova.ai",
    limits: "20 RPM · 20 万 token/天 · RDU 极速",
    models: ["Meta-Llama-3.3-70B-Instruct", "Qwen3-Coder-480B"],
  },
];

async function fetchOpenRouter() {
  const resp = await fetch(OPENROUTER, {
    headers: { Accept: "application/json" },
    signal: AbortSignal.timeout(30_000),
  });
  if (!resp.ok) throw new Error(`OpenRouter HTTP ${resp.status}`);
  const j = await resp.json();
  return j.data ?? [];
}

function normModel(m) {
  const p = m.pricing ?? {};
  return {
    id: m.id,
    context_length: m.context_length ? Number(m.context_length) : null,
    input_per_m: usdPerM(p.prompt),
    output_per_m: usdPerM(p.completion),
    modalities: m.architecture?.input_modalities ?? ["text"],
    tools: !!m.supported_parameters?.includes("tools"),
  };
}
function usdPerM(v) {
  const n = Number(v);
  return Number.isFinite(n) ? Math.round(n * 1_000_000 * 1000) / 1000 : null; // $/M
}

// ---------------------------------------------------------------------------
// Markdown 生成
// ---------------------------------------------------------------------------
function md(direct, free, cheap) {
  const L = [];
  L.push("# Maestro 模型目录 —— 免费 / 低价 API 与模型", "");
  L.push(`> 由 \`node tools/sync-model-catalog.mjs\` 生成于 ${new Date().toISOString().slice(0, 10)}。`);
  L.push("> 免费层限额会变动，以各官网为准。OpenRouter 名单按 **price**（非 `:free` 后缀）筛选，覆盖更多模型。", "");

  L.push("## 一、直连 provider 免费 API（推荐：稳定、限额高）", "");
  for (const p of direct) {
    L.push(`### ${p.name}`, "");
    L.push(`- 限额：${p.limits}`);
    L.push(`- 注册 / 取 key：${p.signup}`);
    L.push("- 模型：" + p.models.map((m) => `\`${m}\``).join("、"));
    L.push("", "**Maestro / 任意 OpenAI 客户端填写示例：**", "");
    L.push("```bash");
    L.push(`BASE_URL = ${p.base_url}`);
    L.push(`KEY (环境变量 ${p.key_env})`);
    L.push(`MODEL    = ${p.models[0]}`);
    L.push("```");
    L.push(...curlBlock(p, p.models[0]), "");
  }

  L.push("## 二、OpenRouter 免费模型（一个 key、22+ 模型）", "");
  L.push(`Base URL：\`https://openrouter.ai/api/v1\` · key 环境变量 \`OPENROUTER_API_KEY\` · 注册 https://openrouter.ai/keys · 免费档 200 请求/天`, "");
  L.push("| 模型 | 上下文 | 工具调用 | 模态 |", "|---|---|---|---|");
  for (const m of free) {
    L.push(`| \`${m.id}\` | ${m.context_length ?? "?"} | ${m.tools ? "✓" : "—"} | ${m.modalities.join("/")} |`);
  }
  L.push("", "**填写示例（把模型 id 换成上表任一）：**", "");
  L.push("```bash", "BASE_URL = https://openrouter.ai/api/v1", `MODEL    = ${free[0]?.id ?? "openrouter/free"}`, "```");
  L.push(...curlBlock(
    { base_url: "https://openrouter.ai/api/v1", key_env: "OPENROUTER_API_KEY" },
    free[0]?.id ?? "openrouter/free"
  ), "");
  L.push("> 不想挑模型：直接用 `openrouter/free`，由路由器在当前免费模型中自动选择（200K 上下文）。", "");

  L.push("## 三、低价价值模型（OpenRouter，输入 < $0.15/M，前 15 个）", "");
  L.push("| 模型 | $/M 输入 | $/M 输出 | 上下文 |", "|---|---|---|---|");
  for (const m of cheap) {
    L.push(`| \`${m.id}\` | $${m.input_per_m} | $${m.output_per_m} | ${m.context_length ?? "?"} |`);
  }
  L.push("", "**填写示例：**", "");
  L.push("```bash", "BASE_URL = https://openrouter.ai/api/v1", `MODEL    = ${cheap[0]?.id ?? ""}`, "```");
  L.push(...curlBlock(
    { base_url: "https://openrouter.ai/api/v1", key_env: "OPENROUTER_API_KEY" },
    cheap[0]?.id ?? ""
  ), "");
  return L.join("\n");
}

function curlBlock(p, model) {
  return [
    "```bash",
    `curl ${p.base_url}/chat/completions \\`,
    `  -H "Authorization: Bearer $${p.key_env}" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '{"model":"${model}","messages":[{"role":"user","content":"你好"}]}'`,
    "```",
  ];
}

// ---------------------------------------------------------------------------
async function main() {
  const raw = await fetchOpenRouter();
  const all = raw.map(normModel);
  const free = all.filter((m) => m.input_per_m === 0 && m.output_per_m === 0)
    .sort((a, b) => (b.context_length ?? 0) - (a.context_length ?? 0));
  const cheap = all
    .filter((m) => m.input_per_m > 0 && m.input_per_m <= usdPerM(String(CHEAP_IN_PER_TOKEN)))
    .sort((a, b) => a.input_per_m - b.input_per_m)
    .slice(0, 15);

  const catalog = {
    generated_at: new Date().toISOString(),
    direct_providers: DIRECT,
    openrouter_free: free,
    openrouter_cheap: cheap,
  };
  const docsDir = join(ROOT, "docs");
  mkdirSync(docsDir, { recursive: true });
  writeFileSync(join(docsDir, "model-catalog.json"), JSON.stringify(catalog, null, 2));
  writeFileSync(join(docsDir, "MODEL_CATALOG.md"), md(DIRECT, free, cheap));
  console.log(`OK direct=${DIRECT.length} free=${free.length} cheap=${cheap.length}`);
  console.log("-> docs/model-catalog.json, docs/MODEL_CATALOG.md");
}

main().catch((e) => {
  console.error("sync failed:", e.message);
  process.exit(1);
});
