# MiMo ASR Assistant

本地音频转文字工具，支持 AI 规整、多文件并发处理。

## 已实现功能

- **音频转文字**：支持音频格式 WAV、MP3、M4A、FLAC、OGG，以及常见视频容器的音轨提取（MP4、MKV、MOV、AVI、FLV、TS），通过 MiMo ASR API 流式转写
- **本地转码（音视频通吃）**：捆绑精简版 ffmpeg.exe 本地转码（提取音轨 → 16kHz/mono/16bit WAV），取代此前仅有限支持部分编码的纯 Rust 自实现（symphonia/rubato），常见音频与视频容器统一可解；转码过程不上传任何数据
- **AI 规整**：转写结果可一键 AI 规整（修正错别字、补充标点、智能分段），规整中可随时停止
- **多文件并发**：多个音频文件可同时处理，独立进度追踪，文件切换无串扰
- **速率控制**：内置并发信号量 + RPM 滑动窗口，自动排队避免 API 限流
- **分片重试**：每个音频分片独立重试 3 次，失败写入日志
- **导出**：支持导出原文 / 规整文本，自定义保存路径
- **系统托盘**：关闭窗口最小化到托盘，托盘图标点击恢复
- **配置加密**：API Key 使用 Windows DPAPI 加密存储
- **深色/亮色主题**：跟随系统或手动切换
- **流式标签过滤**：SSE 流自动过滤 ` thinking` 思考块和 XML 标签，支持跨 chunk 不完整标签缓冲
- **Markdown 实时渲染**：转写/规整结果实时按内容渲染 Markdown（标题 `#`、加粗 `**`、列表、表格、代码块等），流式输出时 `# 标题` 换行后即渲染为标题，无需等待任务完成；纯文本内容仍走转义快速路径
- **提示词管理**：设置页支持提示词的新增、编辑、删除、上移/下移排序，持久化到 `prompts.json`，重启后保留
- **滚动智能跟踪**：输出时自动跟随底部；向上滚动查看先前内容后，点击「跳转到底部」恢复跟踪，滞回设计避免按钮在滑动时闪烁

## 平台支持（清单 I-03：策略 A）

**运行与发布仅支持 Windows 10/11 x64**；CI 只承诺 Windows。

| 平台 | 状态 | 说明 |
|------|------|------|
| Windows 10/11 x64 | ✅ 支持 | 开发、CI、发布（NSIS 安装包）唯一目标平台 |
| Linux / macOS | ❌ 不支持 | 编译期 `compile_error!` 给出明确平台不支持提示（DPAPI 与捆绑 ffmpeg.exe 均为 Windows 专属），不承诺可编译/可运行 |

捆绑的 `ffmpeg.exe` 为 Windows 可执行文件，仅随 Windows 安装包分发；非 Windows 源码保持合理 `cfg` 隔离但不承诺完整应用编译。

## 支持的服务商

| 服务商 | 转写（ASR） | 规整（Chat） | Base URL |
|--------|------------|-------------|----------|
| MiMo API | `mimo-v2.5-asr` | `mimo-v2.5`、`mimo-v2.5-pro` | `https://api.xiaomimimo.com/v1` |
| MiMo Token Plan | `mimo-v2.5-asr` | `mimo-v2.5`、`mimo-v2.5-pro` | `https://token-plan-cn.xiaomimimo.com/v1` |
| DeepSeek | — | `deepseek-v4-pro`、`deepseek-v4-flash` | `https://api.deepseek.com` |

## 技术栈

| 层 | 技术 |
|---|------|
| 前端 | Vue 3 + TypeScript + Vite + Pinia |
| 桌面框架 | Tauri v2 |
| 后端 | Rust (tokio + reqwest) |
| 音频处理 | 捆绑精简版 ffmpeg.exe 本地转码（FFmpeg 9.0 `--disable-everything` 精简构建，无网络能力）+ 纯 Rust WAV 切片/解析（hound 仅慢速路径） |

ffmpeg.exe 的供应链记录（来源、版本、SHA-256 完整性门禁与更新流程）见 [SECURITY.md](SECURITY.md)。

## 本地编译

### 前置依赖

- [Rust](https://rustup.rs/) (1.96+；工具链 channel/components 由仓库根 `rust-toolchain.toml` 固定，与 CI 一致)
- [Node.js](https://nodejs.org/) (24+；CI 固定 Node 24 主版本)
- Windows WebView2 Runtime

### 编译命令

```bash
# 安装前端依赖
npm install

# 开发模式（前端热更新 + Rust 后端）
npm run tauri dev

# 构建生产版本（生成 exe + 安装包）
# 可选：CI 已能产出同样的安装包（见「CI」的 package job），
# 本地不必保留 target/ 这类重产物，需要时从 CI Artifacts 下载
npm run tauri build

# 仅检查 Rust 语法
cd src-tauri && cargo check
```

本地构建产物（均可随时删除、按需重建，且已在 `.gitignore` 中）：
- `src-tauri/target/`（体积最大，Rust 编译中间产物与 release 产物）
- `src-tauri/target/release/mimo-asr-assistant.exe`
- `src-tauri/target/release/bundle/nsis/`（NSIS 安装包，内含 resources/ffmpeg.exe）
- `dist/`（前端构建输出）

发布用的安装包由 CI `package` job 产出并发布到 Releases（main 构建覆盖更新 `latest` pre-release），
因此本地只需保留 `node_modules/`（开发/门禁命令依赖），
其余构建输出可以删除；用 `npm run tauri dev` 时 Rust 会按需增量重编。

### 质量门禁（清单 Q-00）

合入/发版前必须全部通过（0 warning / 0 error）：

```bash
# 前端
npm run typecheck      # vue-tsc --noEmit 类型检查
npm run lint           # ESLint 检查
npm run test           # vitest 单元测试
npm run build          # 类型检查 + vite build（跨 shell 兼容，见 scripts/build.mjs）
npm run version:check  # 核对 package.json / tauri.conf.json / Cargo.toml 三处版本号

# Rust（在 src-tauri 目录执行）
cargo check            # 编译检查
cargo test             # 单元测试（DPAPI 相关测试依赖 Windows）
cargo clippy --all-targets -- -D warnings   # lint 零容忍（含测试目标）

# Tauri 开发冒烟
npm run tauri dev
```

`npm run tauri build` 的前置构建已指向 `npm run build`（tauri.conf.json `beforeBuildCommand`），
即先执行 `vue-tsc --noEmit` 再 `vite build`：TypeScript 类型错误会在进入 Rust 打包前阻断发布。

### CI（清单 I-01）

上述门禁由 `.github/workflows/ci.yml` 在 Windows runner 上自动执行（push 默认分支 / PR 触发，三 job 并行）：

| Job | 内容 |
|-----|------|
| `frontend` | npm ci → typecheck → lint → vitest → build → version:check |
| `rust` | ffmpeg.exe SHA-256 基准比对 → cargo check/test/clippy --all-targets（均 `--locked`；build.rs 门禁编译期再校验一次哈希） |
| `security` | `npm audit --omit=dev --audit-level=high` + 固定版本 `cargo audit --deny warnings`（已登记例外见 `src-tauri/.cargo/audit.toml` 与 SECURITY.md） |
| `package` | main 推送 / tag / 手动 dispatch 时运行：`tauri build --bundles nsis` → 静默安装后逐项核对 `bundle.resources` 声明的静态资产与 ffmpeg 哈希 → 上传安装包 artifact，并发布 release（见下）。只产出 NSIS 安装包 |

**取安装包**（本地不再需要 `src-tauri/target/`）：

方式一，Releases 页面直接下载（推荐，资产是原始 .exe 而不是 zip）：
- **滚动 pre-release**：每次 main 构建都会覆盖发布固定 tag [`latest`](https://github.com/Bronzesakon/MiMo-ASR-Assistant/releases/tag/latest) 的 pre-release——资产用 `--clobber` 覆盖、tag 强制移动到本次 commit、标题与说明刷新，所以页面上永远是 main 最新一次构建的产物。标题与说明里带**精确到秒的 UTC+8 构建开始时间**（另附本次 commit 的提交时间），便于确认拿到的是哪一次构建。
- **正式 release**：推 tag 会发布该 tag 的正式 release 并自动生成 release notes。`v1.2.0` 与仓库历史沿用的裸版本号 `1.2.0` 两种写法都接受；tag 触发时会校验 tag 与 `package.json` 版本一致，不一致直接失败。

方式二，该次运行的页面底部 **Artifacts** 下载 `mimo-asr-assistant-<ref>`（保留 30 天，只含 NSIS 安装包），适合需要保留某次具体构建的场景。

说明：
- 资产名去掉了空格与版本号（`mimo-asr-assistant-setup.exe`）：GitHub 会把资产名里的空格重写成点，导致下载 URL 不可预期。
- 连续推送时，`concurrency` 会取消仍在跑的旧运行，因此被取消的那次不会发布 release，属预期行为。
- CI 不运行需要真实 API Key 的在线转写；真实服务端到端冒烟保留为发布前**人工步骤**（见「发布检查清单」），CI 不宣称覆盖桌面 UI 冒烟。
- 分支保护建议：要求 `frontend`、`rust`、`security` 全绿后才允许合并——**需由仓库管理员在 GitHub 仓库设置中配置**（当前无仓库管理权限，未代为声称已完成）。
- CI badge 待仓库公开且 workflow 稳定后补充。

## 隐私与数据流（清单 D-01）

- **音频转码全程本地**：导入的音频/视频由捆绑的精简 ffmpeg.exe 在本机转码、切片为 16kHz/mono WAV，ffmpeg 无网络能力，不上传任何数据。
- **发送到 ASR 服务商**：切片音频以 Base64 编码随请求发送到**用户自行配置**的 ASR 服务商（见「支持的服务商」）。这是本应用唯一的音频出站路径。
- **发送到规整服务商**：AI 规整时，转写文本与所选提示词模板发送到**用户自行配置**的规整服务商。
- **API Key 存储**：经 Windows DPAPI 加密后写入配置（`ENC:` 前缀 + Base64 密文）。DPAPI 绑定当前 Windows 用户上下文——同一用户下的应用可解密，其他用户/机器不可。
- **日志**：
  - 位置：Tauri 标准 app_log_dir（Windows 下位于 `%APPDATA%\com.mimo.asr.assistant\logs`），可用应用内「打开日志目录」入口定位；删除方式即直接删除该目录下文件。
  - 可能包含：时间戳、级别、模块名、任务状态与错误信息、服务商名/模型名/HTTP 状态码、错误响应体的**安全截断**片段。
  - 明确不包含：API Key 明文、加密前/解密后的 Key、音频文件内容。发布代码不含调试正文日志。
- **临时切片**：
  - 位置：系统临时目录下应用专属根目录 `%TEMP%\mimo-asr-assistant\` 内的任务子目录（目录名含 pid + 纳秒 + 随机哈希，并写元数据文件甄别归属）。
  - 清理策略：文件移除/任务结束即清理对应目录；应用启动时按元数据清理上次会话遗留的孤儿目录。
  - 残余风险：进程被强制终止（断电/杀进程）时当次临时切片可能残留，将在下次启动清理；敏感场景可手动删除整个 `%TEMP%\mimo-asr-assistant\` 目录。

## 配置与日志位置

| 内容 | 位置（Windows） |
|------|----------------|
| 配置（config.json，Key 字段 DPAPI 加密） | `%APPDATA%\com.mimo.asr.assistant\` |
| 日志 | `%APPDATA%\com.mimo.asr.assistant\logs\` |
| 临时切片 | `%TEMP%\mimo-asr-assistant\` |

## 项目结构

```
├── src/                    # Vue 3 前端
│   ├── components/         # UI 组件
│   ├── stores/             # Pinia 状态管理
│   ├── types/              # TypeScript 类型
│   └── utils/              # 工具函数（Markdown 渲染/消毒、外链白名单）
├── src-tauri/              # Rust 后端
│   ├── resources/          # 捆绑精简 ffmpeg.exe + sha256 基准（入库，双重完整性门禁）
│   ├── src/
│   │   ├── lib.rs          # 应用入口（含非 Windows compile_error! 平台门禁）
│   │   ├── main.rs         # 入口
│   │   ├── api/            # API 客户端（流式 SSE + 标签过滤 + 重试）
│   │   ├── audio/          # 任务临时目录模型与切片调度
│   │   ├── wav_codec/      # 纯 Rust WAV 切片与解析（零解码依赖）
│   │   ├── ffmpeg_exe.rs   # 捆绑 ffmpeg.exe 封装（定位/完整性校验/转码/探测）
│   │   ├── prompt/         # 提示词模板
│   │   ├── provider/       # 服务商配置
│   │   ├── rate_limiter.rs # 速率控制
│   │   ├── dpapi.rs        # API Key 加密（Windows DPAPI，cfg(windows) 隔离）
│   │   └── log.rs          # 日志系统
│   └── build.rs            # ffmpeg.exe SHA-256 构建期门禁
└── SECURITY.md             # 漏洞报告渠道 + ffmpeg 供应链记录
```

## 版本发布

三处版本号（`package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`）必须保持一致，发版统一使用：

```bash
npm run tauri version <x.y.z>
```

发布前可运行 `npm run version:check` 核对。

### 发布检查清单（清单 D-01）

发版按顺序执行，全部通过才可发布：

1. **版本同步**：`npm run tauri version <x.y.z>` 后 `npm run version:check` 三处一致。
2. **锁文件**：`package-lock.json` 与 `src-tauri/Cargo.lock` 已提交且未漂移。
3. **CI 全绿**：`frontend` / `rust` / `security` 三 job 全部通过；tag/手动触发的 `package` job 亦通过；本地等价命令见「质量门禁」。
4. **依赖审计**：`npm audit --omit=dev` 无 high/critical；`cargo audit --deny warnings` 通过（例外已在 SECURITY.md 登记到期日，且仅限 `src-tauri/.cargo/audit.toml` 中的 ID）。
5. **release 构建（在 CI）**：推 tag 后 `package` job 成功，正式 release 上出现安装包；仅需 main 最新构建时，直接取 `latest` 滚动 pre-release 的资产即可。本地 `npm run tauri build` 为等价可选路径，非必需。
6. **安装包 ffmpeg 校验**：安装后确认安装目录下 `resources/ffmpeg.exe` 存在，且与 `src-tauri/resources/ffmpeg.exe.sha256` 哈希一致：
   ```powershell
   Get-FileHash <安装目录>\resources\ffmpeg.exe -Algorithm SHA256
   ```
   并确认应用启动日志出现「ffmpeg.exe 定位成功并通过完整性校验」。
7. **AV 扫描**：安装包与主 exe 通过 Windows Defender（及可用时第二引擎）扫描，误报需记录处理结论。
8. **完整冒烟**（`npm run tauri dev` 或安装版）：导入音频与视频容器 → 转写 → 暂停/继续/取消 → 选择提示词规整 → 停止规整 → 覆盖确认/取消规整 → 提示词新增/编辑/删除/排序/重启持久化 → 导出 txt/md → 设置读写 → 自定义服务商模型获取 → 打开日志/提示词目录。
9. **CSP 控制台**：DevTools Console 无 CSP 违规告警（tauri.conf.json `app.security.csp` 刚收紧过；任何 CSP/capability 变更必须在发布说明中说明影响面并重跑外链/渲染回归）。
10. **安装/卸载**：NSIS 安装、升级覆盖、卸载后无残留异常（配置目录按预期保留）。
11. **配置迁移**：旧版配置（含 `ENC:` Key）升级后可正常解密读取。
12. **日志脱敏**：抽查本轮日志无转写正文、无 Key 明文。
13. **临时目录清理**：确认 `%TEMP%\mimo-asr-assistant\` 任务结束后清空、重启后孤儿目录被清理。
14. **发布说明逐条核验**：每条声明必须能从当前代码/diff 反证（人工逐条核对，禁止沿用历史文案）；不伪造性能百分比，无基准数据时用定性描述并注明测试条件。
15. **外链白名单变更（如涉及）**：必须**同时**修改 `src/utils/externalLink.ts` 的 `ALLOWED_HOSTS` 与 `src-tauri/capabilities/default.json` 的 opener scope 两处，缺一即拒绝合入。

> 仓库卫生建议（清单 G-01）：后续版本的发布说明统一收纳进 `release说明文档/` 目录（历史 `RELEASE_v*.md` 已移入；该目录已在 `.gitignore` 中，不追踪）。

## 安全与漏洞报告

见 [SECURITY.md](SECURITY.md)。请勿在公开 issue 提交安全漏洞或附带 API Key / 音频 / 日志原文。

## 致谢

- [MiMo-Code](https://github.com/XiaomiMiMo/MiMo-Code) — MiMo API 参考实现
- [Joyi-code/DeepSeekMonitorWindows](https://github.com/KerryChia/DeepSeek_Monitor_for_Windows) — 系统托盘与窗口管理参考
- [KerryChia/DeepSeek_Monitor_for_Windows](https://github.com/Joyi-code/DeepSeekMonitorWindows/commit/ce26946cfc09dfd08a91367bf42578fe02dc0445) — 托盘点击首次唤出窗口修复
- [Marked](https://github.com/markedjs/marked) — Markdown 渲染（MIT License）
