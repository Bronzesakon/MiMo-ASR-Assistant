//! 全局魔法常量集中区（清单 B-11）：切片钳制、channel 容量、限流、
//! SSE/ffmpeg 超时、窗口兜底等。各模块一律引用本模块常量，禁止散落字面量。

// ============================================================
// 切片时长（audio 分片）
// ============================================================

/// 切片时长上限（秒）：配置值与动态收缩均不得超过该值
pub const MAX_SLICE_SECONDS: u32 = 150;

/// 切片时长下限（秒）：动态收缩不会低于该值
pub const MIN_SLICE_SECONDS: u32 = 5;

/// 默认切片时长（秒）
pub const DEFAULT_SLICE_SECONDS: u32 = 10;

/// 切片时长合法值集合（P8-01：能力文档单一来源，前端下拉据此渲染）
pub const SLICE_SECONDS_OPTIONS: [u32; 4] = [10, 30, 60, 150];

/// 编码失败时切片时长收缩步长（秒）
pub const CHUNK_SHRINK_STEP: u32 = 10;

/// 单切片编码产物的内存上限（字节，约 9MB data URL）
pub const MAX_BASE64_BYTES: usize = 9 * 1024 * 1024;

// ============================================================
// 任务管道
// ============================================================

/// 切片生产（blocking）→ 转写消费（async）channel 容量。
/// 小容量背压即可：切片速度略快于 API 时生产侧自然阻塞，避免内存堆积。
pub const SLICE_CHANNEL_CAPACITY: usize = 12;

// ============================================================
// 限流（rate_limiter）
// ============================================================

/// ASR 并发上限（API 100 RPM 上限，留 10% 余量 → 90 RPM）
pub const ASR_MAX_CONCURRENT: usize = 8;
/// ASR 每分钟请求上限
pub const ASR_RPM_LIMIT: usize = 90;

/// Chat 并发上限（Pro 系列 100 RPM，留 10% 余量 → 90 RPM）
pub const CHAT_MAX_CONCURRENT: usize = 3;
/// Chat 每分钟请求上限
pub const CHAT_RPM_LIMIT: usize = 90;

// ============================================================
// SSE / API
// ============================================================

/// SSE 空闲超时（秒）：无新数据视为连接中断（api 层引用）
pub const SSE_IDLE_TIMEOUT_SECS: u64 = 90;

// ============================================================
// 捆绑 ffmpeg.exe（ffmpeg_exe）
// ============================================================

/// ffmpeg try_wait 轮询间隔（毫秒）
pub const FFMPEG_POLL_INTERVAL_MS: u64 = 150;

/// ffmpeg 时长探测独立超时（秒）
pub const FFMPEG_PROBE_TIMEOUT_SECS: u64 = 10;

/// ffmpeg 转码超时基础值（秒）：60s + 1s/MB 输入大小
pub const FFMPEG_CONVERT_TIMEOUT_BASE_SECS: u64 = 60;

/// ffmpeg 转码超时封顶（秒）
pub const FFMPEG_CONVERT_TIMEOUT_CAP_SECS: u64 = 600;

// ============================================================
// 窗口
// ============================================================

/// 主窗口显示兜底等待（秒）：超过该时长窗口仍不可见时强制显示
pub const WINDOW_FALLBACK_SECS: u64 = 5;

// ============================================================
// 能力文档（P8-01）：导入/导出业务限制，get_capabilities 单一来源
// ============================================================

/// 单次导入可选文件数的上限（批量导入批次大小）
pub const MAX_IMPORT_BATCH: usize = 100;

/// 文件列表探针返回项数上限
pub const MAX_FILE_LIST_ITEMS: usize = 500;

/// 文件列表并发探测（尺寸/时长）的并发数
pub const PROBE_CONCURRENCY: usize = 4;

/// 单次导出内容的最大字节数（16 MiB）
pub const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;

/// 允许导入的媒体扩展名（不含点号，P8-01 能力文档导出）
pub const IMPORT_EXTENSIONS: &[&str] = &[
    "wav", "mp3", "m4a", "flac", "ogg", "mp4", "mkv", "avi", "mov", "flv", "ts",
];

/// 允许导出的文本格式（P8-01 能力文档导出，与 fs::ExportFormat 对齐）
pub const EXPORT_FORMATS: &[&str] = &["txt", "md"];
