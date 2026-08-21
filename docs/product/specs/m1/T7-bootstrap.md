# T7 — Admin Token 首启引导

> 依赖: T2 | **裁决：无独立代码产出，逻辑并入 T2 `TokenService::new`** | 上游: M1 spec §3.1

## 1. 归属裁决

T7 不产生独立代码文件。理由：引导逻辑约 20 行，唯一调用点是 `TokenService::new`（服务构造即引导），拆独立模块徒增一层间接。本文件保留为**行为契约 + 验收清单**，供审查与 T8 集成测试引用；实现细节以 T2 spec §8 为准。

## 2. 行为契约（引用 T2 §8，此处为验收视角重述）

| # | 契约 | 验证方式 |
|---|------|----------|
| B1 | 全新 DB 首启：自动生成 `pony_admin_<48hex>`，name=`__admin__`，expires_at=NULL，落库（insert_token 无 is_admin 参数，C-P1-3） | T8 步骤 1：日志出现 `ADMIN_TOKEN (仅此一次，请立即保存): pony_admin_...` |
| B2 | 明文仅打印一次（warn 级），此后任何启动不再打印 | 重启 server（T8 步骤 11），journal 无第二条 ADMIN_TOKEN |
| B3 | 已有未撤销 `__admin__` 行 → 不生成不打印 | 同上 |
| B4 | `__admin__` 被撤销后重启：**不**自动重新生成（防"撤销即重置"被误用为提权路径；恢复 admin 走 DB 手工运维：`UPDATE tokens SET revoked_at=NULL WHERE name='__admin__'`） | 单元测试：revoke admin → 重建 TokenService → 日志无 ADMIN_TOKEN |
| B5 | admin token 经 `verify_admin` 可过管理面；数据 token 不可 | T2 单测 8 + T6 单测 2 |
| B6 | 明文不出现在 DB（仅 64hex 哈希）与错误信息中 | 审查：`python3 -c "..."`（sqlite3 模块，C-P2-7）查 `select token_hash from tokens` 无明文 |
| B7 | **环境变量注入（S-P1-3）**：`PPROXY_ADMIN_TOKEN` 已设置且 DB 无未撤销 `__admin__` 行 → 跳过生成，以其哈希落库，**不打印** ADMIN_TOKEN；注入值可过 `verify_admin` | 单元测试：设 env 构造 TokenService → admin_exists()==true、日志无 ADMIN_TOKEN、注入明文 verify_admin Ok；T8 可选步骤同断言 |
| B8 | `PPROXY_ADMIN_TOKEN` 为空串 → 视为未设置（走 B1 生成路径） | 单元测试：env="" → 日志出现 ADMIN_TOKEN |

## 3. 依赖任务

T2（实现载体）。

## 4. 单元测试清单（实现在 token.rs 测试模块，此处列验收断言）

1. 临时空 DB 构造 TokenService → `admin_exists()==true`，list_tokens 恰含一行 name=`__admin__` 且 revoked_at=None、expires_at=None。
2. 同一 DB 二次构造 TokenService → 不再新增 admin 行（行数不变）。
3. 撤销 `__admin__` 后重建 → 不重新生成（B4）；`admin_exists()==false`。
4. **注入（B7）**：设 `PPROXY_ADMIN_TOKEN=pony_admin_custom123` 构造 → `admin_exists()==true`、该明文 `verify_admin` Ok、日志无 ADMIN_TOKEN。
5. **空注入（B8）**：`PPROXY_ADMIN_TOKEN=""` → 走生成路径，日志出现 ADMIN_TOKEN。
6. 日志断言：`tracing_test` 或手工检查（允许实现者用 `tracing::subscriber` 捕获，断言首启恰一次 ADMIN_TOKEN 输出；若捕获成本高，允许降级为代码审查项 + T8 日志文件检查，二选一，spec 不强制）。

## 5. 验收标准

- 第 4 节 1-5 项单测全绿（第 6 项按其内注明的降级选项执行）。
- T8 集成步骤 1 通过（日志文件检索 ADMIN_TOKEN）。
- 运维文档要点（写入 T8 归档的 API.md 变更）：admin token 丢失时的恢复路径 = B4 注明的手工 SQL；**规避 journal 明文留存用 `PPROXY_ADMIN_TOKEN` 注入**（systemd `EnvironmentFile=`）；journald 清理命令 `journalctl --vacuum-time=1s`（S-P1-3）。
