# 安全政策（SECURITY）

## 支持版本

| 版本 | 状态 |
|------|------|
| 最新 1.1.x | 支持（接收安全修复） |
| 更早版本 | 不支持，请升级 |

## 漏洞报告方式

- 请使用 **GitHub Security Advisory 私密报告**（仓库 → Security → Advisories → Report a vulnerability）私下提交漏洞。
- **不要**在公开 issue / discussion 中报告安全漏洞。
- 报告中**不要附带**：真实 API Key、用户音频文件、转写正文、日志原文。如需示例，请使用脱敏占位数据（如 `sk-REDACTED`）。
- 项目尚未建立专用安全邮箱；请勿假定任何邮箱地址。以上 GitHub 渠道为当前唯一官方入口。

## 预计响应流程

1. 收到报告后尽快确认接收（目标 7 天内，尽力而为，不构成 SLA）。
2. 评估影响面与复现条件，必要时与报告者私下沟通补充细节。
3. 修复并发布新版本后，按 GitHub 流程发布 Security Advisory 并向报告者致谢（除非报告者要求匿名）。

## 捆绑 ffmpeg.exe 供应链记录（清单 I-02/D-01）

| 项 | 内容 |
|----|------|
| 文件 | `src-tauri/resources/ffmpeg.exe`（已入库，受版本控制） |
| 版本 | FFmpeg 9.0 精简构建 |
| 构建配置 | `--disable-everything` 基础上仅启用所需 demuxer/decoder（常见音频与视频容器音轨提取），encoder/muxer 仅 `pcm_s16le` + `wav` |
| 能力裁剪 | 无网络协议（无 http/rtmp 等）、无 ffprobe、无滤镜链、无编码器生态；唯一用途：输入容器 → 提取首条音轨 → 16kHz / mono / 16bit WAV |
| 完整性基准 | `src-tauri/resources/ffmpeg.exe.sha256`（SHA-256，已入库） |

**双重完整性门禁**：

1. 构建期：`src-tauri/build.rs` 在每次编译时比对 exe 与 sha256 基准——exe 存在而基准缺失或哈希不匹配时**硬失败**（panic），干净 checkout 中 exe 与基准一并入库，正常构建必然通过。
2. 运行期：`src-tauri/src/ffmpeg_exe.rs` 在定位 exe 后再次计算 SHA-256 比对（运行时防篡改/防损坏）。

**sha256 更新流程**（更换 ffmpeg.exe 版本时）：

```powershell
# 1. 用新 exe 替换 src-tauri/resources/ffmpeg.exe
# 2. 重算哈希并更新基准文件
Get-FileHash src-tauri\resources\ffmpeg.exe -Algorithm SHA256
#    将输出的 Hash（64 位十六进制）写入 src-tauri/resources/ffmpeg.exe.sha256
# 3. build.rs 门禁会在下次构建强制校验；CI rust job 亦有显式哈希比对步骤
# 4. 提交 exe 与 .sha256 两个文件，二者必须同一提交内成对变更
```

**攻击面说明**：ffmpeg.exe 仅处理用户主动导入的本地文件；以参数数组方式启动（无 shell 解释）、stderr 限长抽干、独立超时与取消（进程树终止）；不发起任何网络连接。残余风险主要来自恶意构造的媒体文件触发解码器漏洞，故构建保持精简配置并及时跟进 FFmpeg 上游安全公告。

## 依赖审计现状（清单 I-02）

- `npm audit --omit=dev`：**0 漏洞**（PostCSS high 已于 2026-08 通过锁文件升级 8.5.15 → 8.5.25 修复，传递依赖，来自 vite / @vue/compiler-sfc）。
- `cargo audit --deny warnings`：已接入 CI security job，cargo-audit 固定为 **0.22.2**（原 0.21.2 已弃用：该版本无法解析 advisory-db 中新增的 CVSS 4.0 条目，`error loading advisory database: unsupported CVSS version: 4.0`，与漏洞无关却恒使 job 失败）。
- 2026-09-10 锁文件升级已消除全部可修复项：`anyhow` 1.0.102 → 1.0.104、`event-listener` 5.4.1 → 5.4.2、`h2` 0.4.15 → 0.4.19、`plist` 1.9.0 → 1.10.1（带动 `quick-xml` 0.39.4 → 0.42.0）。**当前 0 漏洞。**
- 其余 7 条 warning 均无可用修复，按 ID 精确豁免（清单见 `src-tauri/.cargo/audit.toml`），逐条登记如下。

### 已登记例外（到期需复查）

| Advisory | Crate / 版本 | 类型 | 豁免原因 | 上游链接 | 到期日期 | 负责人 |
|----------|--------------|------|----------|----------|----------|--------|
| RUSTSEC-2024-0429 | glib 0.18.5 | unsound | Linux GTK 链，Windows 目标不编译；修复需 gtk-rs 0.20，只能随 tauri 上游升级 | https://rustsec.org/advisories/RUSTSEC-2024-0429 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2024-0370 | proc-macro-error 1.0.4 | unmaintained | 经 glib-macros / gtk3-macros 进入 Linux GTK 链，Windows 目标不编译 | https://rustsec.org/advisories/RUSTSEC-2024-0370 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2025-0075 | unic-char-range 0.9.0 | unmaintained | 经 tauri-utils → urlpattern 0.3 引入，上游未发布替代版本 | https://rustsec.org/advisories/RUSTSEC-2025-0075 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2025-0080 | unic-common 0.9.0 | unmaintained | 同上（urlpattern 0.3 依赖链） | https://rustsec.org/advisories/RUSTSEC-2025-0080 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2025-0081 | unic-char-property 0.9.0 | unmaintained | 同上（urlpattern 0.3 依赖链） | https://rustsec.org/advisories/RUSTSEC-2025-0081 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2025-0098 | unic-ucd-version 0.9.0 | unmaintained | 同上（urlpattern 0.3 依赖链） | https://rustsec.org/advisories/RUSTSEC-2025-0098 | 2026-12-10 | @Bronzesakon |
| RUSTSEC-2025-0100 | unic-ucd-ident 0.9.0 | unmaintained | 同上（urlpattern 0.3 依赖链） | https://rustsec.org/advisories/RUSTSEC-2025-0100 | 2026-12-10 | @Bronzesakon |

**复查要求**：到期前重跑 `cargo audit --deny warnings`，若上游已发布修复（tauri 升级 gtk-rs / urlpattern 换依赖），则升级依赖并删除对应 ignore；否则重新评估风险并顺延到期日期。豁免仅限上表 ID，任何新增 advisory 仍会阻断 CI。

- moderate/low 级别暂缓项登记：（暂无）。

## 数据与隐私

详见 README「隐私与数据流」章节。要点：音频转码全程本地；仅 Base64 音频发往用户配置的 ASR 服务商、转写文本与提示词发往规整服务商；API Key 经 Windows DPAPI 加密落盘；日志不含转写正文与 Key 明文。
