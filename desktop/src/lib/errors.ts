// 服务端错误串字典：crates/server/src/api.rs 的 4xx error 常量 → 中文。
// client.ts 保持冻结；视图层经 errText() 消费，未命中回退原文（不猜语义）。
import { errorMessage } from '@/api/client'

const DICT: Record<string, string> = {
  bad_request: '请求参数无效',
  unauthorized: '登录凭据无效',
  not_found: '目标不存在（可能已被删除）',
  'cannot revoke admin': '系统管理员令牌不可撤销',
  'invalid expires_days': '有效期不合法（需为正整数天数）',
  'invalid route name': '服务名称不合法（小写字母开头，仅小写字母、数字、-、_）',
  'invalid target host': '目标地址不合法（请填域名本身，不带 https:// 与路径）',
  'invalid token name': '密钥名称不合法',
  'invalid upstream': '上游线路不合法（worker / vercel）',
}

/** 视图层错误文案入口：api 类错误先查字典译中文，其余沿用 client 分流表文案。 */
export function errText(e: unknown): string {
  const raw = errorMessage(e)
  return DICT[raw] ?? raw
}
