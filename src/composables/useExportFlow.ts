// 导出单飞（P6）：按 fileId 维护 exporting 状态，保存对话框与写入期间禁用该文件导出按钮。
// 导出前按 UTF-8 bytes 校验 ≤ capabilities.export.maxBytes；用户取消不显示错误；
// 文件已删除时忽略迟到结果，不创建任何 file state。
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { useCapabilitiesStore } from '../stores/capabilities'
import { useUiStore } from '../stores/ui'

export type ExportType = 'original' | 'polished'
export type ExportFormat = 'txt' | 'md'

export interface ExportResult {
  ok: boolean
  code?: 'EMPTY' | 'TOO_LARGE' | 'FAILED'
}

export function useExportFlow() {
  const caps = useCapabilitiesStore()
  const ui = useUiStore()

  /** fileId -> 是否正在导出（保存对话框或写入期间） */
  const exporting = ref<Record<string, boolean>>({})

  function isExporting(fileId: string): boolean {
    return !!exporting.value[fileId]
  }

  function setExporting(fileId: string, v: boolean) {
    exporting.value = { ...exporting.value, [fileId]: v }
  }

  function utf8Bytes(text: string): number {
    return new TextEncoder().encode(text).length
  }

  /**
   * 导出内容：先做 UTF-8 bytes 上限校验，再进行后端保存（对话框+写入一体）。
   * callbacks：getContent 取文本，getDefaultName 生成建议文件名（均在文件仍存在时读取）。
   */
  async function exportFile(
    fileId: string,
    type: ExportType,
    format: ExportFormat,
    getContent: (fileId: string, type: ExportType) => string,
    getDefaultName: (fileId: string, type: ExportType) => string,
  ): Promise<ExportResult> {
    if (isExporting(fileId)) return { ok: false }

    const content = getContent(fileId, type)
    if (!content) {
      ui.showToastMessage('暂无导出内容', 'error')
      return { ok: false, code: 'EMPTY' }
    }

    await caps.ensureLoaded()
    const maxBytes = caps.caps?.export.maxBytes ?? 16 * 1024 * 1024
    if (utf8Bytes(content) > maxBytes) {
      ui.showToastMessage('导出内容超过 16 MiB 上限，无法导出', 'error')
      return { ok: false, code: 'TOO_LARGE' }
    }

    setExporting(fileId, true)
    try {
      const savedPath = await invoke<string | null>('export_file', {
        content,
        suggestedName: getDefaultName(fileId, type),
        format,
      })
      // 用户取消时后端返回 null，不报错；文件已删除时忽略迟到结果（export 不创建 state）
      if (savedPath) ui.showToastMessage(`已保存: ${savedPath}`, 'success')
      return { ok: true }
    } catch (e) {
      console.error('[导出] 失败:', e)
      ui.showToastMessage('导出失败', 'error')
      return { ok: false, code: 'FAILED' }
    } finally {
      setExporting(fileId, false)
    }
  }

  return { exporting, isExporting, exportFile }
}