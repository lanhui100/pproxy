# Agent Note: 集群滚动升级引擎落地（pproxy cluster upgrade 真实执行化）

Status: implemented

## Problem

`pproxy cluster upgrade` 此前是"横幅级"实现：`--minio` / `--r2` 分发源只被解析与打印、
无任何下载代码；横幅声称的"广播 Draining / 15s 排空 / 重启服务自检探活"全是占位文案，
实际行为只有 `--local` 分支的裸拷贝；`--sig` 签名校验只打印"正在比对"却从未真正验签。
运维据此自动化升级会被误导——二进制可能根本没换成，或换了却没重启、旧进程继续跑。
集群节点还无法自动重启：systemd 单元部署的 `pproxy-server` 与 `cluster join --auto-start`
拉起的 serve 守护都没有受管的替换/重启闭环。

## Decision

把 `pproxy cluster upgrade`（`crates/cli/src/cmd/cluster.rs`）从占位实现改写为真实执行的
**本节点滚动升级引擎**，流程为：拉取升级包 → SHA-256 + Ed25519 硬校验 → 排空等待 →
原子备份替换 → 重启 + 健康检查。

1. **三类分发源全部落地**：
   - `--local <path>`：本地文件读取；
   - `--minio` / `--r2`：`fetch_binary_from_url` 经 reqwest 下载，支持 `http(s)://` 直链与
     `s3://<bucket>/<key>`（path-style，端点由 `R2_ENDPOINT` / `S3_ENDPOINT` / `MINIO_ENDPOINT`
     环境变量注入）；产物 ≥512KB 防错误页误替换。
2. **真实密码学校验**（`crates/core/src/auth.rs` 新增原语）：
   - `TokenSigner::sign_bytes` / `from_seed_hex`、`TokenVerifier::verify_bytes`；
   - 签名约定：**对升级包 SHA-256 摘要做 Ed25519 签名**，`.sig` 为 64 字节 HEX；
   - `pproxy cluster upgrade --sig <file>` 验签失败**必须中止、绝不替换**；验签公钥解析顺序
     `--verify-key` → `USER_VERIFYING_KEY` → `~/.pony/cluster_verifying_key.hex` →
     `~/.pony/cluster_signing_key.hex`（管理机私钥种子派生公钥）。
3. **真实排空与原子替换**：`--drain-wait <s>`（默认 3）真实 sleep 收尾在途短请求（长连接由
   HA Forwarder 熔断漂移接管，见 implemented/architecture/2026-09-25-distributed-commercialization.md）；
   `atomic_replace` 同分区 `.tmp` + rename，旧版本备份为 `<target>.old`，替换失败尝试还原。
4. **目标路径与重启闭环**：
   - `--target <path>` 可指定替换 systemd 服务二进制（如 `/opt/pproxy/target/release/pproxy-server`），
     默认替换当前 CLI 自身；
   - 重启优先级：Linux systemd（`service::systemd_action` 自动适配 root/system 与 `--user` 单元
     `pproxy-server` / `pproxy`）→ `~/.pony/pproxy-serve.pid` 守护回退（SIGTERM → 等 18899 端口关闭 →
     按 `pproxy serve --lan` 重新拉起）→ 均不可用时打印人工指引；
   - 重启后 `health_check` 轮询 `127.0.0.1:18899` / `8899` 根路径 2xx（30s 超时）。
5. **配套**：
   - `cluster join --auto-start` 复用 `spawn_serve_daemon` 并记录 PID 至 `~/.pony/pproxy-serve.pid`
     供重启回退路径使用；
   - 新增 `pproxy user sign <file>`：管理机用 `~/.pony/cluster_signing_key.hex` 为升级包生成 `.sig`
     （默认 `<file>.sig`），与节点侧验签约定对称；
   - CLI 版本升至 `0.3.57`。

## Alternatives considered

- **继续用 GitHub Release 直链（复用 `pproxy upgrade` 的 download_binary）**：`upgrade` 的分发源是
  版本化 GitHub/镜像 URL，适合公开发布；集群内网节点更常从私有对象存储（MinIO/R2）或 P2P 本地
  推送取包，且对象存储可离线、免外网。最终保留 `pproxy upgrade` 不动，`cluster upgrade` 独立实现
  私源分发。
- **S3 签名请求（SigV4）**：私有桶需签名头才能 GET。当前文档约定使用公开桶/预签名 URL/自定义
  域名直链，避免引入 aws-sigv4 依赖与凭据编排；签名验证防篡改已由 `.sig` 层覆盖。
- **真正的 Gossip 广播 Draining**：运行时并无已接线的 gossip 总线（ClusterManager 仅存于测试），
  "广播 Draining" 无真实接收方。诚实方案是：排空窗口 = 短等待 + HA Forwarder 熔断漂移，二者已覆盖
  业务感知，故不引入伪广播。
- **升级后强杀进程再拉起（无 PID 文件）**：`cluster join --auto-start` 早期不记录 PID，无法安全定位
  旧 serve 进程。改为记录 `pproxy-serve.pid` 并以端口关闭为同步信号，避免误杀无关进程。
- **签名原文用裸二进制而非摘要**：Ed25519 对任意长度消息签名在约定与跨工具校验上更易错；
  统一"先 SHA-256 再签名"，与 `pproxy user sign` / `cluster upgrade` 双侧共享同一原语。

## Consequences

- `pproxy cluster upgrade` 从"占位横幅"变为可安全自动化的本节点升级闭环：失败中止、替换原子、
  重启受管、恢复可验；集群滚动升级 SOP 见 `docs/ops/ROLLING-UPGRADE.md`。
- 行为变更：横幅不再声称不存在的"广播 Draining / 15s 熔断"，输出与实际执行一致。
- 破坏性：无——全部为 CLI 行为新增/修正，无线协议、配置格式、存储格式变更；
  心跳 schema（`CLUSTER_SCHEMA_VERSION=1` + `#[serde(default)]`）保持跨版本节点共存。
- 已知边界：systemd 单元与 serve 守护均探测不到时（非 Linux + 无 PID 文件）只替换不重启，
  打印人工指引；`cluster status` 的 peer 版本列仍显示 `-`（gossip 未接线），版本验证靠逐节点
  `pproxy --version`（SOP 已注明）。
