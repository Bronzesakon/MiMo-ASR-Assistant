/**
 * 外链统一出口（清单 S-01）
 *
 * 所有打开外部链接的操作必须经过本模块：
 * 1. `new URL()` 结构化解析，拒绝协议相对 URL、非法 URL；
 * 2. 仅允许 `https:`，拒绝 `javascript:`/`data:`/`file:`/自定义 scheme；
 * 3. 拒绝 username/password 与非默认端口；
 * 4. hostname 必须精确命中白名单（当前仅小米 MiMo 与 DeepSeek 官方控制台域名），
 *    精确匹配天然拒绝伪造子域（如 platform.deepseek.com.evil.test）；
 * 5. 校验通过后交给 Tauri opener 插件在系统浏览器打开（capability 层另有 URL scope 双保险）。
 *
 * 禁止对动态 href 使用 target="_blank" 期待浏览器自行保护。
 */
import { openUrl } from '@tauri-apps/plugin-opener'

/** 允许跳转的外部域名白名单（精确匹配，不支持通配） */
const ALLOWED_HOSTS: ReadonlySet<string> = new Set([
  'platform.xiaomimimo.com',
  'platform.deepseek.com',
])

/**
 * 校验外链合法性，返回规范化后的 URL；任一规则不满足返回 null。
 * 纯函数，便于单元测试。
 */
export function validateExternalUrl(raw: string): string | null {
  if (typeof raw !== 'string' || raw.length === 0 || raw.length > 2048) {
    return null
  }
  // 拒绝前导空白/控制字符构造的绕过（new URL 会 trim，先显式拒绝更严格）
  const firstCode = raw.codePointAt(0)
  if (firstCode !== undefined && (firstCode <= 0x20 || firstCode === 0x7f)) {
    return null
  }
  let url: URL
  try {
    url = new URL(raw)
  } catch {
    return null
  }
  if (url.protocol !== 'https:') {
    return null
  }
  if (url.username !== '' || url.password !== '') {
    return null
  }
  // 仅允许默认端口（省略或显式 443）
  if (url.port !== '' && url.port !== '443') {
    return null
  }
  const host = url.hostname.toLowerCase()
  if (!ALLOWED_HOSTS.has(host)) {
    return null
  }
  return url.toString()
}

/**
 * 打开外链：白名单校验通过后经系统浏览器打开。
 * 返回 true 表示已放行并调用 opener；false 表示被拒绝或打开失败。
 */
export async function openExternalUrl(raw: string): Promise<boolean> {
  const validated = validateExternalUrl(raw)
  if (!validated) {
    return false
  }
  try {
    await openUrl(validated)
    return true
  } catch {
    return false
  }
}
