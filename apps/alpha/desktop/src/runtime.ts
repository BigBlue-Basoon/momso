import { W } from './words'
import { invoke } from '@tauri-apps/api/core'
import { openPath } from '@tauri-apps/plugin-opener'
import type { AiConversation, AiConversationPreview, AiProvider } from './ai-conversation-import'

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

type BackupInfo = {
  path: string
}

export type ExternalProviderId = 'codex' | 'claude' | 'github' | 'neon' | 'cloudflare'

export type ExternalToolStatus = {
  id: ExternalProviderId
  label: string
  installed: boolean
  authenticated: boolean
  version: string | null
  canInstall: boolean
}

export type ConnectionOverview = {
  tools: ExternalToolStatus[]
  openaiApiKeyConfigured: boolean
  anthropicApiKeyConfigured: boolean
}

export type TranscriptionTerm = {
  id: string
  heardAs: string
  correctedTo: string
  note: string | null
  enabled: boolean
  createdAt: string
  updatedAt: string
}

export type CreateTranscriptionTermInput = {
  heardAs: string
  correctedTo: string
  note?: string
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

export type BackupEvidenceState =
  | 'not_configured'
  | 'no_recorded_success'
  | 'recorded_success'
  | 'inconsistent'

export type BackupEvidenceLane = {
  state: BackupEvidenceState
  snapshotId: string | null
  recordedAt: string | null
}

export type UserOwnedBackupEvidence = {
  workspaceId: string
  evidenceScope: 'local_record_only'
  currentRemoteAvailabilityChecked: false
  settingsBackup: BackupEvidenceLane
  encryptedRecordsBackup: BackupEvidenceLane
  connectionMapBackup: BackupEvidenceLane
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

export type SourceAssetQuarantineProposal = {
  assetId: string
  actor: string
}

export type SourceAssetQuarantinePlan = {
  planDigest: string
  assetId: string
  issue: 'missing' | 'changed' | 'unsafe_path' | 'unreadable'
  expectedContentSha256: string
  expectedByteLength: number
  requiresConfirmation: true
}

export type HashBoundTextPage = {
  contentSha256: string
  byteLength: number
  cursor: number
  nextCursor: number | null
  content: string
}

export type SourceTextPage = HashBoundTextPage & {
  assetId: string
}

export type RecordBodyPage = Omit<HashBoundTextPage, 'contentSha256' | 'byteLength'> & {
  recordId: string
  version: number
  bodySha256: string
  bodyBytes: number
}

export type LocalRecordStatus = 'draft' | 'needs_review' | 'reviewed'

export type CreateRecordInput = {
  recordKind: string
  title: string
  body: string
  summary: string | null
  actor: string
  initialStatus?: Exclude<LocalRecordStatus, 'reviewed'>
}

export type LocalRecordSummary = {
  id: string
  recordKind: string
  title: string
  status: LocalRecordStatus
  currentVersion: number
  summary: string | null
  summaryState: 'missing_summary' | 'unconfirmed' | 'fresh' | 'stale' | 'body_integrity_error'
  bodyBytes: number
  updatedAt: string
}

export type LocalRecordReadinessReport = {
  records: LocalRecordSummary[]
  metadataReadCount: number
  bodyReadCount: number
}

export type LocalRecordDetail = LocalRecordSummary & {
  body: string
  bodySha256: string
  createdBy: string
  updatedBy: string
  reviewedBy: string | null
  reviewedAt: string | null
  createdAt: string
}

export type RecordChangeProposal = {
  recordId: string
  expectedVersion: number
  recordKind: string
  title: string
  body: string
  summary: string | null
  targetStatus: LocalRecordStatus
  actor: string
}

export type RecordChangePlan = {
  recordId: string
  currentVersion: number
  nextVersion: number
  changedFields: string[]
  targetStatus: LocalRecordStatus
  summaryStateAfter: LocalRecordSummary['summaryState']
  bodyBytesAfter: number
  requiresConfirmation: boolean
  planDigest: string
}

export type SourceAssetAuditResult = {
  totalAssets: number
  totalObjectFiles: number
  healthyAssets: number
  missingAssets: number
  changedAssets: number
  unsafePathAssets: number
  unreadableAssets: number
  unregisteredFiles: number
  issueSamples: Array<{
    assetId: string
    issue: 'missing' | 'changed' | 'unsafe_path' | 'unreadable'
  }>
  samplesTruncated: boolean
}

export type OperationState = 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted'

export type OperationStatus<TResult = unknown> = {
  id: string
  operationKind: string
  state: OperationState
  stage: string
  completedUnits: number
  totalUnits: number
  result: TResult | null
  resultSha256: string | null
  errorCode: string | null
  startedAt: string
  heartbeatAt: string
  completedAt: string | null
}

export type UserOwnedBackupResult = {
  snapshotId: string
  encryptedArchiveSha256: string
  recoveryKey: string | null
}

export type UserOwnedBackupCandidate = {
  repository: string
  workspaceId: string
  r2Bucket: string
  neonOrganizationId: string | null
  neonProjectId: string
  neonBranchId: string
  neonDatabaseName: string
  neonRoleName: string
  updatedAt: string
}

export type UserOwnedRestorePlan = {
  planDigest: string
  remoteSnapshot: RemoteBackupSnapshot
  localContentSha256: string
  syncState: BackupSyncState
  requiresOverwriteConfirmation: boolean
}

export type UserOwnedRestoreResult = {
  restoredSnapshotId: string
  safetyBackupPath: string
}

export type AiAssistantRequest = {
  requestId: string
  provider: 'codex' | 'claude'
  authMode: 'subscription' | 'api_key'
  workflow: 'ask' | 'draft' | 'review' | 'vocabulary'
  responseDensity: 'concise' | 'balanced' | 'detailed'
  prompt: string
  recordContext: string | null
}

export type AiAssistantResponse = {
  text: string
  provider: 'codex' | 'claude'
  authMode: 'subscription' | 'api_key'
  usage: AiUsageSummary | null
}

export type AiUsageSummary = {
  provider: 'codex' | 'claude'
  authMode: 'subscription' | 'api_key'
  model: string | null
  inputTokens: number
  cachedInputTokens: number
  cacheCreationInputTokens: number
  outputTokens: number
  reasoningOutputTokens: number
  totalTokens: number
  estimatedCostUsdMicros: number | null
  costSource: 'provider_estimate' | 'local_price_table' | null
}

export type AiQuotaWindow = {
  id: string
  label: string
  usedPercent: number
  windowDurationMins: number | null
  resetsAt: number | null
}

export type AiQuotaOverview = {
  status: 'available' | 'not_applicable' | 'unavailable' | 'restricted'
  planType: string | null
  windows: AiQuotaWindow[]
  checkedAt: number | null
  detail: string | null
}

export type AiUsageOverview = {
  provider: 'codex' | 'claude'
  authMode: 'subscription' | 'api_key'
  lastRequest: AiUsageSummary | null
  todayEstimatedCostUsdMicros: number
  monthEstimatedCostUsdMicros: number
  todayTotalTokens: number
  monthTotalTokens: number
  monthRequestCount: number
  quota: AiQuotaOverview
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

export async function createRuntimeBackup() {
  if (!isDesktopRuntime()) throw new Error('Desktop runtime is not active')
  const backup = await invoke<BackupInfo>('create_data_backup')
  return backup.path
}

export async function discoverLocalAiConversations(provider: AiProvider) {
  if (!isDesktopRuntime()) throw new Error('local conversation search needs the desktop runtime')
  return invoke<AiConversationPreview[]>('discover_local_ai_conversations', { provider })
}

export async function loadLocalAiConversations(provider: AiProvider, sourceLocators: string[]) {
  if (!isDesktopRuntime()) throw new Error('local conversation read needs the desktop runtime')
  return invoke<AiConversation[]>('load_local_ai_conversations', { provider, sourceLocators })
}

export async function getConnectionOverview() {
  if (!isDesktopRuntime()) {
    return {
      tools: [
        { id: 'codex', label: 'Codex', installed: false, authenticated: false, version: null, canInstall: true },
        { id: 'claude', label: 'Claude', installed: false, authenticated: false, version: null, canInstall: true },
        { id: 'github', label: 'GitHub', installed: false, authenticated: false, version: null, canInstall: true },
        { id: 'neon', label: 'Neon', installed: false, authenticated: false, version: null, canInstall: true },
        { id: 'cloudflare', label: 'Cloudflare', installed: false, authenticated: false, version: null, canInstall: true },
      ],
      openaiApiKeyConfigured: false,
      anthropicApiKeyConfigured: false,
    } satisfies ConnectionOverview
  }
  return invoke<ConnectionOverview>('connection_overview')
}

export async function getAiConnectionOverview() {
  if (!isDesktopRuntime()) {
    return {
      tools: [
        { id: 'codex', label: 'Codex', installed: false, authenticated: false, version: null, canInstall: true },
        { id: 'claude', label: 'Claude', installed: false, authenticated: false, version: null, canInstall: true },
      ],
      openaiApiKeyConfigured: false,
      anthropicApiKeyConfigured: false,
    } satisfies ConnectionOverview
  }
  return invoke<ConnectionOverview>('ai_connection_overview')
}

export async function connectExternalProvider(provider: ExternalProviderId) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text7)
  return invoke<ExternalToolStatus>('connect_external_provider', { provider })
}

export async function installExternalProvider(provider: ExternalProviderId) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text8)
  return invoke<ExternalToolStatus>('install_external_provider', { provider })
}

export async function storeAiApiKey(provider: 'openai' | 'anthropic', apiKey: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text9)
  await invoke('save_ai_api_key', { provider, apiKey })
}

export async function removeAiApiKey(provider: 'openai' | 'anthropic') {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text10)
  await invoke('delete_ai_api_key', { provider })
}

export async function listTranscriptionTerms() {
  if (!isDesktopRuntime()) return [] satisfies TranscriptionTerm[]
  return invoke<TranscriptionTerm[]>('list_transcription_terms')
}

export async function createTranscriptionTerm(input: CreateTranscriptionTermInput) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text11)
  return invoke<TranscriptionTerm>('create_transcription_term', { input })
}

export async function setTranscriptionTermEnabled(termId: string, enabled: boolean) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text12)
  return invoke<TranscriptionTerm>('set_transcription_term_enabled', { termId, enabled })
}

export async function deleteTranscriptionTerm(termId: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text12)
  await invoke('delete_transcription_term', { termId })
}

export async function getUserOwnedBackupOverview() {
  if (!isDesktopRuntime()) return null
  return invoke<UserOwnedBackupOverview>('user_owned_backup_overview')
}

export async function getUserOwnedBackupEvidence() {
  if (!isDesktopRuntime()) return null
  return invoke<UserOwnedBackupEvidence>('user_owned_backup_evidence')
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

export async function planSourceAssetQuarantine(proposal: SourceAssetQuarantineProposal) {
  if (!isDesktopRuntime()) throw new Error('source quarantine review needs the desktop runtime')
  return invoke<SourceAssetQuarantinePlan>('plan_source_asset_quarantine', { proposal })
}

export async function applySourceAssetQuarantine(
  proposal: SourceAssetQuarantineProposal,
  planDigest: string,
  quarantineConfirmed: boolean,
) {
  if (!isDesktopRuntime()) throw new Error('source quarantine needs the desktop runtime')
  return invoke<SourceAsset>('apply_source_asset_quarantine', {
    input: { proposal, planDigest, quarantineConfirmed },
  })
}

export async function listSourceAssets(recordId?: string) {
  if (!isDesktopRuntime()) return [] satisfies SourceAsset[]
  return invoke<SourceAsset[]>('list_source_assets', { recordId: recordId ?? null })
}

export async function readSourceTextPage(
  assetId: string,
  cursor: number,
  expectedContentSha256?: string,
  maxBytes = 32 * 1024,
) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text13)
  return invoke<SourceTextPage>('read_source_text_page', {
    assetId,
    cursor,
    expectedContentSha256: expectedContentSha256 ?? null,
    maxBytes,
  })
}

export async function readRecordBodyPage(
  recordId: string,
  version: number | null,
  cursor: number,
  expectedBodySha256?: string,
  maxBytes = 32 * 1024,
) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text14)
  return invoke<RecordBodyPage>('read_record_body_page', {
    recordId,
    version,
    cursor,
    expectedBodySha256: expectedBodySha256 ?? null,
    maxBytes,
  })
}

export async function createRecord(input: CreateRecordInput) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text15)
  return invoke<LocalRecordDetail>('create_record_draft', { input })
}

export async function listRecordSummaries() {
  if (!isDesktopRuntime()) {
    return {
      records: [],
      metadataReadCount: 0,
      bodyReadCount: 0,
    } satisfies LocalRecordReadinessReport
  }
  return invoke<LocalRecordReadinessReport>('list_record_summaries')
}

export async function getRecordDetail(recordId: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text16)
  return invoke<LocalRecordDetail>('get_record_detail', { recordId })
}

export async function planRecordChange(proposal: RecordChangeProposal) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text17)
  return invoke<RecordChangePlan>('plan_record_change', { proposal })
}

export async function applyRecordChange(
  proposal: RecordChangeProposal,
  planDigest: string,
  reviewer: string,
) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text18)
  return invoke<LocalRecordDetail>('apply_record_change', {
    input: {
      proposal,
      planDigest,
      reviewer,
      reviewConfirmed: true,
    },
  })
}

export async function startSourceAssetAudit() {
  if (!isDesktopRuntime()) throw new Error('source check needs the desktop runtime')
  return invoke<OperationStatus<SourceAssetAuditResult>>('start_source_asset_audit')
}

export async function getOperationStatus<TResult = unknown>(operationId: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text19)
  return invoke<OperationStatus<TResult>>('get_operation_status', { operationId })
}

export async function cancelOperation(operationId: string) {
  if (!isDesktopRuntime()) return
  await invoke('cancel_operation', { operationId })
}

export async function prepareUserOwnedBackup(selectedNeonOrganizationId?: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text20)
  return invoke<UserOwnedBackupOverview>('prepare_user_owned_backup', {
    selectedNeonOrganizationId: selectedNeonOrganizationId ?? null,
  })
}

export async function discoverUserOwnedBackups() {
  if (!isDesktopRuntime()) return [] satisfies UserOwnedBackupCandidate[]
  return invoke<UserOwnedBackupCandidate[]>('discover_user_owned_backups')
}

export async function adoptUserOwnedBackup(repository: string) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text21)
  return invoke<UserOwnedBackupOverview>('adopt_user_owned_backup', { repository })
}

export async function runUserOwnedBackup() {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text22)
  return invoke<UserOwnedBackupResult>('run_user_owned_backup')
}

export async function startUserOwnedBackup() {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text22)
  return invoke<OperationStatus<UserOwnedBackupResult>>('start_user_owned_backup')
}

export async function planUserOwnedRestore() {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text23)
  return invoke<UserOwnedRestorePlan>('plan_user_owned_restore')
}

export async function applyUserOwnedRestore(planDigest: string, confirmOverwrite: boolean) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text23)
  return invoke<UserOwnedRestoreResult>('apply_user_owned_restore', { planDigest, confirmOverwrite })
}

export async function startUserOwnedRestore(planDigest: string, confirmOverwrite: boolean) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text23)
  return invoke<OperationStatus<UserOwnedRestoreResult>>('start_user_owned_restore', {
    planDigest,
    confirmOverwrite,
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

export async function runAiAssistant(request: AiAssistantRequest) {
  if (!isDesktopRuntime()) throw new Error(W.runtime.text26)
  return invoke<AiAssistantResponse>('run_ai_assistant', { request })
}

export async function getAiUsageOverview(
  provider: AiAssistantRequest['provider'],
  authMode: AiAssistantRequest['authMode'],
) {
  if (!isDesktopRuntime()) {
    return {
      provider,
      authMode,
      lastRequest: null,
      todayEstimatedCostUsdMicros: 0,
      monthEstimatedCostUsdMicros: 0,
      todayTotalTokens: 0,
      monthTotalTokens: 0,
      monthRequestCount: 0,
      quota: {
        status: 'not_applicable',
        planType: null,
        windows: [],
        checkedAt: null,
        detail: null,
      },
    } satisfies AiUsageOverview
  }
  return invoke<AiUsageOverview>('ai_usage_overview', { provider, authMode })
}

export async function cancelAiAssistant(requestId: string) {
  if (!isDesktopRuntime()) return
  await invoke('cancel_ai_assistant', { requestId })
}
