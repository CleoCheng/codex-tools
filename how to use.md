# How to Use

这份文档提供一个简洁的使用流程，适合第一次使用 `Codex Tools` 时快速上手。

## 1. 使用前准备

- 安装或从源码构建并打开 `Codex Tools`
- 准备好一个或多个 Codex / Claude 账号
- 如果要使用编辑器联动或 API 反代，先确保本机已经安装对应工具

## 2. 导入账号

应用启动后，先把账号导入进来。支持以下几种方式：

- OAuth 登录导入
- 上传单个或多个 `.json` 账号文件，或导出的 `accounts.json` 备份
- 直接选择一个文件夹，批量读取其中的账号文件

导入完成后，应用会保留你当前机器上的登录状态，不会直接覆盖正在使用的账号。

## 3. 查看账号用量与模型指纹

导入后，先刷新一次账号列表和用量信息。

你可以在界面中看到：

- 每个账号的 `5h` 与 `1week` 用量窗口及完整重置时间
- 当前账号套餐类型与授权状态
- `gpt-6.2-sol` / `gpt-6.2-astra` 模型指纹检测状态（是否降智、检测置信度与下次恢复探针时间）

这一页主要用来判断哪个账号状态健康、还有可用额度。

## 4. 切换账号并启动 ChatGPT / Codex

确认目标账号后，可以直接执行切换。

常见流程是：

1. 在账号列表中选择一个可用账号
2. 点击切换或启动按钮
3. 等待应用把本机环境切换到对应账号
4. 如果本机装了编辑器，也可以按需触发联动重启

程序会优先启动新版 ChatGPT App，同时兼容旧 Codex App；如果找不到桌面应用，会自动尝试回退到 `codex app`。

如需提前启动某个账号的 `5h` 用量窗口，可在账号详情的快捷操作中选择“激活 5h 窗口”。程序会先刷新用量，只有窗口尚未激活且周额度未耗尽时才发送固定 `hello` 请求；该请求会真实消耗少量额度。设置页可为指定账号开启自动预热，默认关闭，成功后 5 小时内或失败后 10 分钟内不会重复发送。

## 5. 可选：开启 API 反代

如果你希望让其他工具通过 OpenAI / Anthropic 兼容接口接入，可以开启本地 API 反代。

基本流程：

1. 打开应用中的 API 反代面板
2. 选择或确认端口
3. 启动反代服务
4. 复制界面里显示的 `Base URL` 和 `API Key`
5. 在你的客户端中填入这些参数

启动后，本地会提供 `/v1` 兼容接口，并自动从启用且未降智的账号池里选择可用账号转发请求。反代哨兵监测到模型降级时会自动停用问题账号、把触发请求回调重放到其他健康账号，并按限流窗口调度后续恢复探针。

注意：

- 本机直连客户端可以直接使用本地 `Base URL`
- Codex App/CLI 可以在反代面板点击“切到本机反代”，需要恢复时点击“恢复正常地址”
- 负载均衡选择“逐个”时，带有稳定 session key 的同一会话会优先复用同一账号，额度、鉴权失败或进入探针隔离后再切到下一个账号
- `Cursor` 这类可能由服务端代发请求的客户端，不一定允许访问 `127.0.0.1` 或私网地址；如报 `ssrf_blocked`，请改用 `cloudflared` 公网地址或远程 `codex-tools-proxyd`

详细说明可参考 [docs/api-proxy.md](docs/api-proxy.md)。

## 6. 可选：使用原生 CLI 管理账号

如需在终端或脚本中操作账号库，可直接运行安装包附带或源码编译出的原生命令行工具 `codex-tools-cli`：

```bash
# 从源码编译 CLI
cd src-tauri
cargo build --release --bin codex-tools-cli
```

常用命令示例：

| 命令 | 用途 |
| --- | --- |
| `codex-tools-cli list --json` | 列出账号，输出 JSON |
| `codex-tools-cli list --refresh --json` | 刷新用量后列出账号 |
| `codex-tools-cli switch 1 --json` | 切换到第 1 个账号 |
| `codex-tools-cli switch --best --launch` | 自动选择余量更合适的账号，并启动应用 |
| `codex-tools-cli login --label work` | 调用官方 `codex login`，登录后导入账号 |
| `codex-tools-cli import ./auth.json --json` | 导入账号 JSON |
| `codex-tools-cli export ./accounts.json --json` | 导出账号库 |
| `codex-tools-cli usage --cached --json` | 查看本地缓存用量 |
| `codex-tools-cli doctor --json` | 检查本地环境和账号库 |
| `codex-tools-cli report --json` | 输出完整诊断报告 |
| `codex-tools-cli tui` | 打开终端选择器，选择账号后切换 |

说明：

- `switch` 会写入本机 `~/.codex/auth.json` 和 `~/.codex/config.toml`
- `--data-dir <目录>` 可指定账号库目录，适合隔离测试或多环境备份

## 7. 日常使用建议

- 新增账号后先刷新一次用量，必要时做一次模型指纹检测
- 遇到账号 `401` 或授权失效提示时及时重新授权，自动切换与恢复探针会自动跳过失效账号
- 如果长期需要外部工具接入，可以保持 API 反代开启
- 需要暴露给外部环境时，可按需配置 `cloudflared` 或部署 Linux `codex-tools-proxyd`
