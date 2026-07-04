/**
 * Markdown → HTML 转换器 (CommonMark + GFM 子集)
 * 支持：标题、粗体、斜体、粗斜体、删除线、行内/块代码、列表(嵌套)、
 *       引用(多行)、链接、图片、自动链接、表格、任务列表、分隔线、换行、转义
 */

let _placeholderId: number
let _placeholders: Map<string, string>

function placeholder(html: string): string {
  const key = `\x00PH${_placeholderId++}\x00`
  _placeholders.set(key, html)
  return key
}

export function markdownToHtml(md: string): string {
  if (!md) return ''

  _placeholderId = 0
  _placeholders = new Map()

  // ── 1. 提取代码块和行内代码，避免后续规则误匹配 ──

  // 围栏代码块
  let text = md.replace(/```(\w*)\n([\s\S]*?)```/g, (_m, lang, code) => {
    const cls = lang ? ` class="language-${lang}"` : ''
    return placeholder(`<pre><code${cls}>${escapeHtml(code.trim())}</code></pre>`)
  })

  // 缩进代码块（4空格或tab）
  text = text.replace(/^(?: {4}|\t)(.+)$/gm, (_m, code) => {
    return placeholder(`<pre><code>${escapeHtml(code)}</code></pre>`)
  })

  // 行内代码
  text = text.replace(/`([^`]+)`/g, (_m, code) => {
    return placeholder(`<code>${escapeHtml(code)}</code>`)
  })

  // ── 2. 转义字符 ──
  text = text.replace(/\\([\\`*_{}\[\]()#+\-.!~|>])/g, '\x01$1\x01')

  // ── 3. HTML 实体转义（占位符内部已处理，此处处理正文）──
  text = text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')

  // ── 4. 块级元素（逐行解析）──
  const lines = text.split('\n')
  const blocks: string[] = []
  let i = 0

  while (i < lines.length) {
    const line = lines[i]

    // 空行
    if (line.trim() === '') { i++; continue }

    // 标题
    const headingMatch = line.match(/^(#{1,6})\s+(.+)$/)
    if (headingMatch) {
      const level = headingMatch[1].length
      blocks.push(`<h${level}>${renderInline(headingMatch[2])}</h${level}>`)
      i++; continue
    }

    // 分隔线
    if (/^[-*_]{3,}\s*$/.test(line)) {
      blocks.push('<hr>')
      i++; continue
    }

    // 表格
    if (i + 1 < lines.length && /^\|?[\s:-]+\|/.test(lines[i + 1])) {
      const tableLines: string[] = [line]
      let j = i + 1
      while (j < lines.length && lines[j].includes('|') && lines[j].trim() !== '') {
        tableLines.push(lines[j]); j++
      }
      if (tableLines.length >= 2) {
        blocks.push(renderTable(tableLines)); i = j; continue
      }
    }

    // 引用
    if (/^&gt;\s?/.test(line)) {
      const quoteLines: string[] = []
      while (i < lines.length && /^&gt;\s?/.test(lines[i])) {
        quoteLines.push(lines[i].replace(/^&gt;\s?/, ''))
        i++
      }
      blocks.push(`<blockquote>${markdownToHtml(quoteLines.join('\n'))}</blockquote>`)
      continue
    }

    // 无序列表
    if (/^(\s*)[-*+]\s/.test(line)) {
      const result = parseList(lines, i, 'ul')
      blocks.push(result.html); i = result.next; continue
    }

    // 有序列表
    if (/^(\s*)\d+\.\s/.test(line)) {
      const result = parseList(lines, i, 'ol')
      blocks.push(result.html); i = result.next; continue
    }

    // 段落：收集连续非空、非特殊开头的行
    const paraLines: string[] = []
    while (i < lines.length && lines[i].trim() !== '' &&
      !/^(#{1,6}\s|[-*_]{3,}\s*$|&gt;\s?|\s*[-*+]\s|\s*\d+\.\s|\|.*\|)/.test(lines[i])) {
      paraLines.push(lines[i]); i++
    }
    if (paraLines.length > 0) {
      blocks.push(`<p>${renderInline(paraLines.join('<br>'))}</p>`)
    }
  }

  let html = blocks.join('\n')

  // ── 5. 还原占位符（循环处理嵌套占位符）──
  let safety = 10
  while (html.includes('\x00PH') && safety-- > 0) {
    html = html.replace(/\x00PH\d+\x00/g, (key) => _placeholders.get(key) || key)
  }

  // ── 6. 还原转义字符 ──
  html = html.replace(/\x01(.)\x01/g, '$1')

  return html
}

// ── 行内元素渲染 ──

function renderInline(text: string): string {
  // 图片 ![alt](url)（必须在链接之前）
  text = text.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, '<img src="$2" alt="$1">')

  // 链接 [text](url)
  text = text.replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank">$1</a>')

  // 粗斜体 ***text***
  text = text.replace(/\*\*\*(.+?)\*\*\*/g, '<strong><em>$1</em></strong>')

  // 粗体 **text**
  text = text.replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
  text = text.replace(/__(.+?)__/g, '<strong>$1</strong>')

  // 斜体 *text*
  text = text.replace(/\*(.+?)\*/g, '<em>$1</em>')
  // _斜体_ 仅匹配单词边界，避免 some_variable_name 误触发
  text = text.replace(/\b_(.+?)_\b/g, '<em>$1</em>')

  // 删除线 ~~text~~
  text = text.replace(/~~(.+?)~~/g, '<del>$1</del>')

  // 自动链接裸 URL（http/https/www）
  text = text.replace(
    /(^|[^"(])(https?:\/\/[^\s<>&*~_`]+[^\s<>&*~_`.,;:!?)\]])/g,
    '$1<a href="$2" target="_blank">$2</a>'
  )
  text = text.replace(
    /(^|[^"(])(www\.[^\s<>&*~_`]+[^\s<>&*~_`.,;:!?)\]])/g,
    '$1<a href="https://$2" target="_blank">$2</a>'
  )

  return text
}

// ── 列表解析（支持嵌套）──

function parseList(lines: string[], start: number, tag: 'ul' | 'ol'): { html: string; next: number } {
  const indent = getIndent(lines[start])
  const items: string[] = []
  let i = start
  let currentContent = ''
  let inSubList = false

  while (i < lines.length) {
    const line = lines[i]
    const lineIndent = getIndent(line)

    // 缩进减少 → 列表结束
    if (line.trim() !== '' && lineIndent < indent) break

    // 同级列表项
    const isItem = (tag === 'ul' ? /^\s*[-*+]\s/ : /^\s*\d+\.\s/).test(line)
    if (lineIndent === indent && isItem) {
      if (currentContent) items.push(currentContent)
      currentContent = line.replace(/^\s*[-*+\d.]+\s+/, '')
      inSubList = false
      i++
      continue
    }

    // 子列表
    if (lineIndent > indent && /^\s*([-*+]|\d+\.)\s/.test(line)) {
      if (!inSubList) { currentContent += '\n'; inSubList = true }
      currentContent += line + '\n'
      i++
      continue
    }

    // 续行
    if (line.trim() !== '' && lineIndent > indent) {
      currentContent += ' ' + line.trim()
      i++
      continue
    }

    break
  }

  if (currentContent) items.push(currentContent)

  const listItems = items.map(item => {
    // 任务列表
    let content = item
    let checkbox = ''
    const taskMatch = content.match(/^\[([ xX])\]\s+(.*)$/s)
    if (taskMatch) {
      const checked = taskMatch[1] !== ' ' ? ' checked' : ''
      checkbox = `<input type="checkbox"${checked} disabled> `
      content = taskMatch[2]
    }

    // 检查是否包含子列表
    if (content.includes('\n')) {
      const parts = content.split(/\n(?=\s*[-*+]|\s*\d+\.)/)
      const mainText = parts[0].trim()
      const subListMd = parts.slice(1).join('\n')
      const subHtml = subListMd ? markdownToHtml(subListMd) : ''
      return `<li>${checkbox}${renderInline(mainText)}${subHtml}</li>`
    }
    return `<li>${checkbox}${renderInline(content)}</li>`
  }).join('\n')

  return { html: `<${tag}>\n${listItems}\n</${tag}>`, next: i }
}

// ── 表格解析 ──

function renderTable(tableLines: string[]): string {
  const parseRow = (line: string, cellTag: string): string => {
    const cells = line.split('|').slice(1, -1)
      .map(c => `<${cellTag}>${renderInline(c.trim())}</${cellTag}>`)
      .join('')
    return `<tr>${cells}</tr>`
  }

  const header = parseRow(tableLines[0], 'th')
  // 跳过分隔行（第 2 行）
  const bodyRows = tableLines.slice(2).map(r => parseRow(r, 'td')).join('\n')

  return `<table>\n<thead>${header}</thead>\n<tbody>\n${bodyRows}\n</tbody>\n</table>`
}

// ── 工具函数 ──

function escapeHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
}

function getIndent(line: string): number {
  const match = line.match(/^(\s*)/)
  return match ? match[1].length : 0
}
