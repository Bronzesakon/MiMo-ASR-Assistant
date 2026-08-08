export interface FileInfo {
  id: string
  name: string
  path: string
  duration: number
  size: number
  status: 'waiting' | 'processing' | 'transcribing' | 'pausing' | 'paused' | 'polishing' | 'completed' | 'failed' | 'cancelling' | 'cancelled'
  progress: number
  transcription: string
  polished: string
  transcriptionWordCount: number
  polishedWordCount: number
  polishStage: 'idle' | 'sending' | 'receiving' | 'failed'
  error?: string
  /** 兼容保留：后端不再作为授权依据，仅用于展示/回退 */
  tempDir?: string
  lastCompletedIndex?: number
  resumeTotal?: number
  /** 当前运行标识（前端生成，每次启动转写/规整/恢复生成） */
  runId?: string
  /** 后端返回的 generation（u64） */
  runGeneration?: number
  /** ffmpeg 转码/切片阶段（来自 process-status 的 stage，仅转写） */
  stage?: string
  language?: string
}

// ============================================================
// 统一任务事件信封（与后端 TaskEvent 对齐，camelCase 序列化）
// ============================================================

export type TaskOperation = 'transcription' | 'polish'

export type TaskEventKind =
  | 'started'
  | 'stage'
  | 'progress'
  | 'chunk'
  | 'paused'
  | 'completed'
  | 'cancelled'
  | 'failed'

/** 统一 TaskEvent 信封 payload */
export interface TaskEvent<T = Record<string, unknown>> {
  schemaVersion: number
  fileId: string
  operation: TaskOperation
  runId: string
  generation: number
  sequence: number
  kind: TaskEventKind
  payload: T
}

/** stage 事件 payload：{ stage } */
export interface TaskStagePayload {
  stage: string
}

/** progress 事件 payload：{ current, total } */
export interface TaskProgressPayload {
  current: number
  total: number
}

/** chunk 事件 payload：{ text } */
export interface TaskChunkPayload {
  text: string
}

/** paused 事件 payload：PauseCheckpoint { lastCompleted, totalSlices } */
export interface TaskPausedPayload {
  lastCompleted: number
  totalSlices: number
}

/** failed 事件 payload：{ error } */
export interface TaskErrorPayload {
  error: string
}

export interface ApiConfig {
  provider: string
  baseUrl: string
  apiKey: string
  transcriptionModel: string
  polishModel: string
  providerKeys: Record<string, string>
  loggingEnabled: boolean
  asrProvider: string
  polishProvider: string
  sliceSeconds: number
  customProviders: CustomProvider[]
}

// ============================================================
// 提示词分条管理（清单 F-06，与后端 prompt_store snake_case 一一对应）
// ============================================================

export interface PromptEntry {
  id: string
  title: string
  content: string
  is_default: boolean
}

export interface PromptDocument {
  schema_version: number
  selected_prompt_id: string
  prompts: PromptEntry[]
}

/** 规整发起时复制的提示词快照，请求期间不受后续编辑影响 */
export interface PromptSnapshot {
  id: string
  title: string
  content: string
}

/** 设置页编辑草稿；id 为 null 表示尚未落盘的新增草稿 */
export interface PromptDraft {
  id: string | null
  title: string
  content: string
}

export interface ProviderInfo {
  id: string
  name: string
  baseUrl: string
  keyPrefix: string
}

export interface CustomProvider {
  id: string
  name: string
  baseUrl: string
  apiKey: string
  models: string[]
}

export interface CustomModelOption {
  type: 'builtin' | 'custom'
  providerId: string
  modelId: string
  label: string
  customProviderId?: string
}

// ============================================================
// 后端事件 payload 类型（与 src-tauri 中 serde_json::json! 输出一一对应）
// ============================================================

/** 所有携带 file_id 的事件的公共结构 */
export interface FileIdEvent {
  file_id: string
}

/** transcription-chunk / chat-chunk：TaggedEmitter 包装 { file_id, text } */
export interface TranscriptionChunkEvent extends FileIdEvent {
  text: string
}

export interface ChatChunkEvent extends FileIdEvent {
  text: string
}

/** transcription-progress：分片进度 */
export interface TranscriptionProgressEvent extends FileIdEvent {
  current: number
  total: number
}

/** transcription-complete / chat-complete：仅携带 file_id */
export type TranscriptionCompleteEvent = FileIdEvent

export type ChatCompleteEvent = FileIdEvent

/** transcription-error：转写/规整失败 */
export interface TranscriptionErrorEvent extends FileIdEvent {
  error: string
}

/** transcription-paused：暂停时携带断点续传信息 */
export interface TranscriptionPausedEvent extends FileIdEvent {
  last_completed: number
  total: number
  temp_dir: string
}

/** token-update：TaggedEmitter 包装，text 为 token 数字字符串 */
export interface TokenUpdateEvent extends FileIdEvent {
  text: string
}

/** prompts-recovery-notice：提示词恢复/重建一次性提示（H-01 结构化 payload） */
export interface PromptsRecoveryNoticeEvent {
  message: string
}

/** process-status：ffmpeg 转码/切片阶段提示 */
export interface ProcessStatusEvent extends FileIdEvent {
  stage: string
}

// ============================================================
// 配置快照（P3，与后端 ConfigSnapshot 对齐：revision/notice/secretIssues camelCase，
// 扁平 config 字段沿用 AppConfig snake_case，与既有前端字段对齐）
// ============================================================
export interface ConfigSnapshot {
  revision: number
  provider: string
  base_url: string
  api_key: string
  transcription_model: string
  polish_model: string
  theme: string
  provider_keys: Record<string, string>
  logging_enabled: boolean
  asr_provider: string
  polish_provider: string
  slice_seconds: number
  custom_providers: CustomProvider[]
  notice: string | null
  secret_issues: Array<{ path: string; reason: string }>
}

/** 配置命令错误（结构化返回，前端据此识别 REVISION_CONFLICT） */
export interface ConfigCommandError {
  code: string
  message: string
  currentRevision?: number
}

// ============================================================
// 能力文档（P8，与后端 AppCapabilities camelCase 对齐）
// ============================================================
export interface ProviderCapability {
  id: string
  name: string
  baseUrl: string
  keyPrefix: string
  asrModels: string[]
  chatModels: string[]
}

export interface PromptLimits {
  maxItems: number
  titleMaxChars: number
  contentMaxChars: number
}

export interface ImportCapabilities {
  extensions: string[]
  maxBatch: number
  maxFiles: number
  probeConcurrency: number
}

export interface ExportCapabilities {
  formats: string[]
  maxBytes: number
}

export interface AppCapabilities {
  schemaVersion: number
  providers: ProviderCapability[]
  promptLimits: PromptLimits
  sliceSeconds: number[]
  import: ImportCapabilities
  export: ExportCapabilities
}

/** 导入动作返回码（P5）：仅 added 时选择新文件 */
export type AddFileResult = 'added' | 'duplicate' | 'limitReached'
