# MiMo ASR Assistant

本地音频转文字工具，支持 AI 规整、多文件并发处理。

## 已实现功能

- **音频转文字**：支持 WAV、MP3、M4A、FLAC、AAC、OGG 格式，通过 MiMo ASR API 流式转写
- **AI 规整**：转写结果可一键 AI 规整（修正错别字、补充标点、智能分段）
- **多文件并发**：多个音频文件可同时处理，独立进度追踪，文件切换无串扰
- **速率控制**：内置并发信号量 + RPM 滑动窗口，自动排队避免 API 限流
- **分片重试**：每个音频分片独立重试 3 次，失败写入日志
- **导出**：支持导出原文 / 规整文本，自定义保存路径
- **系统托盘**：关闭窗口最小化到托盘，托盘图标点击恢复
- **配置加密**：API Key 使用 Windows DPAPI 加密存储
- **深色/亮色主题**：跟随系统或手动切换
- **流式标签过滤**：SSE 流自动过滤 `<think>` 思考块和 XML 标签，支持跨 chunk 不完整标签缓冲

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
| 音频处理 | 纯 Rust：symphonia（解码）+ hound（WAV 编码）+ rubato（重采样） |

## 本地编译

### 前置依赖

- [Rust](https://rustup.rs/) (1.96+)
- [Node.js](https://nodejs.org/) (24+)
- Windows WebView2 Runtime

### 编译命令

```bash
# 安装前端依赖
npm install

# 开发模式（前端热更新 + Rust 后端）
npm run tauri dev

# 构建生产版本（生成 exe + 安装包）
npm run tauri build

# 仅检查 Rust 语法
cd src-tauri && cargo check
```

构建产物：
- `src-tauri/target/release/mimo.exe`
- `src-tauri/target/release/bundle/nsis/` (NSIS 安装包)

## 项目结构

```
├── src/                    # Vue 3 前端
│   ├── components/         # UI 组件
│   ├── stores/             # Pinia 状态管理
│   └── types/              # TypeScript 类型
├── src-tauri/              # Rust 后端
│   ├── src/
│   │   ├── lib.rs          # Tauri 命令层
│   │   ├── api/            # API 客户端
│   │   ├── audio/          # 音频切分
│   │   ├── ffmpeg/         # 纯 Rust 音频处理（解码/转码/切片）
│   │   ├── prompt/         # 提示词模板
│   │   ├── provider/       # 服务商配置
│   │   ├── rate_limiter/   # 速率控制
│   │   ├── dpapi/          # API Key 加密
│   │   └── log/            # 日志系统
└── 项目文档.md              # 详细开发文档
```

## 致谢

- [MiMo-Code](https://github.com/XiaomiMiMo/MiMo-Code) — MiMo API 参考实现
- [Joyi-code/DeepSeekMonitorWindows](https://github.com/KerryChia/DeepSeek_Monitor_for_Windows) — 系统托盘与窗口管理参考
- [KerryChia/DeepSeek_Monitor_for_Windows](https://github.com/Joyi-code/DeepSeekMonitorWindows/commit/ce26946cfc09dfd08a91367bf42578fe02dc0445) — 托盘点击首次唤出窗口修复
