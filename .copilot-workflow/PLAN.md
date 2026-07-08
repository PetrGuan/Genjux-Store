# Plan

## Problem

开源软件（GitHub/Gitee 上的项目）大多不提供傻瓜式安装体验：用户需要自行判断 Release 里哪个文件对应自己的平台/架构，下载、校验、安装的门槛都很高。Genjux-Store 要做一个跨平台（macOS/Windows/Linux/Android）客户端，按当前平台自动推荐软件，并一键代理下载官方 Release 原始产物、本地执行安装——定位是"发现 + 安装编排层"，不重新打包/托管第三方二进制，以降低法律与安全责任。

**开源决策（已确认）**：项目采用 **MIT License** 开源。原因：① 这类"下载并以较高权限执行别人代码"的工具，闭源本身是额外的信任负担，开源是这个品类的行业默契（Homebrew/winget manifest/Chocolatey 均开源）；② 4 平台原生 UI 工程量大，开源可吸引社区贡献；③ 真正的护城河是元数据质量/信任信号聚合/推荐质量/品牌与社区动能，不是代码保密。MIT（而非 GPL 系）是为了最大化社区贡献意愿。

**产品形态确认（3 种调用方都要支持）**：① 原生 GUI（4 平台）② CLI（类似 `brew install`）③ AI friendly——既暴露 MCP server 给 Claude/Copilot CLI 等 agent 直接调用（search_software/install_software 等工具），也提供通用本地 HTTP/JSON API 给任意程序化调用方。这个"三方调用方共享同一状态"的需求，直接决定了下面的架构选型（core 是本地服务而非各端各自内嵌一份库）。

**分发渠道决策（已确认）：不上架 Mac App Store，也不上架 Google Play，全部走直接分发。** 原因：
- Apple App Store Review Guideline 2.5.2 明确禁止"app 内展示/分发/销售可执行代码，如插件/模块商店"，Genjux-Store 的核心形态（展示并一键安装其他软件）正对应这条禁令，且 Mac App Store 强制沙箱也和"调用 hdiutil/installer 安装外部二进制"这一核心能力冲突，双重不可行。
- Google Play 政策同样禁止"app 商店里卖/装别的 app"这类形态（Aptoide/APKPure/F-Droid 等同类第三方应用商店也均未上架 Play Store，是行业常态而非个例）。
- 因此 macOS 走**开发者签名 + 公证（notarization）+ 官网直接下载**；Android 走**侧载（sideload，用户需手动允许"安装未知来源应用"）**。这两步是自动化安全扫描，不是人工编辑审核，可控性更高。
- **Windows 和 Linux 不完全一样**：Genjux-Store 这个 app 本身仍可以上架 **winget**（微软官方包管理器，开放 PR 提交，非编辑审核制）和 **Flathub/Snap Store/AUR**（Linux，同样开放提交），间接获得一条"官方渠道"曝光，不算完全没有官方分发面。
- 推广不能单靠自付费 marketing，更现实的低成本增长杠杆：开发者社区口碑（Hacker News/V2EX/少数派/Product Hunt 首发）、被安装项目维护者在其 README 挂"一键用 Genjux-Store 安装"徽章形成的病毒式传播、以及上面提到的 winget/Flathub 收录。

## Proposed approach

### 1. 分层架构 & 核心技术选型（已与用户讨论确认）

**关键决策：core 不是"内嵌库"，而是本地服务（懒启动、空闲自动退出的进程）**，理由：GUI、CLI、AI agent 三种调用方需要看到同一份状态（安装进度、已装清单等），只有单一常驻 core 进程 + 大家都连它，才能保证一致；各端各自内嵌一份库做不到状态共享。

分层：
- **Core（本地服务进程，跨平台共享同一份代码）**：发现/推荐、Release 元数据解析与平台分类、下载管理（可续传）、校验（checksum/签名信号收集）、安装编排状态机、本地已装应用登记与更新检查。对外暴露两层接口：① **MCP server**（给 Claude/Copilot CLI 等 AI agent 直接调用工具，如 `search_software`/`install_software`/`list_installed`）② **本地 HTTP/JSON API**（通用程序化接口，也是桌面 GUI 与 CLI 的真正调用方式）。监听 localhost 端口或 Unix domain socket/Windows named pipe，仅限本机访问。
- **Platform Adapter（每平台一份，薄，跑在 core 进程内）**：调用 OS 原生安装 API/子进程（hdiutil/installer、msiexec、dpkg-rpm-AppImage）。
- **UI/CLI（每端瘦客户端）**：GUI 和 CLI 都只是本地 HTTP API 的客户端，不直接持有业务逻辑。

**推荐技术选型：Rust**（tokio 异步运行时 + axum 提供 HTTP/JSON API + rmcp 官方 Rust MCP SDK 提供 MCP server；与你 Nautilus-HN 的 Rust 技术栈一致，可复用经验）：
- 无 GC 也内存安全——对"解析不可信下载数据"（GitHub API 响应、解压未知来源的 zip/tar 包）这类场景是经典攻击面，Rust 明显降低风险，这个优势在服务化架构下依然成立。
- reqwest/rustls/zip/tar/sha2 生态成熟；rmcp（modelcontextprotocol 官方 Rust SDK）+ axum 组合社区案例较多。
- **意外的好处**：因为桌面 GUI 现在走 HTTP 调用而非 FFI，之前担心的"UniFFI 在 Windows 上 C# 绑定不成熟"这个风险直接消失——桌面三端都不需要 UniFFI 了，只有 Android 还需要（见下）。

备选方案对比（讨论过程记录）：
| 方案 | 优点 | 缺点 |
|---|---|---|
| Rust（推荐） | 内存安全、生态成熟(reqwest/zip/tar/sha2)、rmcp 官方 MCP SDK、与你已有技术栈一致 | 团队若不止你一人，Rust 招人/协作门槛比 C++/C#/Kotlin 高 |
| Node.js/TypeScript | MCP 协议发源地，官方 TS SDK 生态最成熟；HTTP 框架(hono/express)选择多 | 解析不可信二进制数据时内存安全弱于 Rust；打包成独立原生二进制不如 Rust 直接 |
| Go | 语言简单、编译快、交叉编译方便 | MCP/Rust 生态成熟度不如 rmcp；GC 语言，内存安全优势不如 Rust 明显但也不差 |
| C++ | 全平台最成熟、性能最好 | 解析不可信压缩包/网络数据是 C++ 内存安全事故高发区，且无自动绑定生成 |

**用户已确认：继续用 Rust**（rmcp + axum，内存安全优势仍是决定性因素）。

### 2. 各平台 UI/CLI 技术选型 & 与 core 的通信方式

除 Android 外，桌面三端和 CLI 都**不再需要 UniFFI/FFI**，统一走本地 HTTP/JSON API：

| 平台/端 | UI 技术 | 与 core 通信方式 | 备注 |
|---|---|---|---|
| macOS | AppKit，纯命令式风格（不用 SwiftUI） | 本地 HTTP API（URLSession） | 与你在其他项目一贯坚持的 UIKit/AppKit-only 约定保持一致 |
| Windows | WinUI 3 (Windows App SDK) + C#/.NET | 本地 HTTP API（HttpClient） | 不再需要 UniFFI C# 绑定，风险已消除 |
| Linux | GTK4 + libadwaita | 本地 HTTP API | 也可选 Qt/QML 以兼容更多桌面环境；因为是走 HTTP，UI 层甚至不必和 core 同语言 |
| CLI（配套命令行，类似 `brew install`） | 单一跨平台二进制 | 本地 HTTP API（首次调用时懒启动 core 服务，若已运行则直接连） | 和 GUI 共享同一 core 服务与状态 |
| Android | Kotlin + Jetpack Compose | **例外**：UniFFI Kotlin 绑定，core 编译成库直接内嵌进 app 进程 | 手机没有"本地常驻服务+localhost API"的惯用法（后台执行/电量限制），daemon 模式不适用；UniFFI 官方 Kotlin 绑定成熟，问题不大 |

AI agent（Claude/Copilot CLI 等）通过同一 core 服务的 **MCP server** 接口接入，不需要额外适配层。

### 3. Release 产物识别与元数据方案

**多源架构决策（已确认）**：core 从 Phase 0 起就设计成**可插拔多源**（`SourceProvider` trait + provider-agnostic 的 `RepoRef` 类型），GitHub 是第一个（也是 Phase 0 唯一的）具体实现，接口预留给未来接入 Gitee、GitLab、Codeberg（Forgejo）、GitCode、AtomGit 等平台。原因：用户群体是中国开发者，Gitee/GitCode 等国内平台的项目同样是潜在推荐来源，若一开始就把分类/下载/缓存管道硬编码在 GitHub API 类型上，后续接入会是一次痛苦的重构；现在做成 trait 边界，成本很低。注意：**尚未验证 Gitee/GitCode/AtomGit 的 Release API 与 GitHub 的兼容程度**，这是留给后续 provider 实现者的公开问题，不是 Phase 0 阻塞项。追踪：[GitHub issue #28](https://github.com/PetrGuan/Genjux-Store/issues/28)。

分类管道（逐级兜底，运行在 provider-agnostic 的 `RepoRef`/asset 层之上，不假设来源是 GitHub）：
1. 扩展名映射表：.dmg/.pkg/.app.zip → macOS；.exe/.msi/.appx → Windows；.AppImage/.deb/.rpm → Linux；.apk → Android
2. 文件名关键词 + 架构 token 匹配（darwin/mac/osx, win/windows, linux, android；x86_64/amd64, arm64/aarch64）
3. 内容嗅探兜底：文件名模糊时（如泛用 .zip/.tar.gz），下载 header/解包首个 entry 检查 Mach-O/PE/ELF magic bytes 再决定是否继续
4. 策展人元数据覆盖层：为热门/规则匹配失败的项目维护一份类似 Homebrew Formula / Scoop manifest 的内部清单（`genjux.yaml` 约定），人工补充平台映射与特殊安装步骤
5. 按 release tag 缓存解析结果，避免重复请求 GitHub API（注意 rate limit，需要 GitHub App/Token 策略）

内部标准化 schema：`InstallablePackage { platform, arch, kind, sha256?, min_os_version?, silent_install_args? }`

### 4. 安装编排设计

统一状态机：resolve → download（core，断点续传）→ verify（有官方 checksum 则比对；无则计算并展示 hash 供用户核实，不能凭此断言"安全"）→ platform install。

各平台差异（这些是 OS 级强约束，不是可选设计，必须遵守，不能绕过）：
- **macOS**：挂载 dmg（hdiutil）或调用 `installer` 执行 pkg（需管理员权限）；Gatekeeper 会对未签名/未公证应用弹出警告——应展示而非试图绕过，这本身是对用户有益的信任信号。
- **Windows**：子进程运行 exe/msiexec /i；尊重 UAC 提权弹窗；SmartScreen 对声誉未知的二进制会拦截提示，同样应展示而非抑制。
- **Linux**：按包类型分发（`pkexec dpkg -i` / rpm 对应工具 / AppImage 直接 chmod+x 放入 `~/.local/bin` 并可选生成 desktop entry），尽量避免要求 root。
- **Android**：无 root 无法静默装 APK，必须走 `PackageInstaller`/`ACTION_VIEW` intent，用户需先在系统里允许"安装未知应用"来源——这是硬性系统限制，需在 UX 里作为预期步骤呈现，而非 bug。

"沙箱试跑"评估：对任意未签名第三方二进制做真正的跨平台执行沙箱在 MVP 阶段不现实（尤其 macOS App Sandbox 对第三方任意二进制几乎不可行）。建议 MVP 阶段**去掉**"安装前沙箱试跑"，替换为轻量信誉展示（star 数、发布者是否 verified org、发布时间、hash 比对结果、可选 VirusTotal hash 查询链接）。

### 5. 安全与信任模型

- Genjux-Store 定位为"发现 + 安装编排层"，绝不代表"这个二进制是安全的"，只呈现客观信号（签名/公证状态、hash 校验结果、发布者信誉、star 数、发布时间）。
- 安装前必须有明确的用户确认页（展示将要运行的文件、来源 URL、以上信任信号），不允许无确认自动执行。
- 不绕过/不静默通过任何 OS 安全提示（Gatekeeper/UAC/SmartScreen/Android 未知来源）——这既是法律/伦理必要项，也直接降低自身责任。
- 记录安装来源与时间的本地审计日志，便于用户事后排查问题。

### 6. MVP 范围建议与路线图

Phase 0：先做 core 服务（Rust，HTTP API + MCP server）+ CLI 客户端（本身也是产品面之一，不只是内部 harness），验证发现/分类/下载/校验逻辑，是迭代最快最省成本的起点，同时 CLI 早期即可用。
Phase 1：待 Phase 0 完成后再决定旗舰桌面平台（macOS 或 Windows——面向开发者/极客用户 macOS 更合适，面向"发现好用软件"的泛用户 Windows 盘子更大），做完整 GUI（走本地 HTTP API 调 core）。
Phase 2：第二个桌面平台（core 可复用 ~90%）。
Phase 3：Linux（若采用 gtk-rs 直接在 core 语言内写 UI，是成本最低的一个平台）。
Phase 4：Android（安装信任模型差异最大，UX 也最不同，建议放最后）。

### 7. 商业模式（已与用户讨论确认）

**方向：个人版 freemium + 打赏，先做简单**（与你在 Nautilus-HN 的打赏模式一致，可复用产品哲学与部分实现经验）。

- **免费**：核心能力全部免费——搜索/推荐、分类识别、下载、校验、一键安装、CLI、MCP/HTTP API 基础调用额度。不因为付费与否影响"信任信号"的展示完整性（呼应第 5 节：不能把安全相关信息做成付费墙后面的东西）。
- **付费解锁（一次性打赏或订阅，MVP 阶段先选一种，建议先做一次性打赏，最简单）**：非安全相关的效率/便利功能，例如：
  - 批量安装/批量更新全部已装软件
  - 自定义/更聪明的推荐（如按你的已装软件历史做个性化推荐）
  - 多设备同步已装应用清单
  - 使用你代管的 GitHub token 池以避免用户自己遇到 API rate limit（对重度用户是真实痛点）
- **暂不做**（本阶段明确排除，避免与已定信任模型冲突或分散精力）：维护者付费推荐位/置顶（与第 5 节信任模型冲突，若未来要做必须显著标注"赞助"且不能替代客观信号）、企业/团队版（需要账号/组织/license 体系，是更大的架构决策，本阶段不纳入 Phase 0 范围）。
- **架构影响**：这个决策对 Phase 0 影响很小——不需要账号系统，可能只需要一个轻量的"本地一次性解锁 token/收据校验"机制（类似 Nautilus-HN 的 StoreKit 非消耗型解锁），具体支付渠道（Stripe/各平台原生 IAP 等，注意直接分发不再有 App Store IAP 可用）留到对应平台开发阶段再定。

### 8. 竞品差异化定位

- Homebrew/Winget/Scoop/apt 等都要求维护者主动提交/维护到该包管理器的目录（有审核/PR 流程），大量长尾 GitHub 项目从未被收录。Genjux-Store 的差异化：**直接从 GitHub Release 自动摄取，无需维护者任何额外动作**。
- Setapp：策展付费 mac 软件包，非开源导向，非自动发现。
- F-Droid：需要可复现构建提交流程，非"任意 GitHub Release 即可安装"。
- 需要明确决策的政策问题：对未经维护者同意就自动收录/代理安装其 Release 的项目，采用 opt-out 还是 opt-in 策略？（鉴于你此前在 NGA 项目上因"平台明确禁止第三方客户端"而被律师警示，这里应提前查证 GitHub 服务条款是否允许你正在做的这种"读取公开 Release 并代理下载安装"的行为模式——目前认知里这与 NGA 情况不同，GitHub Release 本身就是维护者主动公开发布给他人下载使用的产物，但仍建议在动手前书面确认一次。）

## Files likely involved

尚无代码；后续 Phase 0 预计新增 `core/`（Rust 本地服务：axum HTTP API + rmcp MCP server + 业务逻辑）与 `cli/`（Rust CLI 客户端），之后按 Phase 逐步新增 `macos/`、`windows/`、`linux/`（均为 core 本地 HTTP API 的瘦客户端）、`android/`（例外，UniFFI 内嵌 core 库）平台工程目录。

## Implementation steps

本阶段仅做架构规划，不进入实现。待用户确认以下开放问题后，可进入 Phase 0（core crate 骨架）。

## Validation

本阶段无代码可验证。Phase 0 完成后的验证标准：core crate 能对一组测试 GitHub 仓库正确分类 Release assets、下载并校验 checksum，CLI harness 跑通端到端流程。

## Risks and open questions

1. ~~平台 UI 选型确认~~：已确认，macOS 用 AppKit 纯命令式风格（不用 SwiftUI）。
2. **首发平台**：暂不决定，待 Phase 0 core crate（发现/分类/下载/校验）跑通后，再评估选 macOS 还是 Windows 做 Phase 1 旗舰 UI。
3. **GitHub ToS/rate limit**：需要书面确认自动摄取公开 Release 是否合规，以及 GitHub API 配额策略（App token vs PAT）。
4. **Opt-in/opt-out 策略**：是否需要给维护者一个"要求下架/排除我的项目"的机制。
5. ~~Windows UniFFI 绑定成熟度~~：已不适用——桌面端改走本地 HTTP API，不再需要 UniFFI/FFI 绑定。
6. **沙箱试跑**：确认 MVP 阶段去掉真实沙箱执行，只做信誉展示，是否符合预期。
7. **本地服务安全边界（新增）**：localhost HTTP API 能触发安装动作，需防范同机其他进程/浏览器 DNS rebinding 等方式冒充合法客户端调用；建议本地 token 鉴权 + 仅绑定 loopback/命名管道，具体鉴权方案待 Phase 0 设计。
8. **core 服务生命周期（新增）**：懒启动由谁触发（GUI 启动时拉起？CLI 首次调用时拉起？）、空闲多久自动退出、如何避免和已运行实例冲突，需要在 Phase 0 明确设计。
9. **MCP server 工具集设计（新增）**：`search_software`/`install_software`/`list_installed` 等具体工具的输入输出 schema、以及是否需要用户在 GUI 侧显式授权"允许 AI agent 代为安装"这类高风险操作，需要单独设计确认环节。
10. ~~分发渠道~~：已确认，不上架 Mac App Store / Google Play，走公证直接分发（macOS）+ 侧载（Android）；Genjux-Store 自身可考虑上架 winget/Flathub/Snap/AUR 作为 Windows/Linux 的补充官方渠道。
11. ~~商业模式~~：已确认，个人版 freemium + 打赏为主，MVP 先做一次性打赏解锁效率类功能（批量安装/更新、个性化推荐、多设备同步、token 池），不做维护者付费推荐位、暂不做企业版。支付渠道细节留到对应平台开发阶段确定。
12. ~~多源架构~~：已确认，Phase 0 设计 `SourceProvider` 可插拔抽象（GitHub 优先实现，接口预留 Gitee/GitLab/Codeberg/GitCode/AtomGit）。**新增开放问题**：Gitee/GitCode/AtomGit 的 Release API 与 GitHub 的兼容/差异程度尚未实测，留给未来实现对应 provider 时验证；届时第 3 点的"GitHub ToS"问题也需要对应扩展为"各来源平台的服务条款"逐一确认。

