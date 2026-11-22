// 몸소 폴더(문서, 원자료, 작업 내역이 들어 있는 폴더)의 위치. 백엔드 src-tauri/src/archive_location.rs(2026-11-16 화면 폴더에 옮겨짐):
//   archive_location_get → { path, isDefault, onCloudSyncFolder }  onCloudSyncFolder: true(OneDrive 같은 자동 올리기 폴더) / false / null(확인 못 함)
//   archive_location_set({ path }) → { state, path, previousPath, previousKept }  옮기기가 끝날 때까지 기다린다. 옮기는 동안 다른 쓰기는 잠긴다.
//     state: ok(옮김) / unchanged(지금 자리와 같음) / invalid(옮길 수 없는 자리) / target_not_empty(고른 폴더가 비어 있지 않음) / failed
//     path 는 옮긴 뒤(또는 그대로인) 자리, previousPath·previousKept 는 실제로 옮겼을 때만: 예전 자리와 그 자리가 남아 있는지. 백엔드는 예전 폴더를 지우지 않는다.
//     옛 앱 데이터의 archive 자리(와 그 안·위)로는 코드가 일부러 invalid 로 막는다(까닭은 기록에 없음).
//   이벤트: archive-location-progress { copiedBytes, totalBytes }(전체를 모르면 totalBytes null),
//          archive-location-done / archive-location-failed(archive_location_set 이 돌려주는 것과 같은 결과).
//   archive_location_open → null(실패하면 오류)
import { invoke, listen } from './host'
import { isDesktopRuntime } from './runtime'

export type FolderLocation = { path: string; isDefault: boolean; onCloudSyncFolder: boolean | null }
export type FolderMoveState = 'ok' | 'unchanged' | 'invalid' | 'target_not_empty' | 'failed'
export type FolderMoveResult = { state: FolderMoveState; path: string | null; previousPath: string | null; previousKept: boolean | null }
export type FolderMoveProgress = { copiedBytes: number; totalBytes: number | null }

export async function getFolderLocation() {
  if (!isDesktopRuntime()) throw new Error('archive_location_get needs the desktop runtime')
  return invoke<FolderLocation>('archive_location_get')
}

function readMoveResult(result: Partial<FolderMoveResult> | null | undefined): FolderMoveResult {
  return {
    state: (result?.state ?? 'failed') as FolderMoveState,
    path: result?.path ?? null,
    previousPath: result?.previousPath ?? null,
    previousKept: typeof result?.previousKept === 'boolean' ? result.previousKept : null,
  }
}

// 옮기는 중인지(앱 전체에서 하나). 설정 화면을 떠났다 돌아와도 옮기는 중이면 막대가 다시 보이게 한다.
let movingNow = false
export function isFolderMoving() { return movingNow }

export async function setFolderLocation(path: string): Promise<FolderMoveResult> {
  if (!isDesktopRuntime()) throw new Error('archive_location_set needs the desktop runtime')
  movingNow = true
  try {
    return readMoveResult(await invoke<Partial<FolderMoveResult> | null>('archive_location_set', { path }))
  } finally {
    movingNow = false
  }
}

/**
 * 옮기기가 끝났을 때(archive-location-done / archive-location-failed, archive_location_set 의 결과와 같은 모양).
 * 옮기는 도중에 설정 화면을 떠났다 돌아와도 화면이 끝을 알 수 있게 듣는다.
 */
export async function listenFolderMoveEnd(onEnd: (result: FolderMoveResult) => void) {
  if (!isDesktopRuntime()) return () => {}
  const stops = await Promise.all([
    listen<Partial<FolderMoveResult>>('archive-location-done', (event) => onEnd(readMoveResult(event.payload))),
    listen<Partial<FolderMoveResult>>('archive-location-failed', (event) => onEnd(readMoveResult(event.payload))),
  ])
  return () => stops.forEach((stop) => stop())
}

/** 옮기는 동안 온 양. 끝은 setFolderLocation 의 결과로 안다. */
export async function listenFolderMove(onProgress: (progress: FolderMoveProgress) => void) {
  if (!isDesktopRuntime()) return () => {}
  return listen<FolderMoveProgress>('archive-location-progress', (event) => onProgress(event.payload))
}

/** 옮긴 비율(0~1). 전체를 모르면 null(비율을 지어내지 않는다). */
export function folderMoveRatio(progress: FolderMoveProgress | null) {
  const total = progress?.totalBytes
  if (!progress || typeof total !== 'number' || total <= 0) return null
  return Math.min(1, Math.max(0, progress.copiedBytes / total))
}

export async function openFolderLocation() {
  if (!isDesktopRuntime()) throw new Error('archive_location_open needs the desktop runtime')
  await invoke<null>('archive_location_open')
}
