// 앱 버전과 업데이트. 계약: momso-desktop-backend/outputs/2026-11-15-momso-app-update-contract.md
//   app_version → { version, buildDate }
//   update_check → { state: up_to_date | available | unavailable | failed, currentVersion, latestVersion, notes, publishedAt }
//     unavailable = 업데이트 주소·키가 설정되지 않음(화면은 업데이트 쪽을 눌리지 않게).
//     no_release = 공개된 릴리스가 아직 없음(2026-11-19 첫째 줄 M16 D40, 오류가 아님). 화면은 up_to_date 와 같게 "최신 버전입니다"로 본다.
//   update_install → { state: downloaded | failed | unavailable | busy } (내려받고 서명 검증까지, 설치는 안 함)
//   update_restart → { state } (화면이 미저장·AI 작업 확인을 마친 뒤)
//     S69(줄6 cc9e65e79, desktop-update-s69.md): 진행 중인 채팅이 있으면 { state: 'busy', reason: 'active_turn' } 로 설치하지 않는다(화면은 알리고 일이 끝나면 다시 묻는다).
//     다른 업데이트 작업 중이면 busy(까닭 없음). 실패 까닭은 backup_failed · installation_failed · worker_failed. 정상이면 앱이 꺼져 결과가 오지 않을 수 있다.
//   이벤트 update-available(update_check 와 같은 모양), update-progress { downloadedBytes, totalBytes },
//          update-done { state: 'downloaded' }, update-failed { state: 'failed' }
import { invoke, listen } from './host'
import { isDesktopRuntime } from './runtime'

export type AppVersion = { version: string; buildDate: string | null }

export type UpdateInfo = {
  state: 'up_to_date' | 'available' | 'unavailable' | 'failed'
  currentVersion: string | null
  latestVersion: string | null
  notes: string | null
  publishedAt: string | null
}

export async function getAppVersion() {
  if (!isDesktopRuntime()) throw new Error('app_version needs the desktop runtime')
  return invoke<AppVersion>('app_version')
}

export async function checkUpdate() {
  if (!isDesktopRuntime()) throw new Error('update_check needs the desktop runtime')
  const info = await invoke<Omit<UpdateInfo, 'state'> & { state: UpdateInfo['state'] | 'no_release' }>('update_check')
  // 공개된 릴리스가 없으면 오류가 아니라 지금 판이 최신인 것으로 본다(D40).
  return (info.state === 'no_release' ? { ...info, state: 'up_to_date' } : info) as UpdateInfo
}

export async function installUpdate() {
  if (!isDesktopRuntime()) throw new Error('update_install needs the desktop runtime')
  return invoke<{ state?: string } | null>('update_install')
}

export type RestartResult = { state: string | null; reason: string | null }

export async function restartApp(): Promise<RestartResult> {
  if (!isDesktopRuntime()) throw new Error('update_restart needs the desktop runtime')
  const result = await invoke<{ state?: unknown; reason?: unknown } | null>('update_restart')
  return { state: typeof result?.state === 'string' ? result.state : null, reason: typeof result?.reason === 'string' ? result.reason : null }
}

const RESTART_FAILURES = ['backup_failed', 'installation_failed', 'worker_failed']

/** 다시 시작이 되지 않은 까닭: 진행 중인 채팅(turn), 다른 업데이트 작업(busy), 실패(failed). 그 밖(시작됨 등)은 null. */
export function restartRefusal(result: RestartResult): 'turn' | 'busy' | 'failed' | null {
  if (result.state === 'busy') return result.reason === 'active_turn' ? 'turn' : 'busy'
  if (result.state === 'failed' || (result.reason !== null && RESTART_FAILURES.includes(result.reason))) return 'failed'
  return null
}

export async function listenUpdates(handlers: {
  onAvailable: (info: Partial<UpdateInfo>) => void
  onProgress: (percent: number | null) => void
  onDone: () => void
  onFailed: (reason: string | null) => void
}) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<Partial<UpdateInfo>>('update-available', (event) => handlers.onAvailable(event.payload ?? {})),
    listen<{ downloadedBytes?: number | null; totalBytes?: number | null }>('update-progress', (event) => {
      const done = event.payload?.downloadedBytes ?? null
      const total = event.payload?.totalBytes ?? null
      handlers.onProgress(done !== null && total ? Math.min(100, (done / total) * 100) : null)
    }),
    listen('update-done', () => handlers.onDone()),
    // 실패 이벤트에는 사람이 읽는 까닭이 없다(state 만). 화면은 정해 둔 한 줄을 보인다.
    listen('update-failed', () => handlers.onFailed(null)),
  ])
  return () => stops.forEach((stop) => stop())
}
