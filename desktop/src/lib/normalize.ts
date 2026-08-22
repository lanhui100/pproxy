// 输入规范化（M5 验收实测：URL 粘贴带空格导致连接失败——最高频真实故障源）：
// - URL：去首尾空白/零宽不可见字符、自动补 http://、去结尾斜杠
// - token：去所有空白（token 为 hex 串，合法内容不含空白；粘贴常带换行）
const INVISIBLE = /[\u200B-\u200D\uFEFF]/g

export function normalizeBaseUrl(input: string): string {
  let s = input
    .trim()
    .replace(INVISIBLE, '')
    .replace(/\s+/g, '')
  if (!s) return ''
  if (!/^https?:\/\//i.test(s)) s = `http://${s}`
  return s.replace(/\/+$/, '')
}

export function normalizeToken(input: string): string {
  return input.replace(/[\s\u200B-\u200D\uFEFF]/g, '')
}
