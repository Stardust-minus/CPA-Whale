# CPA Whale

<p align="center">
  <img src="assets/CPAWhaleIcon.png" width="160" alt="CPA Whale 图标">
</p>

CPA Whale 是一个独立、低占用的 Windows 原生桌面挂件，用鲸鱼气泡展示 [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) 的用量、模型、账户额度和可选外部信号。

当前开发版本：**v0.3.3**

- Windows 原生客户端：v0.3.3
- CLIProxyAPI 动态插件：v0.3.3
- Linux 部署管理工具：v0.3.3
- Snapshot API schema：v1
- Capabilities schema：v1
- Plugin config schema：v2
- 已验证 CLIProxyAPI 基线：v7.2.145，C ABI / JSON schema v4

## 特性

### 通用 CLIProxyAPI 数据

- 今日 token、请求数和可选 USD 等价估算
- 当前挂件进程启动后的 token / USD 增量
- 根据服务端实际流量动态发现 provider、model 和 reasoning effort
- 客户端从 `/v1/capabilities` 获取实例名称、模型、功能、默认卡片和轮询间隔
- 新客户端可回退读取旧 v0.1.2 插件；旧 v0.2.6 客户端仍可读取新插件的 `/v1/snapshot`
- quota 账户由服务端 adapter 和 visibility policy 决定，不再写死 Codex Pro 20x
- pricing 使用精确 provider/model/alias/effort 匹配；未定价模型不会按零价处理
- 外部 status、community intelligence、reset event 和 historical risk 均为显式 opt-in

### Windows 桌面体验

- 原版鲸鱼素材、气泡弧线、尾泡动画、Q 弹、吸附、镜像和 Rua GIF
- D3D11、Direct2D、DirectWrite、DirectComposition GPU 渲染，WARP 回退
- 自绘连接向导、菜单、数据设置和详情面板
- 连接前测试 capability/snapshot schema，公网 HTTP 需要二次确认
- 主窗口使用鲸鱼 alpha + 气泡几何区域裁剪；透明方形区域不阻挡下方窗口
- 支持 CPA 根地址、完整插件地址和 `CPAW1-...` connection code
- GUI 选择关注模型、reasoning effort 和启用的数据卡片
- 0.60–2.50 倍连续缩放、多显示器、Per-Monitor DPI V2
- 默认始终置顶，托盘可恢复窗口并打开菜单或详情
- 动画按 DWM 刷新率调度；无活动动画时停止高频计时器
- Whale Token 使用 Windows DPAPI 保存

## 架构

```text
CLIProxyAPI
  └─ cpa-whale plugin
     ├─ usage.handle
     ├─ SQLite WAL + daily rollups
     ├─ pricing / quota adapters
     ├─ optional signal adapters
     └─ authenticated resource API
                    │
                    │ HTTPS + Whale read token
                    ▼
              Windows widget
              ├─ capabilities discovery
              ├─ snapshot polling
              ├─ local startup baseline
              └─ GPU-rendered whale UI
```

详细设计见 [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)。

## 数据语义

CPA Whale 插件默认统计整个 CLIProxyAPI 实例。若该实例由多人共用且没有稳定下游身份，服务端不会通过 IP、User-Agent 或共享 API Key 猜测个人归因。

- `今日`：服务端 reporting timezone 下当前日期的实例统计
- `挂件启动后`：Windows 客户端用本地 baseline 对 lifetime total 求差
- `USD`：配置价格下的 API 等价估算（可采用部署者指定的 Standard/Flex 等费率），不是 provider 或 Codex 账单
- `quota`：只有已实现 adapter 且实际观察到兼容信号的账户才可用；Windows 的“剩余”百分比优先使用账户级 Primary 窗口，不把模型附加额度混入账户最低剩余
- `external signals`：携带来源、抓取时间、过期时间、confidence 和 stale 状态

服务端配置可以提供实例显示名和 scope label，但不会改变数据本身是否支持个人归因。

## 安全与隐私

客户端只持有独立的 Whale 只读 Token，不需要：

- CLIProxyAPI Management Key
- 下游共享 API Key
- OAuth access token
- provider auth 文件

Config v2 支持多个命名 read token，便于按客户端单独吊销。插件配置只保存 SHA-256 digest，客户端使用 DPAPI 加密原始 token。

插件持久化内容不包含：

- prompt、response 或 failure body
- 原始 API Key、access token 或 token hash
- IP、User-Agent、邮箱或 auth 文件路径
- 原始 response headers

外部第三方 signals 在 config v2 中默认关闭，部署者必须显式启用。

## 构建

需要 Docker，本机不需要预装 Rust 或 MinGW。

### Linux 插件和管理工具

```bash
docker build \
  -f build/plugin.Dockerfile \
  --target export \
  --output type=local,dest=build-output/release-linux \
  .
```

输出：

```text
cpa-whale-plugin-linux-amd64.so
cpa-whale-admin-linux-amd64
```

### Windows x64 客户端

```bash
docker build \
  -f build/windows.Dockerfile \
  --target export \
  --output type=local,dest=build-output/release-windows \
  .
```

输出：

```text
cpa-whale-windows-x64.exe
```

### 完整发行 bundle

```bash
python3 scripts/build-release.py
```

脚本从 Cargo metadata 获取统一版本，生成版本化 plugin/admin/client、`release-manifest.json` 和 `SHA256SUMS`。它只复制明确列出的公开文件，不读取或打包 `build-output/whale-read-token.txt`。

## 测试

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p whale-widget-win --target x86_64-pc-windows-gnu -- -D warnings
CPA_WHALE_PLUGIN=target/release/libcpa_whale_plugin.so python3 tests/abi_harness.py
python3 scripts/check-public-tree.py
```

CI 定义见 [`.github/workflows/ci.yml`](.github/workflows/ci.yml)：每次 push/PR 都运行测试并上传三平台构建产物。推送与 workspace 版本一致的 tag（例如 `v0.3.3`）会触发 [`.github/workflows/release.yml`](.github/workflows/release.yml)，再次执行测试、ABI/PE/secret 检查并发布 GitHub Release、版本化二进制、bundle、manifest 和 checksums。

## 部署

推荐使用预构建 Release bundle 中的管理工具：

```bash
./cpa-whale-admin-linux-amd64 check
./cpa-whale-admin-linux-amd64 token generate --endpoint https://your-cpa.example
./cpa-whale-admin-linux-amd64 config render \
  --token-id desktop \
  --token-sha256 <SHA256>
sudo ./cpa-whale-admin-linux-amd64 install \
  --plugin ./cpa-whale-plugin-linux-amd64.so
```

管理工具会进行架构检查、版本化原子复制、配置/数据库备份和本地 install manifest 记录，但不会擅自使用 Management API 或重启 CLIProxyAPI。最终启用仍通过部署者现有的 Management UI/API 工作流完成。

完整原生和 Docker 部署流程见 [`deploy/README.md`](deploy/README.md)。

## Windows 首次连接

首次启动自动打开连接面板。可以输入：

1. CPA 根地址，例如 `https://cpa.example`
2. 完整 Whale API 根地址
3. 管理工具生成的 `CPAW1-...` connection code

连接向导会先验证 capability/snapshot，再保存 endpoint 和 DPAPI token。右键鲸鱼打开菜单后，可进入“数据设置”选择关注模型、effort 和卡片。

## 项目结构

```text
crates/
├── whale-protocol/      # Snapshot 和 capabilities DTO
├── whale-core/          # pricing、baseline、quota 和 reporting day
├── cpa-whale-plugin/    # CLIProxyAPI Linux C-ABI 插件
├── cpa-whale-admin/     # install/check/doctor/rollback 工具
└── whale-widget-win/    # Windows 原生客户端
assets/                  # 鲸鱼、GIF、音效和图标
build/                   # Docker 可复现构建
deploy/                  # config v2、pricing profile、原生/Docker 部署
scripts/                 # release 和 public-tree 安全检查
tests/                   # ABI harness 和外部信号 fixtures
```

## 已知限制

- 当前已验证的预构建平台为 Linux amd64 plugin/admin 和 Windows x64 client
- arm64 目录和配置约定已预留，但未伪称已经验证
- 当前 quota adapter 只实现 Codex response headers；其他 provider 需要新增 adapter
- community intelligence/reset 信息不是官方服务承诺
- Windows EXE 当前未签名，公开测试版可能触发 SmartScreen

## 许可证

项目代码使用 MIT License。第三方项目、鲸鱼素材、GIF、音效和衍生图标声明见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。
