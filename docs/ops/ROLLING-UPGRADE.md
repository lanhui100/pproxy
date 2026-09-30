# 集群滚动升级 SOP（Rolling Upgrade）

> 本手册回答："项目有新版本时，分布式集群怎么优雅升级？升级是否有破坏性？"
> 配套决策：`.agents/notes/implemented/feature/2026-09-27-cluster-rolling-upgrade-engine.md`。

## 0. 一句话结论

**逐节点滚动升级，全程零停机、非破坏性**：每个节点 = 拉取新包 → Ed25519 验签 → 原子替换 →
重启 → 健康检查；HA Forwarder 常驻 `8899` 门面，引擎重启窗口内请求 0 延时漂移至备灾节点，
业务无感知。升级顺序：**非种子节点先升，种子节点最后**。

## 1. 前置：发布升级包并签名（管理机，一次）

```bash
# 1) 管理机生成密钥对（仅首次；私钥 0600 只留管理机）
pproxy user keygen

# 2) 构建发布二进制（如 target/release/pproxy）
cargo build --release -p pproxy-cli
#    ⚠️ 集群分发推荐 GitHub Release 的 musl 静态产物（glibc 兼容见 §5.7）；
#    本地 GNU 构建仅适配与构建机同代 glibc 的节点

# 3) 为升级包生成 .sig（对升级包 SHA-256 摘要做 Ed25519 签名，输出 <file>.sig）
pproxy user sign target/release/pproxy

# 4) 分发：本地推送 / MinIO / R2（公开桶或预签名 URL / 自定义域名直链）
#    节点侧记录验签公钥：
#       export USER_VERIFYING_KEY=<公钥HEX>   # 或写入节点 ~/.pony/cluster_verifying_key.hex
```

## 2. 逐节点滚动升级

对**每个节点**依次执行（一个节点完全恢复后再动下一个）：

```bash
# 方案 A：本地推送升级包（P2P 流式推送，推荐）
pproxy cluster upgrade --local /path/to/pproxy \
    --sig /path/to/pproxy.sig \
    --target /opt/pproxy/target/release/pproxy-server \   # systemd 节点指向服务二进制
    --drain-wait 3

# 方案 B：对象存储分发（minio / r2，二选一）
pproxy cluster upgrade --minio https://minio.example.com/pproxy-releases/v0.3.57/pproxy --sig pproxy.sig ...
pproxy cluster upgrade --r2 s3://pproxy-releases/v0.3.57/pproxy --sig pproxy.sig ...   # 需 R2_ENDPOINT
```

命令执行的实际行为（输出与执行一致，不再有占位横幅）：

| 步骤 | 实际行为 |
|---|---|
| `[1/4]` | 拉取升级包（local 读取 / URL 下载），SHA-256 记录；`--sig` 时 Ed25519 硬校验，**失败立即中止、不替换** |
| `[2/4]` | 排空等待 `--drain-wait <s>`（默认 3s），在途短请求收尾；长连接由 HA Forwarder 熔断漂移接管 |
| `[3/4]` | 原子备份替换：旧版备份 `<target>.old`（可回滚），同分区 `.tmp` + rename，失败尝试还原 |
| `[4/4]` | 重启服务（systemd 自动适配 root/system 与 `--user`；或 serve PID 守护回退）+ 30s 健康检查（`18899` / `8899` 恢复 2xx） |

### 2.1 关键参数

| 参数 | 说明 |
|---|---|
| `--local` / `--minio` / `--r2` | 分发源三选一（必填）；s3:// 需 `R2_ENDPOINT` / `S3_ENDPOINT` / `MINIO_ENDPOINT` |
| `--sig <file>` | 签名文件（推荐强制）；缺失时仅记录 SHA-256 并警告 |
| `--verify-key <hex>` | 验签公钥显式指定；默认 `USER_VERIFYING_KEY` → `~/.pony/cluster_verifying_key.hex` → 私钥种子派生 |
| `--target <path>` | 替换目标二进制；**systemd 节点请指向服务二进制**（默认当前 CLI 自身） |
| `--drain-wait <s>` | 排空等待秒数（默认 3；0 关闭） |
| `--no-restart` | 仅替换二进制，不重启（用于分批演练） |

### 2.2 升级顺序

1. 先升**一个边缘/非种子节点**，`pproxy cluster status` 确认在线后再铺开；
2. 逐节点推进，**种子节点殿后**（避免 join 引导期间 seed 行为漂移）；
3. 全部完成后再统一验证（见 §4）。

## 3. 回滚（任一节点）

```bash
# 方式一：利用 .old 备份（升级命令自动保留）
cp /opt/pproxy/target/release/pproxy-server.old /opt/pproxy/target/release/pproxy-server
pproxy restart          # 或 systemctl restart pproxy

# 方式二：git 源码回滚（DEPLOY.md §回滚 server）
git checkout <上一可用commit> && cargo build --release && sudo systemctl restart pproxy
```

## 4. 升级验证清单

```bash
pproxy cluster status        # 本节点及对等节点在线 / RTT 正常
curl -s http://127.0.0.1:8899/ | head -c 100     # 门面健康（HA Forwarder 常驻）
curl -s http://127.0.0.1:18899/ | head -c 100    # 引擎恢复（仅启用 HA Forwarder 时）
systemctl is-active pproxy   # systemd 节点
pproxy --version             # 逐节点核对版本（见 §5 边界）
```

## 5. 破坏性评估与已知边界（务必知晓）

**升级本身非破坏性**：二进制原子替换、无配置/数据/线协议变更；心跳 schema 向后兼容
（`CLUSTER_SCHEMA_VERSION=1` + `#[serde(default)]`），新旧节点混跑不崩；SQLite 路由热更无需迁移。
真正的破坏性风险只在**未来 bump schema / 改 config.json / cluster.json 结构**时（需先设计迁移并全节点同步升级）。

已知边界：

1. **`cluster status` 的 peer 版本列显示 `-`**：gossip 心跳未接线到运行时，`version` 字段来自 core
   crate（恒为 `0.1.0`）。**节点版本验证靠逐节点 `pproxy --version`**，不能靠大盘。
2. **systemd 节点**：`cluster upgrade` 默认替换"当前调用命令的那个 exe"（`std::env::current_exe()`）；
   服务跑的是 `/opt/pproxy/.../pproxy-server`，必须用 `--target` 指向服务二进制，再 `pproxy restart`
   或 `systemctl restart`（升级命令会自动尝试，探测不到时打印人工指引）。
3. **serve 守护节点**（`cluster join --auto-start` 拉起）：升级命令通过 `~/.pony/pproxy-serve.pid`
   SIGTERM 旧进程 → 等 `18899` 关闭 → 自动重新拉起；`8899` 由 HA Forwarder 常驻吸收窗口。
4. **`--no-restart`**：只替换不重启，需自行重启生效（演练/分批场景）。
5. **私有对象存储桶**：本实现走公开桶 / 预签名 URL / 自定义域名直链（无 SigV4）；私桶请先生成
   预签名 URL 再用 `--minio/--r2` 直链。
6. **云端边缘组件**（CF Worker / Vercel / Gate）与 Rust 节点升级互不依赖：`wrangler deploy`、
   Vercel 部署、桌面端 updater 各自独立，无需协调。
7. **构建物 glibc 兼容性**：`cargo build` 产物为构建机 glibc 的动态链接二进制（如 Ubuntu 24.04
   构建依赖 `GLIBC_2.39`），在更旧系统（如 Ubuntu 22.04 / GLIBC_2.35）上无法 exec（报
   `GLIBC_2.39 not found`）。集群分发使用 GitHub Release 的 musl 静态产物（`cli-release` 工作流
   cross 构建的 `pproxy-linux-amd64`，静态链接、任意 glibc 可跑），由管理机 `pproxy user sign`
   重新签名后按本 SOP 分发。
8. **单二进制 serve 节点自替换边界**：`serve` 节点以 CLI 自身作为服务二进制，`--target` 与升级
   命令可执行文件同路径时，`atomic_replace` 换掉运行中 inode 后 `current_exe()` 指向 `(deleted)`，
   serve 守护回退无法自动重新拉起（第 4 步报 `自启动服务失败`；此时二进制已替换成功，仅剩拉起）。
   手动拉起：`setsid nohup <target> serve --lan >> ~/.pony/serve.log 2>&1 &`，并将新 PID 回写
   `~/.pony/pproxy-serve.pid`。systemd 直跑 `pproxy-server`、`--target` 指向 server 二进制的节点
   不受影响。
9. **验签公钥不一致**：节点 `~/.pony/cluster_signing_key.hex` 可与管理机不同，或不存在（部分节点
   仅持有 `cluster.json`），节点侧自动解析出的公钥会与管理机签名公钥不匹配而中止升级。统一做法：
   以 `pproxy user sign` 输出中的「验签公钥 (HEX)」显式传给每个节点 `--verify-key <hex>`，不依赖
   节点本地公钥文件。

## 6. 故障演练（可选，验证零停机）

```bash
# 模拟引擎下线（= 升级重启窗口的等价场景）
fuser -k -9 18899/tcp
curl -s -w "\nHTTP_CODE: %{http_code}\n" http://127.0.0.1:8899/   # 应仍联通（failover 至备灾节点）
```
