// 导入统一入口（P5）：工具栏导入与拖拽导入只负责取得路径，统一调用 importPaths。
// 媒体探测（get_audio_duration）并发受限、保持结果顺序、可取消；批次结束汇总提示。
// 单批上限取 capabilities.import.maxBatch，文件列表上限 maxFiles。
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useFilesStore } from '../stores/files'
import { useUiStore } from '../stores/ui'
import { useSettingsStore } from '../stores/settings'
import { useCapabilitiesStore } from '../stores/capabilities'

/** 批次汇总（P5）：成功/重复/无效/探测失败/超限数量 */
export interface ImportSummary {
  added: number
  duplicate: number
  invalid: number
  probeFailed: number
  overLimit: number
}

interface ProbeResult {
  path: string
  duration: number
  ok: boolean
}

/** 能力缺失时的兜底扩展名（与后端 IMPORT_EXTENSIONS 对齐） */
const FALLBACK_EXTS = ['wav', 'mp3', 'm4a', 'flac', 'ogg', 'mp4', 'mkv', 'avi', 'mov', 'flv', 'ts']

export function useImportFlow() {
  const files = useFilesStore()
  const ui = useUiStore()
  const settings = useSettingsStore()
  const caps = useCapabilitiesStore()

  const importing = ref(false)
  let cancelled = false

  function isSupported(path: string, exts: string[]): boolean {
    const lower = path.toLowerCase()
    return exts.some(ext => lower.endsWith(`.${ext}`))
  }

  /** 并发探测并保持结果顺序（capabilities.import.probeConcurrency） */
  function probePaths(paths: string[], concurrency: number): Promise<ProbeResult[]> {
    return new Promise((resolve) => {
      const out: ProbeResult[] = new Array(paths.length)
      let next = 0
      let pending = 0
      let done = 0

      function worker() {
        while (pending < concurrency && next < paths.length) {
          const idx = next++
          const path = paths[idx]
          pending++
          invoke<number>('get_audio_duration', { filePath: path })
            .then(d => { out[idx] = { path, duration: Number(d), ok: true } })
            .catch(() => { out[idx] = { path, duration: 0, ok: false } })
            .finally(() => {
              pending--
              done++
              if (done === paths.length) resolve(out)
              else worker()
            })
        }
      }
      worker()
    })
  }

  function showSummary(s: ImportSummary) {
    const parts: string[] = []
    if (s.added > 0) parts.push(`成功 ${s.added}`)
    if (s.duplicate > 0) parts.push(`重复 ${s.duplicate}`)
    if (s.invalid > 0) parts.push(`无效 ${s.invalid}`)
    if (s.probeFailed > 0) parts.push(`探测失败 ${s.probeFailed}`)
    if (s.overLimit > 0) parts.push(`超限 ${s.overLimit}`)
    ui.showToastMessage(parts.length > 0 ? `导入完成：${parts.join('，')}` : '未导入任何文件', 'success')
  }

  /**
   * 统一导入入口：过滤扩展名 → 批次/列表上限裁剪 → 并发探测 → 只加入有效文件。
   * 返回汇总；返回 null 表示未执行（无 Key / 进行中 / 无可导入文件）。
   */
  async function importPaths(paths: string[]): Promise<ImportSummary | null> {
    if (importing.value) return null
    if (!settings.asrKey?.trim()) {
      ui.showToastMessage('请先填写 API Key', 'error')
      ui.isSettingsOpen = true
      return null
    }

    await caps.ensureLoaded()
    const exts = caps.caps?.import.extensions?.length ? caps.caps.import.extensions : FALLBACK_EXTS
    const maxBatch = caps.caps?.import.maxBatch ?? 100
    const maxFiles = caps.caps?.import.maxFiles ?? 500
    const concurrency = caps.caps?.import.probeConcurrency ?? 4

    let batch = paths.filter(p => isSupported(p, exts))
    if (batch.length === 0) {
      ui.showToastMessage('未检测到支持的音频/视频文件', 'error')
      return null
    }

    // 批次上限（maxBatch）
    let overLimit = 0
    if (batch.length > maxBatch) {
      overLimit += batch.length - maxBatch
      batch = batch.slice(0, maxBatch)
    }
    // 文件列表上限（maxFiles）
    const remaining = maxFiles - files.files.length
    if (remaining <= 0) {
      ui.showToastMessage('文件列表已达上限', 'error')
      return null
    }
    if (batch.length > remaining) {
      overLimit += batch.length - remaining
      batch = batch.slice(0, remaining)
    }

    importing.value = true
    cancelled = false
    const summary: ImportSummary = { added: 0, duplicate: 0, invalid: 0, probeFailed: 0, overLimit }
    let firstAddedId: string | null = null

    const results = await probePaths(batch, concurrency)
    for (const r of results) {
      if (cancelled) break
      if (!r.ok) { summary.probeFailed++; continue }
      if (!Number.isFinite(r.duration) || r.duration <= 0) { summary.invalid++; continue }
      const name = r.path.split(/[/\\]/).pop() || r.path
      const id = crypto.randomUUID()
      const result = files.addFile({
        id,
        name,
        path: r.path,
        duration: r.duration,
        size: 0,
        status: 'waiting',
        progress: 0,
        transcription: '',
        polished: '',
        transcriptionWordCount: 0,
        polishedWordCount: 0,
        polishStage: 'idle',
      }, maxFiles)
      if (result === 'added') {
        summary.added++
        if (!firstAddedId) firstAddedId = id
      } else if (result === 'duplicate') {
        summary.duplicate++
      }
    }

    importing.value = false
    if (firstAddedId) files.selectFile(firstAddedId)
    showSummary(summary)
    return summary
  }

  /** 取消进行中的导入（停止派发后续有效文件入队） */
  function cancel() { cancelled = true }

  return { importing, importPaths, cancel }
}
