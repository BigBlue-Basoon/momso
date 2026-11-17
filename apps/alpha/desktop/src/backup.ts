// 백업 두 가지(2026-11-16 결정): GitHub = 문서와 고친 이력, 오브젝트 스토리지 = 원자료. DB(Neon)는 없다.
// GitHub 계약: momso-desktop-backend/outputs/2026-11-15-momso-archive-remote-contract.md
//   archive_remote_status / connect({ repo }) / push / pull / disconnect → { state, reason, historyReason, repo, visibility, lastPushedAt, ahead, behind, signedInAs }
// 오브젝트 스토리지 계약: momso-desktop-backend/outputs/2026-11-16-momso-object-store-contract.md
//   object_store_connect({ provider, endpoint, region, bucket, accessKeyId, secretAccessKey }), object_store_status, object_store_disconnect,
//     bucket 이 비어 있으면 저장된 username 으로 momso-<username> 버킷을 만들어(이미 있으면 그대로) 연결한다. GCP 는 프로젝트 ID 와 OAuth 토큰이 더 필요해
//     화면은 GCP 에서 버킷 이름을 그대로 받는다(원자료 버킷 자동 생성 계약, desktop-object-store-auto-bucket.md).
//   object_store_backup, object_store_restore({ ids }), app_state_backup, app_state_restore, 이벤트 object-store-progress / done / failed
// 키는 받아서 바로 넘기기만 한다(화면에 남기지 않는다).
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { isDesktopRuntime } from './runtime'

export type RemoteStatus = {
  state: 'not_connected' | 'connected' | 'unavailable'
  reason: string | null
  /** 예전 계약의 이력 까닭. 2026-11-16 그의 결정(사용자 정보도 올림) 뒤로 뒤쪽은 늘 null 을 준다. 화면은 쓰지 않는다. */
  historyReason?: string | null
  repo: string | null
  visibility: string | null
  lastPushedAt: string | null
  ahead: number | null
  behind: number | null
  signedInAs: string | null
  /**
   * 올리기·받기의 합치기 결과(2026-11-17, archive-backend-port-notes.md "GitHub 이력 합치기").
   * autoMerged: 서로 다른 변경을 저절로 합쳤다(화면은 알리지 않는다). preservedDocuments: 양쪽에서 고친 같은 문서라 이 PC 의 것을 이전 버전으로 남긴 문서 번호.
   * conflictPaths: 풀지 못해 남은 경로(reason diverged 와 함께). 계산하지 못했으면 null(수를 짐작하지 않는다).
   */
  autoMerged?: boolean
  preservedDocuments?: string[]
  conflictPaths?: string[] | null
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
/**
 * T37: 저장된 username 으로 GitHub 에 momso-<username> 비공개 저장소를 만들어 연결한다(archive-backend-port-notes.md 264줄).
 * state: created | created_not_connected | created_unverified | create_failed | username_missing | invalid_username | already_connected
 *        | gh_missing | signed_out | git_missing | unavailable | exists_public | exists_empty | exists_matching | exists_private_unknown
 */
/**
 * Git 설치(B12, git_tool.rs): git_install_status() → installed | inProgress | approvalRequired | unavailable.
 * approvalRequired 일 때 사람이 동의하면 git_install({ approved: true }) → 바로 inProgress, 진행과 결과는 이벤트
 * git-install-changed { provider: 'git', state: inProgress|installed|installFailed, stage, completed, total(모르면 null) }.
 * stage: downloading, verifying, extracting, initializing, complete 또는 실패 값(networkFailed, checksumMismatch, sizeLimit 등).
 * 이 PC 에 쓸 수 있는 Git 이 있으면 그것을 먼저 쓰고, 없으면 공식 배포판을 받아 확인한 뒤 몸소 자료 폴더 안에 둔다.
 */
export type GitInstallState = 'installed' | 'inProgress' | 'approvalRequired' | 'unavailable'
export type GitInstallEvent = { state: 'inProgress' | 'installed' | 'installFailed' | string; stage: string; completed: number; total: number | null }
export async function gitInstallStatus() { need(); return invoke<GitInstallState>('git_install_status') }
export async function gitInstall() { need(); return invoke<GitInstallState>('git_install', { approved: true }) }
export async function listenGitInstall(onChange: (event: GitInstallEvent) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<GitInstallEvent>('git-install-changed', (event) => onChange(event.payload))
}

export async function remoteCreate() { need(); return invoke<{ state: string; repo: string | null }>('archive_remote_create') }

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

/** 백업이 실패했을 때(object-store-failed { file, state }). state 는 실패 까닭 값(B.reason 의 열쇠). */
export async function listenStoreFailed(onFailed: (state: string | null) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<{ file?: string | null; state?: string | null }>('object-store-failed', (event) => onFailed(event.payload?.state ?? null))
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
