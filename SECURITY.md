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
- `cargo audit --deny warnings`：已接入 CI security job（固定版本 cargo-audit 0.21.2）。当前无已记录的临时例外；若未来需要 ignore 单个 advisory，必须在此登记原因、上游链接、到期日期与负责人。
- moderate/low 级别暂缓项登记：（暂无）。

## 数据与隐私

详见 README「隐私与数据流」章节。要点：音频转码全程本地；仅 Base64 音频发往用户配置的 ASR 服务商、转写文本与提示词发往规整服务商；API Key 经 Windows DPAPI 加密落盘；日志不含转写正文与 Key 明文。
