# Maestro 模型目录 —— 免费 / 低价 API 与模型

> 由 `node tools/sync-model-catalog.mjs` 生成于 2026-10-05。
> 免费层限额会变动，以各官网为准。OpenRouter 名单按 **price**（非 `:free` 后缀）筛选，覆盖更多模型。

## 一、直连 provider 免费 API（推荐：稳定、限额高）

### Groq

- 限额：30 RPM · 1,000 请求/天 · LPU 极低延迟
- 注册 / 取 key：https://console.groq.com/keys
- 模型：`llama-3.3-70b-versatile`、`openai/gpt-oss-120b`、`qwen/qwen3-coder-480b-a35b-instruct`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.groq.com/openai/v1
KEY (环境变量 GROQ_API_KEY)
MODEL    = llama-3.3-70b-versatile
```
```bash
curl https://api.groq.com/openai/v1/chat/completions \
  -H "Authorization: Bearer $GROQ_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"llama-3.3-70b-versatile","messages":[{"role":"user","content":"你好"}]}'
```

### Cerebras

- 限额：30 RPM · 约 100 万 token/天 · ~2,600 tok/s
- 注册 / 取 key：https://cloud.cerebras.ai
- 模型：`llama-3.3-70b`、`llama-4-scout-17b-16e-instruct`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.cerebras.ai/v1
KEY (环境变量 CEREBRAS_API_KEY)
MODEL    = llama-3.3-70b
```
```bash
curl https://api.cerebras.ai/v1/chat/completions \
  -H "Authorization: Bearer $CEREBRAS_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"llama-3.3-70b","messages":[{"role":"user","content":"你好"}]}'
```

### Google Gemini

- 限额：15 RPM · 1,500 请求/天 · 多模态 2M 上下文
- 注册 / 取 key：https://aistudio.google.com/apikey
- 模型：`gemini-2.5-flash`、`gemini-2.5-flash-lite`、`gemini-2.5-pro`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://generativelanguage.googleapis.com/v1beta/openai
KEY (环境变量 GEMINI_API_KEY)
MODEL    = gemini-2.5-flash
```
```bash
curl https://generativelanguage.googleapis.com/v1beta/openai/chat/completions \
  -H "Authorization: Bearer $GEMINI_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"gemini-2.5-flash","messages":[{"role":"user","content":"你好"}]}'
```

### Mistral AI

- 限额：1 请求/秒 · 500K TPM · 约 10 亿 token/月
- 注册 / 取 key：https://console.mistral.ai
- 模型：`mistral-small-latest`、`mistral-medium-latest`、`codestral-latest`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.mistral.ai/v1
KEY (环境变量 MISTRAL_API_KEY)
MODEL    = mistral-small-latest
```
```bash
curl https://api.mistral.ai/v1/chat/completions \
  -H "Authorization: Bearer $MISTRAL_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"mistral-small-latest","messages":[{"role":"user","content":"你好"}]}'
```

### GitHub Models

- 限额：低类 15 RPM · 150/天；fine-grained PAT 需 Models: Read
- 注册 / 取 key：https://github.com/settings/personal-access-tokens
- 模型：`openai/gpt-4.1`、`meta-llama/Llama-3.3-70B-Instruct`、`anthropic/claude-sonnet-4`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://models.github.ai/inference
KEY (环境变量 GITHUB_PAT)
MODEL    = openai/gpt-4.1
```
```bash
curl https://models.github.ai/inference/chat/completions \
  -H "Authorization: Bearer $GITHUB_PAT" \
  -H "Content-Type: application/json" \
  -d '{"model":"openai/gpt-4.1","messages":[{"role":"user","content":"你好"}]}'
```

### Cloudflare Workers AI

- 限额：10,000 Neurons/天 · 50+ 模型
- 注册 / 取 key：https://dash.cloudflare.com
- 模型：`@cf/meta/llama-3.3-70b-instruct-fp8-fast`、`@cf/qwen/qwen3-coder-480b-a35b-instruct`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.cloudflare.com/client/v4/accounts/{ACCOUNT_ID}/ai/v1
KEY (环境变量 CLOUDFLARE_API_TOKEN)
MODEL    = @cf/meta/llama-3.3-70b-instruct-fp8-fast
```
```bash
curl https://api.cloudflare.com/client/v4/accounts/{ACCOUNT_ID}/ai/v1/chat/completions \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"model":"@cf/meta/llama-3.3-70b-instruct-fp8-fast","messages":[{"role":"user","content":"你好"}]}'
```

### SiliconFlow 硅基流动

- 限额：30 RPM · 60K TPM 永久免费档
- 注册 / 取 key：https://cloud.siliconflow.cn
- 模型：`Qwen/Qwen3-8B`、`deepseek-ai/DeepSeek-V3.1`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.siliconflow.cn/v1
KEY (环境变量 SILICONFLOW_API_KEY)
MODEL    = Qwen/Qwen3-8B
```
```bash
curl https://api.siliconflow.cn/v1/chat/completions \
  -H "Authorization: Bearer $SILICONFLOW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"Qwen/Qwen3-8B","messages":[{"role":"user","content":"你好"}]}'
```

### ModelScope 魔搭

- 限额：2,000 请求/天 · 国内友好
- 注册 / 取 key：https://www.modelscope.cn/my/myaccesstoken
- 模型：`Qwen/Qwen3-8B`、`Qwen/Qwen3-Coder-480B-A35B-Instruct`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api-inference.modelscope.cn/v1
KEY (环境变量 MODELSCOPE_API_KEY)
MODEL    = Qwen/Qwen3-8B
```
```bash
curl https://api-inference.modelscope.cn/v1/chat/completions \
  -H "Authorization: Bearer $MODELSCOPE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"Qwen/Qwen3-8B","messages":[{"role":"user","content":"你好"}]}'
```

### Cohere

- 限额：1,000 次调用/月
- 注册 / 取 key：https://dashboard.cohere.com/api-keys
- 模型：`command-r-plus-08-2024`、`command-a-03-2025`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.cohere.ai/compatibility/v1
KEY (环境变量 COHERE_API_KEY)
MODEL    = command-r-plus-08-2024
```
```bash
curl https://api.cohere.ai/compatibility/v1/chat/completions \
  -H "Authorization: Bearer $COHERE_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"command-r-plus-08-2024","messages":[{"role":"user","content":"你好"}]}'
```

### NVIDIA NIM

- 限额：约 40 RPM · 100+ 模型（开发者会员免费）
- 注册 / 取 key：https://build.nvidia.com
- 模型：`meta/llama-3.3-70b-instruct`、`qwen/qwen3-coder-480b-a35b-instruct`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://integrate.api.nvidia.com/v1
KEY (环境变量 NVIDIA_API_KEY)
MODEL    = meta/llama-3.3-70b-instruct
```
```bash
curl https://integrate.api.nvidia.com/v1/chat/completions \
  -H "Authorization: Bearer $NVIDIA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"meta/llama-3.3-70b-instruct","messages":[{"role":"user","content":"你好"}]}'
```

### SambaNova

- 限额：20 RPM · 20 万 token/天 · RDU 极速
- 注册 / 取 key：https://cloud.sambanova.ai
- 模型：`Meta-Llama-3.3-70B-Instruct`、`Qwen3-Coder-480B`

**Maestro / 任意 OpenAI 客户端填写示例：**

```bash
BASE_URL = https://api.sambanova.ai/v1
KEY (环境变量 SAMBANOVA_API_KEY)
MODEL    = Meta-Llama-3.3-70B-Instruct
```
```bash
curl https://api.sambanova.ai/v1/chat/completions \
  -H "Authorization: Bearer $SAMBANOVA_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"Meta-Llama-3.3-70B-Instruct","messages":[{"role":"user","content":"你好"}]}'
```

## 二、OpenRouter 免费模型（一个 key、22+ 模型）

Base URL：`https://openrouter.ai/api/v1` · key 环境变量 `OPENROUTER_API_KEY` · 注册 https://openrouter.ai/keys · 免费档 200 请求/天

| 模型 | 上下文 | 工具调用 | 模态 |
|---|---|---|---|
| `thinkingmachines/inkling-small:free` | 1048576 | ✓ | text/image/audio |
| `thinkingmachines/inkling:free` | 1048576 | ✓ | text/image/audio |
| `google/lyria-3-pro-preview` | 1048576 | — | text/image |
| `google/lyria-3-clip-preview` | 1048576 | — | text/image |
| `stealth/space-bunny-alpha` | 1000000 | ✓ | text/image/video |
| `nvidia/nemotron-3.5-lightning:free` | 1000000 | ✓ | text |
| `nvidia/nemotron-3-ultra-550b-a55b:free` | 1000000 | ✓ | text |
| `dots-studio/dots-3-note-preview:free` | 512000 | ✓ | text/image |
| `inclusionai/ling-3.1-flash` | 262144 | ✓ | text |
| `apodex/apodex-1.1-mini:free` | 262144 | ✓ | text |
| `inclusionai/ling-3.0-flash-sante:free` | 262144 | ✓ | text |
| `qwen/qwen3.8-27b:free` | 262144 | ✓ | text/image/video |
| `poolside/laguna-s-2.1:free` | 262144 | ✓ | text |
| `poolside/laguna-xs-2.1:free` | 262144 | ✓ | text |
| `google/gemma-4-26b-a4b-it:free` | 262144 | ✓ | image/text/video |
| `google/gemma-4-31b-it:free` | 262144 | ✓ | image/text/video |
| `nvidia/nemotron-3-super-120b-a12b:free` | 262144 | ✓ | text |
| `cohere/north-mini-code:free` | 256000 | ✓ | text |
| `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free` | 256000 | ✓ | text/audio/image/video |
| `openrouter/free` | 200000 | ✓ | text/image |
| `nvidia/nemotron-3.5-content-safety:free` | 128000 | — | text/image |
| `liquid/lfm-2.5-2.6b:free` | 65536 | ✓ | text |

**填写示例（把模型 id 换成上表任一）：**

```bash
BASE_URL = https://openrouter.ai/api/v1
MODEL    = thinkingmachines/inkling-small:free
```
```bash
curl https://openrouter.ai/api/v1/chat/completions \
  -H "Authorization: Bearer $OPENROUTER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"thinkingmachines/inkling-small:free","messages":[{"role":"user","content":"你好"}]}'
```

> 不想挑模型：直接用 `openrouter/free`，由路由器在当前免费模型中自动选择（200K 上下文）。

## 三、低价价值模型（OpenRouter，输入 < $0.15/M，前 15 个）

| 模型 | $/M 输入 | $/M 输出 | 上下文 |
|---|---|---|---|
| `~deepseek/deepseek-flash-latest` | $0.003 | $2.4 | 1048576 |
| `deepseek/deepseek-v4.1-flash` | $0.003 | $2.4 | 1048576 |
| `~deepseek/deepseek-v4-flash-latest` | $0.015 | $1.28 | 1048576 |
| `deepseek/deepseek-v4-flash-0731` | $0.015 | $1.28 | 1048576 |
| `ibm-granite/granite-4.0-h-micro` | $0.017 | $0.112 | 131000 |
| `openai/gpt-oss-20b` | $0.018 | $0.09 | 131072 |
| `mistralai/mistral-nemo` | $0.019 | $0.03 | 131072 |
| `inclusionai/ling-3.0-flash-vl` | $0.021 | $0.062 | 262144 |
| `inclusionai/ling-3.0-flash` | $0.021 | $0.063 | 262144 |
| `deepseek/deepseek-v4-flash` | $0.023 | $1.28 | 1048576 |
| `z-ai/glm-5.2` | $0.024 | $16 | 1048576 |
| `openai/gpt-oss-20b:batch` | $0.024 | $0.112 | 131072 |
| `nex-agi/nex-n2.5-mini` | $0.025 | $0.1 | 262144 |
| `openai/gpt-5-nano:batch` | $0.025 | $0.2 | 400000 |
| `meta-llama/llama-3.2-1b-instruct` | $0.027 | $0.201 | 60000 |

**填写示例：**

```bash
BASE_URL = https://openrouter.ai/api/v1
MODEL    = ~deepseek/deepseek-flash-latest
```
```bash
curl https://openrouter.ai/api/v1/chat/completions \
  -H "Authorization: Bearer $OPENROUTER_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"~deepseek/deepseek-flash-latest","messages":[{"role":"user","content":"你好"}]}'
```
