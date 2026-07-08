# Genjux-Store

**Genjux-Store** 是一个跨平台（macOS / Windows / Linux / Android）开源软件发现与安装客户端，目标是解决开源网站（GitHub、Gitee 等）上的项目普遍缺乏傻瓜式安装体验的痛点。

Genjux-Store is a cross-platform (macOS / Windows / Linux / Android) open-source software discovery & installation client, aiming to solve the pain point that most open-source projects hosted on GitHub/Gitee lack a friendly one-click install experience.

## 核心理念 / Core idea

- 根据当前运行平台自动展示推荐软件，一键安装、试用。
- 不重新打包或托管第三方二进制文件，只代理下载官方 GitHub Release 的原始产物，在本地执行安装——降低法律与安全责任，且对项目维护者零侵入。
- 核心业务逻辑（发现、元数据解析、下载、校验、安装编排）是一个跨平台共享的本地服务，供原生 GUI、CLI 与 AI agent（MCP）三种调用方共用同一份状态。

## 项目状态 / Status

早期架构规划阶段，尚未开始编码实现。详见 [`.copilot-workflow/PLAN.md`](.copilot-workflow/PLAN.md) 获取完整的架构讨论与决策记录（分层架构、技术选型、安全与信任模型、商业模式方向、路线图等）。

This project is in early architecture-planning stage; implementation has not started yet. See [`.copilot-workflow/PLAN.md`](.copilot-workflow/PLAN.md) for the full architecture discussion and decisions made so far (layering, tech stack choices, security/trust model, monetization direction, roadmap, etc).

## License

MIT License — see [LICENSE](LICENSE).
