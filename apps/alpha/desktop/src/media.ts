// 사진·동영상·소리(미디어 화면). 계약: archive-backend-port-notes.md "2026-11-16 옮긴 것"
//   media_list({ kind, before, limit }) → [{ id, kind: 'image' | 'video' | 'audio', title, createdAt, width, height, durationSec,
//                                            path, posterPath, chatId, by, how, at, chats: [{ id, title }] }]
//   - path: 파일의 로컬 경로(화면은 convertFileSrc 로 보인다). posterPath: 동영상의 첫 장면 그림(없으면 null).
//   - createdAt: 미디어를 등록한 때. 화면은 새것부터 날짜별로 묶는다.
//   - by·how·at: 첨부 작업 내역이 있으면 { kind: 'user' }·'attached'·그때. AI 가 채팅방에서 만들고 고친 것은 새 기록부터
//     { kind: 'ai', provider, model }·'created' | 'edited'·그때. 알 수 없으면 null(화면은 그 칸을 비운다. by 가 null 이면 누가 했다고 적지 않는다).
//   - evidence: 'tool'(도구가 성공을 알림) | 'diff'(파일 전후 비교로 확인) | null(옛 기록). 값만 받고 화면에는 보이지 않는다(보일지는 그가 정하지 않음).
//   - 겹쳐 답한 채팅방이 있어 누가 했는지 가를 수 없으면 by 가 null 이다.
import { convertFileSrc } from '@tauri-apps/api/core'
import { invoke } from './host'
import { hasAppBackend, isNetworkRuntime } from './runtime'
import { serverMediaDownloadPath } from './server-client'

export type MediaItem = {
  id: string
  kind: 'image' | 'video' | 'audio'
  title: string | null
  createdAt: string
  width: number | null
  height: number | null
  durationSec: number | null
  path: string
  posterPath: string | null
  chatId: string | null
  chats?: Array<{ id: string; title: string | null }> | null
  by?: { kind: 'ai'; provider: 'codex' | 'claude' | 'gemini'; model?: string | null } | { kind: 'user' } | null
  how?: 'created' | 'edited' | 'attached' | null
  /** 그 일을 한 때(모르면 null). */
  at?: string | null
  evidence?: 'tool' | 'diff' | null
  /**
   * 생성물의 프롬프트와 버전(2026-11-18 화면이 바라는 꼴, 뒤쪽은 아직 주지 않음 — 개발 앱의 예시 생성물만 채운다).
   *   prompt: 이 버전을 만들 때 AI 가 받은 말(고친 것이면 고치라는 말). lineageId: 같은 생성물의 버전끼리 같은 값.
   *   version: 그 묶음 안의 차례(1부터). versionOf: 어느 버전에서 나왔는지. versionHow: 'regenerated'(같은 프롬프트로 다시) | 'edited'(고침).
   */
  prompt?: string | null
  lineageId?: string | null
  version?: number | null
  versionOf?: string | null
  versionHow?: 'regenerated' | 'edited' | null
  /**
   * 2026-11-18 뒤쪽 9e54c07(desktop-media-contract.md C52): 목록에는 같은 묶음의 최신 판 하나와 판 수(versionCount)만 온다.
   * 판 전체는 media_versions({ versionGroupId }). provider·model 은 생성 기록의 회사·모델(by 가 없을 때 그것으로 보인다).
   */
  versionCount?: number | null
  provider?: string | null
  model?: string | null
}

/** 그 미디어를 만든(첨부한) 채팅방 하나. chats 가 있으면 첫째, 없으면 예전 chatId. */
export function mediaChat(item: MediaItem): { id: string; title: string | null } | null {
  if (item.chats && item.chats.length > 0) return item.chats[0]
  return item.chatId ? { id: item.chatId, title: null } : null
}

const AI_PROVIDERS = ['codex', 'claude', 'gemini'] as const
type AiName = (typeof AI_PROVIDERS)[number]
/** 생성 기록만 있고 작업 기록(by)이 없으면 생성 기록의 회사·모델로 누가 만들었는지 채운다. */
function withMaker(item: MediaItem): MediaItem {
  if (item.by || !item.provider || !(AI_PROVIDERS as readonly string[]).includes(item.provider)) return item
  return { ...item, by: { kind: 'ai', provider: item.provider as AiName, model: item.model ?? null } }
}

export async function listMedia() {
  // 휴대폰 몸소도(S75 셋째, server GET /media). 그곳의 path 는 같은 출처의 내려받기 주소다.
  if (!hasAppBackend()) throw new Error('media_list needs the desktop runtime')
  return (await invoke<MediaItem[]>('media_list')).map(withMaker)
}

/**
 * 한 생성물의 판 전체(오름차순). 원자료가 지워졌거나 격리되면 그 판의 path 는 null(화면은 그 판을 고를 수 없게 그린다).
 * 판 항목에는 크기·첫 장면 그림·작업 기록이 없어, 목록에서 받은 그 생성물(cover)의 값을 이어 쓴다.
 */
export async function listMediaVersions(cover: MediaItem): Promise<MediaItem[]> {
  if (!hasAppBackend() || !cover.lineageId) return [cover]
  const list = await invoke<Array<{ assetId: string; kind?: MediaItem['kind']; prompt?: string | null; provider?: string | null; model?: string | null; createdAt?: string | null; chatId?: string | null; version?: number | null; path?: string | null; lineageId?: string | null; versionOf?: string | null; versionHow?: 'regenerated' | 'edited' | null }>>('media_versions', { versionGroupId: cover.lineageId })
  return list.map((version) => {
    if (version.assetId === cover.id) return { ...cover, version: version.version ?? cover.version, versionOf: version.versionOf ?? null, versionHow: version.versionHow ?? null }
    return withMaker({
      ...cover,
      id: version.assetId,
      kind: version.kind ?? cover.kind,
      title: null,
      createdAt: version.createdAt ?? cover.createdAt,
      at: version.createdAt ?? null,
      path: version.path ?? '',
      posterPath: null,
      by: null,
      how: null,
      prompt: version.prompt ?? null,
      provider: version.provider ?? null,
      model: version.model ?? null,
      lineageId: version.lineageId ?? cover.lineageId,
      version: version.version ?? null,
      versionOf: version.versionOf ?? null,
      versionHow: version.versionHow ?? null,
    })
  })
}

/** Windows 저장 창으로 내려받기(media_save_as). 취소하면 'cancelled', 저장하면 'saved'. 오류는 이름 그대로 던진다. */
export async function saveMediaAs(assetId: string): Promise<'saved' | 'cancelled' | 'downloading'> {
  // 휴대폰 몸소: 저장 창이 없어 브라우저가 내려받는다(저장했는지는 알 수 없어 'downloading', 알림 없음).
  if (isNetworkRuntime()) {
    const link = document.createElement('a')
    link.href = serverMediaDownloadPath(assetId)
    link.download = ''
    link.click()
    return 'downloading'
  }
  const result = await invoke<{ status?: string } | null>('media_save_as', { assetId })
  return result?.status === 'saved' ? 'saved' : 'cancelled'
}

/** 화면에 보일 주소. 로컬 경로는 Tauri 의 주소로 바꾸고, 이미 주소(data:, http:)면 그대로. */
export function mediaSrc(path: string | null) {
  if (!path) return null
  // 휴대폰 몸소: server 의 같은 출처 주소(/media/…/download)는 그대로.
  if (isNetworkRuntime() && path.startsWith('/')) return path
  return /^(data:|https?:|blob:)/.test(path) ? path : convertFileSrc(path)
}
