export interface FileInfo {
  id: string
  name: string
  path: string
  duration: number
  size: number
  status: 'waiting' | 'processing' | 'transcribing' | 'polishing' | 'completed' | 'failed'
  progress: number
  transcription: string
  polished: string
  transcriptionWordCount: number
  polishedWordCount: number
  polishStage: 'idle' | 'sending' | 'receiving' | 'failed'
  error?: string
}

export interface ApiConfig {
  provider: string
  baseUrl: string
  apiKey: string
  transcriptionModel: string
  polishModel: string
  polishPrompt: string
  providerKeys: Record<string, string>
}

export interface ProviderInfo {
  id: string
  name: string
  baseUrl: string
  defaultModel: string
  keyPrefix: string
}
