import { beforeEach, describe, expect, it, vi } from 'vitest'

const files = {
  files: [] as Array<{ id: string; path: string }>,
  addFile: vi.fn((file) => {
    files.files.push(file)
    return 'added'
  }),
  selectFile: vi.fn(),
}
const ui = { showToastMessage: vi.fn(), isSettingsOpen: false }
const settings = { asrKey: 'key' }
const caps = {
  ensureLoaded: vi.fn(),
  caps: { import: { extensions: ['mp4'], maxBatch: 100, maxFiles: 500, probeConcurrency: 4 } },
}

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(10)) }))
vi.mock('../stores/files', () => ({ useFilesStore: () => files }))
vi.mock('../stores/ui', () => ({ useUiStore: () => ui }))
vi.mock('../stores/settings', () => ({ useSettingsStore: () => settings }))
vi.mock('../stores/capabilities', () => ({ useCapabilitiesStore: () => caps }))

import { useImportFlow } from './useImportFlow'

describe('useImportFlow', () => {
  beforeEach(() => {
    files.files.length = 0
    files.addFile.mockClear()
    files.selectFile.mockClear()
    vi.spyOn(crypto, 'randomUUID').mockReturnValue('11111111-1111-4111-8111-111111111111')
  })

  it('导入后使用文件 ID 选中首个新增文件', async () => {
    await useImportFlow().importPaths(['E:\\media\\test.mp4'])

    expect(files.selectFile).toHaveBeenCalledWith('11111111-1111-4111-8111-111111111111')
    expect(files.selectFile).not.toHaveBeenCalledWith('E:\\media\\test.mp4')
  })
})
