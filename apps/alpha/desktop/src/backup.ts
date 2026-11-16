// 백업 두 가지(2026-11-16 결정): GitHub = 문서와 고친 이력, 오브젝트 스토리지 = 원자료. DB(Neon)는 없다.
// GitHub 계약: momso-desktop-backend/outputs/2026-11-15-momso-archive-remote-contract.md
//   archive_remote_status / connect({ repo }) / push / pull / disconnect → { state, reason, historyReason, repo, visibility, lastPushedAt, ahead, behind, signedInAs }
// 오브젝트 스토리지 계약: momso-desktop-backend/outputs/2026-11-16-momso-object-store-contract.md
//   object_store_connect({ provider, endpoint, region, bucket, accessKeyId, secretAccessKey }), object_store_status, object_store_disconnect,
//   object_store_backup, object_store_restore({ ids }), app_state_backup, app_state_restore, 이벤트 object-store-progress / done / failed
// 키는 받아서 바로 넘기기만 한다(화면에 남기지 않는다).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type RemoteStatus = {
  state: 'not_connected' | 'connected' | 'unavailable'
  reason: string | null
  /**
   * reason 이 unsafe_history 일 때 그 까닭(T34·T35): profile_in_history(사용자 정보 파일이 이력에 있음),
   * profile_in_rules_history(규칙 파일의 이력에 사용자 정보가 있음), 그 밖은 null. 사용자 정보를 올릴지 그가 정할 때까지 막아 둔 것이다.
   */
  historyReason?: 'profile_in_history' | 'profile_in_rules_history' | string | null
  repo: string | null
  visibility: string | null
  lastPushedAt: string | null
  ahead: number | null
  behind: number | null
  signedInAs: string | null
}

export type StoreProvider = 'aws' | 'gcp' | 'cloudflare' | 'ncp'

export type StoreStatus = {
  state: 'not_connected' | 'connected' | 'unavailable'
  provider: StoreProvider | 's3_compatible' | null
  bucket: string | null
  lastBackupAt: string | null
  pendingCount: number | null
  pendingBytes: number | null
  /**
   * 연결할 때 뒤쪽이 정한 쓰기 방식(연결 하나의 값): conditional(조건부 쓰기), checkThenWrite(있는지 본 뒤 쓰고 다시 읽어 대조), null(검사 결과 없음).
   * 예전 연결에 검사 결과가 없으면 state 가 unavailable 로 오고 다시 연결해야 한다. 네 회사와 S3 호환 모두 고를 수 있다.
   */
  writeSafety?: 'conditional' | 'checkThenWrite' | null
}

export type StoreProgress = { completedCount: number; completedBytes: number; totalCount: number; totalBytes: number }

function need() { if (!isDesktopRuntime()) throw new Error('backup needs the desktop runtime') }

export async function remoteStatus() { need(); return invoke<RemoteStatus>('archive_remote_status') }
export async function remoteConnect(repo: string) { need(); return invoke<RemoteStatus>('archive_remote_connect', { repo }) }
export async function remotePush() { need(); return invoke<RemoteStatus>('archive_remote_push') }
export async function remotePull() { need(); return invoke<RemoteStatus>('archive_remote_pull') }
export async function remoteDisconnect() { need(); return invoke<RemoteStatus>('archive_remote_disconnect') }

export async function storeStatus() { need(); return invoke<StoreStatus>('object_store_status') }
export async function storeConnect(input: { provider: StoreProvider; endpoint: string; region: string; bucket: string; accessKeyId: string; secretAccessKey: string }) {
  need(); return invoke<StoreStatus>('object_store_connect', input)
}
export async function storeDisconnect() { need(); return invoke<null>('object_store_disconnect') }
export async function storeBackup() { need(); await invoke('object_store_backup'); await invoke('app_state_backup') }
export async function storeRestore() { need(); await invoke('app_state_restore'); return invoke<{ restoredCount: number; skippedCount: number }>('object_store_restore', { ids: null }) }

export async function listenStoreProgress(onProgress: (value: StoreProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<StoreProgress>('object-store-progress', (event) => onProgress(event.payload)),
    listen<StoreProgress>('object-store-done', (event) => onProgress(event.payload)),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 실패한 명령의 상태 값(오류 문자열 안의 값). */
export function errorState(error: unknown) {
  const text = String((error as { message?: string })?.message ?? error)
  return /[a-z_]+/.exec(text)?.[0] ?? null
}

/** 회사별 주소: AWS 는 지역에서, GCP 는 공식 주소, Cloudflare 와 NCP 는 사용자가 넣은 주소. */
export function storeEndpoint(provider: StoreProvider, region: string, endpoint: string) {
  if (provider === 'aws') return `https://s3.${region.trim()}.amazonaws.com`
  if (provider === 'gcp') return 'https://storage.googleapis.com'
  return endpoint.trim()
}
