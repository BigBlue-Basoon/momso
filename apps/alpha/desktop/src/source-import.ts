// 입력 칸의 클립(끌어 놓기)으로 고른 파일을 원자료로 보존할 때 쓰는 작은 도움(chat-items.ts 의 attachFileToChat).
// 2026-11-16 옛 "새 기록" 창을 없애며 그 창만 쓰던 것(원문을 기록 본문·전사 줄·요약 절로 옮기기 등)은 뺐다.
import type { SourceAssetKind } from './runtime'

export const SOURCE_IMPORT_ACTOR = 'momso-desktop'

export const AUDIO_EXTENSIONS = ['mp3', 'm4a', 'wav', 'aac', 'ogg', 'flac'] as const
export const TEXT_EXTENSIONS = ['txt', 'md', 'json'] as const

const MEDIA_TYPES: Record<string, string> = {
  mp3: 'audio/mpeg',
  m4a: 'audio/mp4',
  wav: 'audio/wav',
  aac: 'audio/aac',
  ogg: 'audio/ogg',
  flac: 'audio/flac',
  txt: 'text/plain',
  md: 'text/markdown',
  json: 'application/json',
}

export function fileNameFromPath(path: string) {
  const trimmed = path.trim().replace(/[\\/]+$/, '')
  const separator = Math.max(trimmed.lastIndexOf('\\'), trimmed.lastIndexOf('/'))
  return separator >= 0 ? trimmed.slice(separator + 1) : trimmed
}

function fileExtension(fileName: string) {
  const match = /\.([^./\\]+)$/.exec(fileName)
  return match ? match[1].toLowerCase() : ''
}

export function isTextSourceName(fileName: string) {
  return (TEXT_EXTENSIONS as readonly string[]).includes(fileExtension(fileName))
}

export function isAudioSourceName(fileName: string) {
  return (AUDIO_EXTENSIONS as readonly string[]).includes(fileExtension(fileName))
}

export function isSupportedSourceName(fileName: string) {
  return isTextSourceName(fileName) || isAudioSourceName(fileName)
}

export function assetKindForFileName(fileName: string): SourceAssetKind {
  if (isAudioSourceName(fileName)) return 'audio_recording'
  if (isTextSourceName(fileName)) return 'raw_transcript'
  return 'other'
}

export function mediaTypeForFileName(fileName: string) {
  return MEDIA_TYPES[fileExtension(fileName)] ?? null
}
