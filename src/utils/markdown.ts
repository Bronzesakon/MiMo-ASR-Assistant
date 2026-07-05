/**
 * Markdown → HTML 转换器
 * 基于 marked.js (MIT)，CommonMark + GFM 完整支持
 */

import { marked } from 'marked'

marked.setOptions({
  breaks: true,
  gfm: true,
})

export function markdownToHtml(md: string): string {
  if (!md) return ''
  return marked.parse(md, { async: false }) as string
}
