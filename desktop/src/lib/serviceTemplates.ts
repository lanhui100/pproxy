// 服务模板常量表（M7 SPEC §3.6）：前端常量、与服务端解耦；name 需满足服务端
// 校验 ^[a-z][a-z0-9_-]{0,63}$ 且非 pony_ 前缀（十项均已逐条核对合规）。
//
// target_host 核验记录（实施时经 web_search 对照官方文档逐条核验）：
// 全部十项核验通过并收录，无剔除行。来源以 source 字段随行记录，摘要如下：
// - 核验方法：检索各服务官方文档 / 官方 SDK 默认 base_url，确认域名归属与用途；
// - github 收录的是主域 github.com（承载 git smart-HTTP 传输端点），REST API 子域
//   api.github.com 未收录——本网关场景（受限网络访问）以 git 传输为主；
// - hf 收录主域 huggingface.co：Hub API 路径（/api/*）同域直挂，LLM 推理走其子域
//   router.huggingface.co，主域可覆盖两类路径的入口语义。
export interface ServiceTemplate {
  /** 路由名（= 服务名），需通过服务端 name 校验 */
  name: string
  /** 展示名（模板网格按钮主文案） */
  label: string
  /** 目标 host（SSRF 校验要求裸域名，不带 scheme 与路径） */
  target_host: string
  /** 核验来源（官方文档/SDK 基址出处摘要） */
  source: string
}

export const SERVICE_TEMPLATES: ServiceTemplate[] = [
  {
    name: 'openai',
    label: 'OpenAI',
    target_host: 'api.openai.com',
    // 核验：OpenAI 官方 SDK 默认基址 https://api.openai.com/v1（platform.openai.com / openai-python）
    source: 'OpenAI 平台文档与官方 SDK 默认 base_url api.openai.com/v1',
  },
  {
    name: 'anthropic',
    label: 'Anthropic',
    target_host: 'api.anthropic.com',
    // 核验：Anthropic Messages API 官方端点 https://api.anthropic.com/v1/messages（docs.anthropic.com）
    source: 'Anthropic 官方文档 Messages API 端点 api.anthropic.com',
  },
  {
    name: 'gemini',
    label: 'Gemini',
    target_host: 'generativelanguage.googleapis.com',
    // 核验：Google Gemini API 官方端点 https://generativelanguage.googleapis.com（ai.google.dev/gemini-api/docs）
    source: 'Google AI for Developers Gemini API 文档 generativelanguage.googleapis.com',
  },
  {
    name: 'github',
    label: 'GitHub',
    target_host: 'github.com',
    // 核验：GitHub 官方域名承载 git smart-HTTP 远程端点（docs.github.com「关于远程仓库」，
    // remote 形如 https://github.com/owner/repo.git）；REST API 在子域 api.github.com，不在此列。
    source: 'GitHub 官方文档 about-remote-repositories（github.com 为 git HTTPS 传输主域）',
  },
  {
    name: 'x',
    label: 'X (Twitter)',
    target_host: 'api.twitter.com',
    // 核验：X API v2 官方基址仍为 https://api.twitter.com/2（developer.x.ai / developer.twitter.com）；
    // 与 crates/core/src/store/tests.rs 夹具 "x": "api.twitter.com" 一致。
    source: 'X 开发者文档 v2 API 基址 api.twitter.com（品牌迁移后域名未变）',
  },
  {
    name: 'openrouter',
    label: 'OpenRouter',
    target_host: 'openrouter.ai',
    // 核验：OpenRouter 快速开始基址 https://openrouter.ai/api/v1（openrouter.ai/docs/quickstart），host=openrouter.ai
    source: 'OpenRouter 官方 Quickstart openrouter.ai/api/v1',
  },
  {
    name: 'groq',
    label: 'Groq',
    target_host: 'api.groq.com',
    // 核验：Groq OpenAI 兼容基址 https://api.groq.com/openai/v1（console.groq.com/docs/openai）
    source: 'GroqCloud 官方文档 OpenAI Compatibility api.groq.com/openai/v1',
  },
  {
    name: 'mistral',
    label: 'Mistral',
    target_host: 'api.mistral.ai',
    // 核验：Mistral La Plateforme 官方基址 https://api.mistral.ai/v1（docs.mistral.ai）
    source: 'Mistral AI 官方文档 La Plateforme API api.mistral.ai',
  },
  {
    name: 'xai',
    label: 'xAI',
    target_host: 'api.x.ai',
    // 核验：xAI Grok API 官方基址 https://api.x.ai/v1（docs.x.ai/overview 与 Chat Completions 页）
    source: 'xAI 官方文档 docs.x.ai Grok API api.x.ai/v1',
  },
  {
    name: 'hf',
    label: 'Hugging Face',
    target_host: 'huggingface.co',
    // 核验：Hugging Face Hub API 同域路径 huggingface.co/api/*；Inference Providers 的 LLM 推理
    // 走子域 router.huggingface.co（huggingface.co/docs/hub/models-inference）。取主域覆盖入口语义。
    source: 'HF Hub 文档 models-inference（主域 + router 子域均属官方基础设施）',
  },
]
