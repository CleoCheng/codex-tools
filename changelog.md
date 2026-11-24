## 更新日志 / Changelog

### Unreleased

#### English

1. Add dual-model (`gpt-6.2-sol` & `gpt-6.2-astra` `xhigh`) fingerprint degradation detection, passive proxy sentinel monitoring, and auto-disable guards.
2. Add rate-limit-aware recovery probing aligned with official 5h/7d quota windows and exponential backoff (`1h -> 2h -> 4h -> 8h -> 12h`), skipping manually disabled or auth-revoked accounts and automatically re-enabling recovered accounts.
3. Add SSE opening-event `response.model` preflight failover, non-streaming/compact synchronous sentinel verification & replay, and runtime deprioritization for probing/quarantined candidates so triggering requests do not deliver downgraded answers.
4. Preserve local compatibility data identifier (`com.carry.codex-tools`) while updating repository links, documentation, workflows, and MIT attribution (`LICENSE` / `NOTICE`) for `CleoCheng/codex-tools`.

#### 中文

1. 新增 `gpt-6.2-sol` 与 `gpt-6.2-astra`（`xhigh`）双模型降智指纹检测、反代运行期被动哨兵监控与自动停用保护。
2. 新增贴合官方 `5h` / `7d` 限流窗口与指数退避（`1h -> 2h -> 4h -> 8h -> 12h`）的恢复探针调度，自动跳过手动停用及授权失效账号，指纹恢复正常后自动重新启用并入池。
3. 新增流式首包 SSE `response.model` 预检故障转移、非流式与 Compact 请求同步复检回调重放，以及探针/隔离期候选降权，确保触发降智的当次请求不会把降级回答直接返回给客户端。
4. 在保留既有本机数据兼容标识（`com.carry.codex-tools`）的前提下，整理仓库链接、文档、CI 构建工作流与 MIT 开源归属声明（[LICENSE](LICENSE) / [NOTICE](NOTICE)）。

### Baseline

- Based on [170-carry/codex-tools](https://github.com/170-carry/codex-tools) `v3.2.1` (MIT License), with Windows auto-switch/continuation enhancements documented in [docs/custom-windows-maintenance.md](docs/custom-windows-maintenance.md).
