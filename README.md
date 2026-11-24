<div align="center">
  <h1>Codex Tools</h1>
  <p><strong>Codex / Claude 多账号管理、CLI/TUI 切换、用量监控、模型降智哨兵检测与本地 OpenAI/Anthropic 兼容 API 反代工具。</strong></p>
  <p>
    <a href="https://github.com/CleoCheng/codex-tools/releases"><img alt="GitHub Release" src="https://img.shields.io/github/v/release/CleoCheng/codex-tools?label=release"></a>
    <a href="https://github.com/CleoCheng/codex-tools/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-MIT-blue"></a>
    <img alt="Platforms" src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS-lightgrey">
  </p>
</div>

## 概述

Codex Tools 面向同时使用多个 Codex / Claude 账号的桌面与本地开发场景，提供桌面应用、原生 CLI 与本地 `/v1` 反代网关。本仓库基于开源项目 [170-carry/codex-tools](https://github.com/170-carry/codex-tools)（MIT License）持续维护与深度增强，重点强化了**模型降智检测与自动恢复/回调容错**及 **Windows 桌面自动化可靠性**。

## 核心特性

- **模型降智指纹检测与自愈反代（增强）**：
  - 支持对 `gpt-6.2-sol` / `gpt-6.2-astra` (`xhigh`) 执行双模型指纹探针，并在账号列表与详情页展示实测指纹、置信度与 TTFT。
  - 反代运行期内置被动哨兵：流式/WebSocket 响应首包通过 SSE `response.model` 预检识别降级模型；非流式与 `/v1/responses/compact` 支持同步复检确认。确认降智后自动停用异常账号、跳过当次统计并将触发请求无缝重放至其他健康替代账号。
  - 依据官方 `5h` / `7d` 限流窗口与指数退避（`1h -> 2h -> 4h -> 8h -> 12h`）调度周期恢复探针，避免在临近限额时无效探测，指纹恢复正常后自动重新加入反代池。
- **Windows 稳定性与自动换号续跑（增强）**：
  - 严格的 Codex 进程树/Job 边界保护，仅停止已核验的 Codex 桌面与原生组件进程，不误杀外部编辑器或独立 CLI 会话。
  - 额度耗尽时自动过滤授权异常账号（`401` / `token_revoked` 等）与不可用窗口，完成切号重启并安全定位恢复会话。
  - 清晰的完整重置日期格式（跨午夜不再歧义省略日期）。
- **多账号管理与用量统计**：OAuth 导入、JSON 批量导入/导出、`5h` / `1week` 额度窗口监控、账号优先级排序、账号预热、重置卡及按会话归属的本地 Token / TPS 统计。
- **本地 API 反代与公网隧道**：提供 `/v1/responses`、`/v1/chat/completions`、`/v1/messages`、`/v1/responses/compact`、`/v1/images/*` 及 WebSocket 代理，支持优先级/平均/逐个策略、Codex App/CLI 一键绑定及 `cloudflared` 公网访问。
- **桌面能力**：macOS 状态栏额度展示、Windows 任务栏与托盘额度组件、多语言界面与编辑器联动。

## 安装与使用

### 桌面安装包

可从本仓库 [Releases](https://github.com/CleoCheng/codex-tools/releases) 下载（或从源码本地构建）：

- **Windows x64**：NSIS 安装包（保留现有本机数据目录兼容标识 `com.carry.codex-tools`，可直接覆盖更新已有数据）
- **macOS**：Apple Silicon (`aarch64`) / Intel (`x86_64`) DMG 安装包（可通过 GitHub Actions `Build macOS DMGs` 工作流手动构建）

### 从源码构建

环境要求：Node.js 20+、Rust stable。

```bash
# 安装依赖并启动桌面开发预览
npm install
npm run tauri dev

# 构建前端并生成桌面安装包
npm run build
npx tauri build

# 仅构建原生命令行工具（codex-tools-cli）
cd src-tauri
cargo build --release --bin codex-tools-cli
```

原生 CLI 可直接在构建产物目录运行 `codex-tools-cli`（包含 `list` / `switch` / `login` / `import` / `export` / `usage` / `provider` / `doctor` / `report` / `tui` 等子命令）。

## API Proxy 简明说明

- 默认地址：`http://127.0.0.1:8787/v1`（Anthropic Messages 为 `http://127.0.0.1:8787/v1/messages`）
- 鉴权：应用内生成的 `sk-...` API Key（支持模型白名单、快速模式及密钥级权限控制）
- 支持策略：`Priority`（优先级回落）、`Average`（均衡负载）、`Sequential`（同会话亲和逐个消耗）；探针期与刚隔离的降智账号会自动降权至候选队尾
- 更多说明见 [docs/api-proxy.md](docs/api-proxy.md) 与 [how to use.md](how%20to%20use.md)。

## 文档目录

- [快速使用指南](how%20to%20use.md)
- [API 反代说明](docs/api-proxy.md)
- [Linux 独立反代守护进程](docs/linux-proxyd.md)
- [本地项目保留与恢复](docs/local-project-sharing.md)
- [Codex CLI 配置说明](docs/codex-cli-config.md)
- [Windows 增强版维护记录](docs/custom-windows-maintenance.md)
- [更新日志](changelog.md)

## 仓库结构

```text
src/                     React + TypeScript 前端界面
src-tauri/               Tauri 与 Rust 后端（桌面端、反代网关、指纹检测）
src-tauri/src/bin/       原生 CLI 入口（codex-tools-cli）
src-tauri/proxyd/        Linux 无桌面独立反代守护进程
docs/                    技术使用文档与维护记录
.github/workflows/       桌面端校验、macOS DMG 与多平台发布工作流
```

## 致谢与原作者声明 / Attribution

本项目基于原作者 **[170-carry](https://github.com/170-carry)** 开源的 **[170-carry/codex-tools](https://github.com/170-carry/codex-tools)**（MIT License，基线版本 `v3.2.1`）进行衍生开发与长期定制增强。感谢原作者及其贡献者提供的基础架构、反代协议转换及界面设计。

- 原项目地址：<https://github.com/170-carry/codex-tools>
- 本仓库地址：<https://github.com/CleoCheng/codex-tools>
- 详细版权与第三方组件声明见 [NOTICE](NOTICE)。

## License

本项目遵循 **MIT License**，完整协议与双方版权声明见 [LICENSE](LICENSE)；`src-tauri/src/proxy_service/catalog/` 中的模型目录资源遵循 Apache License 2.0，见 [catalog LICENSE](src-tauri/src/proxy_service/catalog/LICENSE)。
