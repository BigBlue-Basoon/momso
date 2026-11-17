import { W } from './words'
import { invoke } from '@tauri-apps/api/core'
import { openPath } from '@tauri-apps/plugin-opener'

export type RuntimeMode = 'starting' | 'ready' | 'preview' | 'error'

export type RuntimeInfo = {
  appVersion: string
  platform: string
  dataRoot: string
  databasePath: string
  objectsPath: string
  exportsPath: string
  backupsPath: string
  logsPath: string
  schemaVersion: number
  databaseHealth: 'ok'
  womAdapterStatus: 'waiting_for_stable_contract'
}

export type RuntimeSession = {
  mode: RuntimeMode
  info: RuntimeInfo | null
  error: string | null
}

export type BackupSyncState =
  | 'needs_setup'
  | 'ready_first_backup'
  | 'current'
  | 'local_changes'
  | 'remote_changes'
  | 'remote_older'
  | 'diverged'
  | 'unavailable'

export type BackupProfile = {
  workspaceId: string
  githubRepository: string | null
  r2Bucket: string | null
  neonOrganizationId: string | null
  neonProjectId: string | null
  neonBranchId: string | null
  neonDatabaseName: string | null
  neonRoleName: string | null
  lastCompletedSnapshotId: string | null
  lastCompletedContentSha256: string | null
  recoveryKeyConfirmedAt: string | null
}

export type RemoteBackupSnapshot = {
  snapshotId: string
  parentSnapshotId: string | null
  contentSha256: string
  encryptedArchiveSha256: string
  manifestSha256: string
  githubCommitSha: string
  r2ObjectKey: string
  recordCount: number
  objectCount: number
  createdAt: string
}

export type NeonOrganization = {
  id: string
  name: string
}

export type UserOwnedBackupOverview = {
  profile: BackupProfile
  syncState: BackupSyncState
  localContentSha256: string
  recordCount: number
  objectCount: number
  remoteSnapshot: RemoteBackupSnapshot | null
  recoveryKeyStored: boolean
  recoveryKeyConfirmed: boolean
  neonOrganizations: NeonOrganization[]
  neonOrganizationSelectionRequired: boolean
}

export type SourceAssetKind =
  | 'audio_recording'
  | 'raw_transcript'
  | 'reference_document'
  | 'generated_report'
  | 'other'

export type SourceAssetRelationRole = 'source' | 'supporting' | 'derived'

export type SourceAssetRecordLink = {
  recordId: string
  recordVersion: number
  relationRole: SourceAssetRelationRole
}

export type SourceAssetImportProposal = {
  sourcePath: string
  assetKind: SourceAssetKind
  mediaType?: string | null
  recordLink?: SourceAssetRecordLink | null
  actor: string
}

export type SourceAssetImportPlan = {
  planDigest: string
  originalFileName: string
  assetKind: SourceAssetKind
  contentSha256: string
  byteLength: number
  alreadyStored: boolean
  recordLink: SourceAssetRecordLink | null
  requiresConfirmation: true
}

export type SourceAsset = {
  id: string
  assetKind: SourceAssetKind
  contentSha256: string
  byteLength: number
  originalFileName: string
  mediaType: string | null
  capturedBy: string
  createdAt: string
  integrityState: 'active' | 'quarantined'
  integrityIssue: 'missing' | 'changed' | 'unsafe_path' | 'unreadable' | null
  quarantinedAt: string | null
  quarantinedBy: string | null
  links: SourceAssetRecordLink[]
}

const PREVIEW_INFO: RuntimeInfo = {
  appVersion: '0.1.0',
  platform: 'Browser',
  dataRoot: W.runtime.text1,
  databasePath: W.runtime.text2,
  objectsPath: W.runtime.text3,
  exportsPath: W.runtime.text4,
  backupsPath: W.runtime.text5,
  logsPath: W.runtime.text6,
  schemaVersion: 7,
  databaseHealth: 'ok',
  womAdapterStatus: 'waiting_for_stable_contract',
}

export function isDesktopRuntime() {
  return '__TAURI_INTERNALS__' in window
}

export async function initializeRuntime(): Promise<RuntimeSession> {
  if (!isDesktopRuntime()) {
    return { mode: 'preview', info: PREVIEW_INFO, error: null }
  }

  try {
    const info = await invoke<RuntimeInfo>('runtime_info')
    return { mode: 'ready', info, error: null }
  } catch (error) {
    return {
      mode: 'error',
      info: null,
      error: error instanceof Error ? error.message : String(error),
    }
  }
}

export async function openRuntimeDataRoot(dataRoot: string) {
  if (!isDesktopRuntime()) return
  await openPath(dataRoot)
}

export async function getUserOwnedBackupOverview() {
  if (!isDesktopRuntime()) return null
  return invoke<UserOwnedBackupOverview>('user_owned_backup_overview')
}

export async function planSourceAssetImport(proposal: SourceAssetImportProposal) {
  if (!isDesktopRuntime()) throw new Error('source import needs the desktop runtime')
  return invoke<SourceAssetImportPlan>('plan_source_asset_import', { proposal })
}

export async function applySourceAssetImport(
  proposal: SourceAssetImportProposal,
  planDigest: string,
  importConfirmed: boolean,
) {
  if (!isDesktopRuntime()) throw new Error('source import needs the desktop runtime')
  return invoke<SourceAsset>('apply_source_asset_import', {
    input: { proposal, planDigest, importConfirmed },
  })
}

export async function exportBackupRecoveryKey() {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text24)
  return invoke<string>('export_backup_recovery_key')
}

export async function importBackupRecoveryKey(recoveryKey: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text25)
  await invoke('import_backup_recovery_key', { recoveryKey })
}

export async function confirmBackupRecoveryKey(recoveryKey: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text24)
  await invoke('confirm_backup_recovery_key', { recoveryKey })
}

/**
 * 파일·폴더 고르기(2026-11-17 뒤쪽 e9b6a27b, desktop-selection-opening-contract-2026-11-17.md). 고르는 창은 뒤쪽이 연다.
 *   select_local_paths({ purpose: 'source' | 'conversation' | 'archive' }) → 고른 경로의 배열(취소하면 빈 배열).
 *   뒤쪽은 고른 경로와 쓸 곳을 5분 동안 기억하고, 그 뒤의 명령(첨부·가져오기·폴더 옮기기)이 성공했을 때만 지운다.
 *   고르지 않았거나 5분이 지난 경로는 selection_required 로 거절한다(실패 뒤 다시 시도는 5분 안이면 다시 고르지 않아도 된다).
 */
export type SelectionPurpose = 'source' | 'conversation' | 'archive'
export async function selectLocalPaths(purpose: SelectionPurpose): Promise<string[]> {
  if (!isDesktopRuntime()) return []
  const value = await invoke<unknown>('select_local_paths', { purpose })
  return Array.isArray(value) ? value.filter((path): path is string => typeof path === 'string' && path.length > 0) : []
}

/** 고른 지 5분이 지나 뒤쪽이 거절한 것인가(selection_required). 그때는 "파일을 다시 선택해 주세요". */
export function isSelectionExpired(error: unknown) {
  return String(error instanceof Error ? error.message : error).includes('selection_required')
}
