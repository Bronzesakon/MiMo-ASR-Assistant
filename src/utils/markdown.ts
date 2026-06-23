/**
 * 轻量级 Markdown → HTML 转换器
 * 支持：标题、粗体、斜体、行内代码、代码块、列表、引用、链接、分隔线
 */
export function markdownToHtml(md: string): string {
  if (!md) return ''

  // 转义 HTML 特殊字符
  let html = md
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')

  // 代码块 ```...```
  html = html.replace(/```(\w*)\n([\s\S]*?)```/g, (_match, _lang, code) => {
    return `<pre><code>${code.trim()}</code></pre>`
  })

  // 行内代码 `...`
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>')

  // 标题 # ~ ######
  html = html.replace(/^######\s+(.+)$/gm, '<h6>$1</h6>')
  html = html.replace(/^#####\s+(.+)$/gm, '<h5>$1</h5>')
  html = html.replace(/^####\s+(.+)$/gm, '<h4>$1</h4>')
  html = html.replace(/^###\s+(.+)$/gm, '<h3>$1</h3>')
  html = html.replace(/^##\s+(.+)$/gm, '<h2>$1</h2>')
  html = html.replace(/^#\s+(.+)$/gm, '<h1>$1</h1>')

  // 粗体 **...** 或 __...__
  html = html.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
  html = html.replace(/__(.+?)__/g, '<strong>$1</strong>')

  // 斜体 *...* 或 _..._
  html = html.replace(/\*(.+?)\*/g, '<em>$1</em>')
  html = html.replace(/_(.+?)_/g, '<em>$1</em>')

  // 删除线 ~~...~~
  html = html.replace(/~~(.+?)~~/g, '<del>$1</del>')

  // 引用 > ...
  html = html.replace(/^&gt;\s+(.+)$/gm, '<blockquote>$1</blockquote>')

  // 无序列表 - 或 *
  html = html.replace(/^[\-\*]\s+(.+)$/gm, '<li>$1</li>')
  html = html.replace(/(<li>.*<\/li>\n?)+/g, '<ul>$&</ul>')

  // 有序列表 1. 2. ...
  html = html.replace(/^\d+\.\s+(.+)$/gm, '<li>$1</li>')
  // 避免重复包裹（已被 ul 包裹的跳过）
  html = html.replace(/(?<!<\/ul>)(<li>.*<\/li>\n?)+/g, (match) => {
    if (match.includes('<ul>')) return match
    return `<ol>${match}</ol>`
  })

  // 分隔线 --- 或 ***
  html = html.replace(/^[-*]{3,}$/gm, '<hr>')

  // 链接 [text](url)
  html = html.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank">$1</a>')

  // 段落：连续非空行合并为 <p>，空行分段
  html = html.replace(/^(?!<[a-z/])((?!^\s*$).+)$/gm, '<p>$1</p>')

  // 清理多余的 <p> 包裹的块级元素
  html = html.replace(/<p>(<(?:h[1-6]|ul|ol|pre|blockquote|hr)[\s\S]*?<\/(?:h[1-6]|ul|ol|pre|blockquote|hr)>)<\/p>/g, '$1')
  html = html.replace(/<p>(<hr>)<\/p>/g, '$1')

  return html
}
