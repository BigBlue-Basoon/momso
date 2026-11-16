import { W } from './words'
import type { SourceAssetImportPlan, SourceAssetKind, SourceTextPage } from './runtime'
import type { SummarySection, TranscriptLine } from './workbench-data'

export const SOURCE_IMPORT_ACTOR = 'momso-desktop'

export const AUDIO_EXTENSIONS = ['mp3', 'm4a', 'wav', 'aac', 'ogg', 'flac'] as const
export const TEXT_EXTENSIONS = ['txt', 'md', 'json'] as const

/** 원문을 기록 본문으로 옮길 때 읽는 상한. 이보다 크면 원본만 보존하고 본문에는 넣지 않는다. */
export const MAX_INLINE_TEXT_BYTES = 4 * 1024 * 1024

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

export function fileExtension(fileName: string) {
  const match = /\.([^./\\]+)$/.exec(fileName)
  return match ? match[1].toLowerCase() : ''
}

export function stripExtension(fileName: string) {
  return fileName.replace(/\.[^/.]+$/, '')
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

export function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function shortChecksum(hash: string) {
  return `SHA-256 · ${hash.slice(0, 8)}…${hash.slice(-8)}`
}

export function splitParagraphs(text: string) {
  return text
    .split(/\r?\n+/)
    .map((line) => line.trim())
    .filter(Boolean)
}

/** 가져오기 미리보기 한 줄. 사람이 확정 전에 무엇이 보존되는지 읽는 문장. */
export function describeImportPlan(plan: SourceAssetImportPlan) {
  const size = formatFileSize(plan.byteLength)
  const checksum = shortChecksum(plan.contentSha256)
  if (plan.alreadyStored) {
    return W.sourceImport.text1(size, checksum)
  }
  return W.sourceImport.text2(size, checksum)
}

export type SourceTextPageReader = (cursor: number, expectedContentSha256?: string) => Promise<SourceTextPage>

/**
 * 해시로 고정된 원문을 첫 페이지부터 끝까지 읽어 이어 붙인다.
 * 첫 페이지의 해시를 이후 페이지에 넘겨 중간에 바뀐 원문은 백엔드가 거부하게 한다.
 */
export async function collectSourceText(readPage: SourceTextPageReader, maxBytes = MAX_INLINE_TEXT_BYTES) {
  const first = await readPage(0)
  if (first.byteLength > maxBytes) {
    throw new Error(W.sourceImport.text3(formatFileSize(maxBytes)))
  }
  const chunks = [first.content]
  let next = first.nextCursor
  while (next !== null) {
    const page = await readPage(next, first.contentSha256)
    chunks.push(page.content)
    next = page.nextCursor
  }
  return { text: chunks.join(''), contentSha256: first.contentSha256, byteLength: first.byteLength }
}

export function transcriptFromText(text: string): { transcript: TranscriptLine[]; paragraphs: string[] } {
  const paragraphs = splitParagraphs(text)
  return {
    paragraphs,
    transcript: paragraphs.map((paragraph, index) => ({
      time: W.sourceImport.text4(index + 1),
      speaker: W.sourceImport.text5,
      text: paragraph,
    })),
  }
}

export function summaryFromParagraphs(paragraphs: string[], fallback: string, note: string): SummarySection[] {
  const sections: SummarySection[] = [{
    title: W.sourceImport.text6,
    body: paragraphs.slice(0, 3).join('\n\n') || fallback,
    source: paragraphs.length > 1 ? W.sourceImport.text7(Math.min(paragraphs.length, 3)) : W.sourceImport.text8,
  }]
  if (note.trim()) sections.push({ title: W.sourceImport.text9, body: note.trim(), source: W.sourceImport.text10 })
  return sections
}
