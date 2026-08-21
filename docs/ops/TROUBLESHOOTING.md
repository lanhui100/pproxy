# 故障排查手册

## 症状速查

### 国内 DNS 解析失败（NXDOMAIN）
- **原因**：子域名含敏感词（如 "proxy"）被国内 DNS 过滤（ADR-003）
- **排查**：`dig @223.5.5.5 <域名>` vs `dig @8.8.8.8 <域名>`——海外通国内不通即命中
- **解决**：换中性子域名（edge/vedge/access）；勿用 /etc/hosts 硬编码（IP 变更脆弱）

### OpenAI 返回 403 unsupported_country_region_territory
- **原因**：请求走了 CF Worker 上游（AS13335 被 OpenAI 整段拉黑）
- **解决**：确认路由走 vercel 上游（config.json route_upstreams.openai=vercel）

### zen 返回 RegionError
- **原因**：同上（CF→CF 内部流量传播入口国家）
- **解决**：走 vercel 上游

### zen 返回 DataPolicyError
- **原因**：账号未 opt-in 数据政策（非链路问题）
- **解决**：访问错误信息中的 opencode.ai workspace 链接点同意

### 上游 403 Unauthorized
- **原因**：X-Proxy-Secret 不匹配
- **排查**：config.json worker_secret / upstreams.*.secret 与 Worker vars / Vercel env 是否一致

### Google 429
- **原因**：CF 出口 IP 被 Google 限流（瞬时）
- **解决**：重试；持续出现则考虑该路由切 vercel

### Facebook/上游偶发 000 或 Broken pipe
- **原因**：上游瞬时抖动或客户端提前断开（日志可见 client error: Broken pipe，无害）
- **解决**：重试；持续失败按"上游可达性"检查（DEPLOY.md 监控点）

### LLM 长响应被掐断（约 300s）
- **原因**：Vercel Hobby 函数 maxDuration 300s（Fluid compute 上限）
- **缓解**：流式模式一般够用；超长任务等待 P2 自定义上游

### vercel.app 域名返回 Login 页面
- **原因**：项目 ssoProtection=all_except_custom_domains（vercel.app 域名有登录墙）
- **解决**：始终使用自定义域名 vedge.ponyjob.top

### 服务无响应
```bash
systemctl status pproxy            # 进程状态
journalctl -u pproxy -n 50         # 最近日志
ss -tlnp | grep 8899               # 端口监听
curl -s http://127.0.0.1:8899/     # 网关健康
```

### CF Worker 部署报 Authentication error (code 10000)
- **原因**：wrangler token 权限不足（无 zone DNS 写权限）
- **现状**：custom_domain 方式可部署成功；直接 DNS API 不可用
- **解决**：域名/DNS 变更走 CF 控制台手动操作

## 诊断工具

```bash
# 出口 IP 验证（经 Worker）
curl -s "https://edge.ponyjob.top/get?url=https://httpbin.org/ip" -H "X-Proxy-Secret: <REDACTED_DEV_SECRET>"
# 期望: CF 出口 IP（104.22.x / 2a06:98c0::），不含 115.63.x（真实 IP 泄露检查）

# 出口 IP 验证（经 Vercel）
curl -s "https://vedge.ponyjob.top/api/proxy?url=https%3A%2F%2Fhttpbin.org%2Fip" -H "X-Proxy-Secret: <REDACTED_DEV_SECRET>"
# 期望: 3.x.x.x（AWS us-east）
```
