/**
 * 外链白名单测试（清单 S-01 验收语料）
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

vi.mock('@tauri-apps/plugin-opener', () => ({
  openUrl: vi.fn(async () => {}),
}))

import { validateExternalUrl, openExternalUrl } from './externalLink'
import { openUrl } from '@tauri-apps/plugin-opener'

describe('validateExternalUrl 白名单校验', () => {
  it('放行白名单内官方 HTTPS 链接', () => {
    expect(validateExternalUrl('https://platform.xiaomimimo.com/console/api-keys')).toBe(
      'https://platform.xiaomimimo.com/console/api-keys',
    )
    expect(validateExternalUrl('https://platform.deepseek.com/api_keys')).toBe(
      'https://platform.deepseek.com/api_keys',
    )
    // 显式 443 端口等价于默认端口
    expect(validateExternalUrl('https://platform.deepseek.com:443/top_up')).not.toBeNull()
  })

  it('拒绝 javascript: / file: / data: / 自定义 scheme', () => {
    expect(validateExternalUrl('javascript:alert(1)')).toBeNull()
    expect(validateExternalUrl('file:///C:/Windows/System32')).toBeNull()
    expect(validateExternalUrl('data:text/html,<script>alert(1)</script>')).toBeNull()
    expect(validateExternalUrl('custom://platform.deepseek.com')).toBeNull()
  })

  it('拒绝 http 明文与非白名单域名', () => {
    expect(validateExternalUrl('http://platform.deepseek.com/api_keys')).toBeNull()
    expect(validateExternalUrl('https://example.com')).toBeNull()
    expect(validateExternalUrl('https://api.deepseek.com/v1')).toBeNull()
  })

  it('拒绝伪造子域与协议相对 URL', () => {
    expect(validateExternalUrl('https://platform.deepseek.com.evil.test/x')).toBeNull()
    expect(validateExternalUrl('https://evilplatform.xiaomimimo.com.evil.test/')).toBeNull()
    expect(validateExternalUrl('//platform.deepseek.com/api_keys')).toBeNull()
  })

  it('拒绝 userinfo、非默认端口、空值与超长输入', () => {
    expect(validateExternalUrl('https://user:pass@platform.deepseek.com/')).toBeNull()
    expect(validateExternalUrl('https://platform.deepseek.com:8443/x')).toBeNull()
    expect(validateExternalUrl('')).toBeNull()
    expect(validateExternalUrl('x'.repeat(4096))).toBeNull()
    expect(validateExternalUrl('not a url')).toBeNull()
  })
})

describe('openExternalUrl 出口行为', () => {
  beforeEach(() => {
    vi.mocked(openUrl).mockClear()
  })

  it('合法 URL 才调用 opener', async () => {
    expect(await openExternalUrl('https://platform.deepseek.com/top_up')).toBe(true)
    expect(openUrl).toHaveBeenCalledTimes(1)
    expect(openUrl).toHaveBeenCalledWith('https://platform.deepseek.com/top_up')
  })

  it('非法 URL 不调用 opener', async () => {
    expect(await openExternalUrl('javascript:alert(1)')).toBe(false)
    expect(await openExternalUrl('https://example.com')).toBe(false)
    expect(openUrl).not.toHaveBeenCalled()
  })

  it('opener 抛错时安全降级为 false', async () => {
    vi.mocked(openUrl).mockRejectedValueOnce(new Error('denied by scope'))
    expect(await openExternalUrl('https://platform.deepseek.com/api_keys')).toBe(false)
  })
})
