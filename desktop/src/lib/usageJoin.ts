// 用量明细 × 设备密钥名 join 纯函数（SPEC §3.7）：
// 命中令牌 → 名字；revoked → 「名字（已撤销）」；rows 中出现但列表缺失的 id → '#id' 回退。
// 同一 id 多行只映射一次；返回 Map 供视图 O(1) 查表。

export function joinUsageTokenName(
  rows: { token_id: number }[],
  tokens: { id: number; name: string; status: string }[],
): Map<number, string> {
  const byId = new Map(tokens.map((t) => [t.id, t]))
  const out = new Map<number, string>()
  for (const row of rows) {
    if (out.has(row.token_id)) continue
    const token = byId.get(row.token_id)
    let label = `#${row.token_id}`
    if (token) {
      label = token.status === 'revoked' ? `${token.name}（已撤销）` : token.name
    }
    out.set(row.token_id, label)
  }
  return out
}
