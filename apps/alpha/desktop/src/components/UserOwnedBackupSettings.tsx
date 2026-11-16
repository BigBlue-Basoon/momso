import { W } from '../words'
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Check,
  Cloud,
  Copy,
  Database,
  Download,
  GitBranch,
  KeyRound,
  Link2,
  LoaderCircle,
  RefreshCw,
  Search,
  ShieldAlert,
  UploadCloud,
  Wrench,
  X,
} from 'lucide-react'
import {
  adoptUserOwnedBackup,
  cancelOperation,
  confirmBackupRecoveryKey,
  connectExternalProvider,
  discoverUserOwnedBackups,
  exportBackupRecoveryKey,
  getConnectionOverview,
  getOperationStatus,
  getUserOwnedBackupOverview,
  importBackupRecoveryKey,
  installExternalProvider,
  planUserOwnedRestore,
  prepareUserOwnedBackup,
  startUserOwnedBackup,
  startUserOwnedRestore,
  type BackupSyncState,
  type ConnectionOverview,
  type ExternalProviderId,
  type OperationStatus,
  type UserOwnedBackupOverview,
  type UserOwnedBackupResult,
  type UserOwnedBackupCandidate,
  type UserOwnedRestoreResult,
  type UserOwnedRestorePlan,
} from '../runtime'

const BACKUP_TOOLS: Array<{
  id: ExternalProviderId
  label: string
  detail: string
  icon: typeof GitBranch
}> = [
  { id: 'github', label: 'GitHub', detail: W.userOwnedBackupSettings.text1, icon: GitBranch },
  { id: 'cloudflare', label: 'Cloudflare R2', detail: W.userOwnedBackupSettings.text2, icon: Cloud },
  { id: 'neon', label: 'Neon', detail: W.userOwnedBackupSettings.text3, icon: Database },
]

const STATE_TEXT: Record<BackupSyncState, { label: string; detail: string; tone: string }> = {
  needs_setup: { label: W.userOwnedBackupSettings.text4, detail: W.userOwnedBackupSettings.text5, tone: 'neutral' },
  ready_first_backup: { label: W.userOwnedBackupSettings.text6, detail: W.userOwnedBackupSettings.text7, tone: 'ready' },
  current: { label: W.userOwnedBackupSettings.text8, detail: W.userOwnedBackupSettings.text9, tone: 'ready' },
  local_changes: { label: W.userOwnedBackupSettings.text10, detail: W.userOwnedBackupSettings.text11, tone: 'attention' },
  remote_changes: { label: W.userOwnedBackupSettings.text12, detail: W.userOwnedBackupSettings.text13, tone: 'attention' },
  remote_older: { label: W.userOwnedBackupSettings.text14, detail: W.userOwnedBackupSettings.text15, tone: 'danger' },
  diverged: { label: W.userOwnedBackupSettings.text16, detail: W.userOwnedBackupSettings.text17, tone: 'danger' },
  unavailable: { label: W.userOwnedBackupSettings.text18, detail: W.userOwnedBackupSettings.text19, tone: 'danger' },
}

const TRANSFER_STAGE_LABELS: Record<string, string> = {
  queued: W.userOwnedBackupSettings.text20,
  checking_local: W.userOwnedBackupSettings.text21,
  comparing_remote: W.userOwnedBackupSettings.text22,
  creating_archive: W.userOwnedBackupSettings.text23,
  uploading_archive: W.userOwnedBackupSettings.text24,
  writing_metadata: W.userOwnedBackupSettings.text25,
  revalidating_plan: W.userOwnedBackupSettings.text26,
  downloading_archive: W.userOwnedBackupSettings.text27,
  decrypting_archive: W.userOwnedBackupSettings.text28,
  verifying_content: W.userOwnedBackupSettings.text29,
  replacing_local_copy: W.userOwnedBackupSettings.text30,
  complete: W.userOwnedBackupSettings.text31,
  cancelled: W.userOwnedBackupSettings.text32,
  failed: W.userOwnedBackupSettings.text33,
}

type MessageArea = 'connections' | 'space' | 'transfer' | 'key'

export function UserOwnedBackupSettings() {
  const [connections, setConnections] = useState<ConnectionOverview | null>(null)
  const [overview, setOverview] = useState<UserOwnedBackupOverview | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  const [messageArea, setMessageArea] = useState<MessageArea>('connections')
  const [recoveryKey, setRecoveryKey] = useState<string | null>(null)
  const [recoveryKeyInput, setRecoveryKeyInput] = useState('')
  const [selectedOrganization, setSelectedOrganization] = useState('')
  const [restorePlan, setRestorePlan] = useState<UserOwnedRestorePlan | null>(null)
  const [confirmOverwrite, setConfirmOverwrite] = useState(false)
  const [backupCandidates, setBackupCandidates] = useState<UserOwnedBackupCandidate[] | null>(null)
  const [activeOperationId, setActiveOperationId] = useState<string | null>(null)
  const [transferStage, setTransferStage] = useState<string | null>(null)
  const [transferProgress, setTransferProgress] = useState({ completed: 0, total: 0 })

  const refresh = useCallback(async (area: MessageArea = 'connections') => {
    setMessage(null)
    try {
      const [nextConnections, nextOverview] = await Promise.all([
        getConnectionOverview(),
        getUserOwnedBackupOverview(),
      ])
      setConnections(nextConnections)
      setOverview(nextOverview)
      if (nextOverview?.neonOrganizations.length === 1) {
        setSelectedOrganization(nextOverview.neonOrganizations[0].id)
      }
    } catch (error) {
      setMessageArea(area)
      setMessage(error instanceof Error ? error.message : String(error))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const backupToolsReady = useMemo(() => BACKUP_TOOLS.every(({ id }) => (
    connections?.tools.find((tool) => tool.id === id)?.authenticated
  )), [connections])
  const githubReady = connections?.tools.find((tool) => tool.id === 'github')?.authenticated ?? false
  const stateText = overview ? STATE_TEXT[overview.syncState] : null
  const canBackUp = (overview?.syncState === 'ready_first_backup' || overview?.syncState === 'local_changes')
    && overview.recoveryKeyConfirmed
  const loadingConnections = connections === null

  async function waitForTransfer<TResult>(operationId: string) {
    setActiveOperationId(operationId)
    let consecutiveReadFailures = 0
    try {
      while (true) {
        let status: OperationStatus<TResult>
        try {
          status = await getOperationStatus<TResult>(operationId)
          consecutiveReadFailures = 0
        } catch (error) {
          consecutiveReadFailures += 1
          if (consecutiveReadFailures >= 6) throw error
          await new Promise((resolve) => window.setTimeout(resolve, 600))
          continue
        }
        setTransferStage(status.stage)
        setTransferProgress({ completed: status.completedUnits, total: status.totalUnits })
        if (status.state === 'succeeded' && status.result) return status.result
        if (status.state === 'cancelled') throw new Error(W.userOwnedBackupSettings.text34)
        if (status.state === 'failed' || status.state === 'interrupted') {
          throw new Error(W.userOwnedBackupSettings.text35)
        }
        await new Promise((resolve) => window.setTimeout(resolve, 600))
      }
    } finally {
      setActiveOperationId(null)
    }
  }

  async function cancelTransfer() {
    if (!activeOperationId) return
    await cancelOperation(activeOperationId)
    setMessageArea('transfer')
    setMessage(W.userOwnedBackupSettings.text36)
  }

  async function connectTool(provider: ExternalProviderId) {
    const tool = connections?.tools.find((candidate) => candidate.id === provider)
    setBusy(provider)
    setMessageArea('connections')
    setMessage(null)
    try {
      if (!tool?.installed) await installExternalProvider(provider)
      await connectExternalProvider(provider)
      await refresh()
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function prepareSpaces() {
    setBusy('setup')
    setMessageArea('space')
    setMessage(null)
    try {
      const next = await prepareUserOwnedBackup(selectedOrganization || undefined)
      setOverview(next)
      setBackupCandidates(null)
      await refresh('space')
      setMessage(W.userOwnedBackupSettings.text37)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function findExistingBackups() {
    setBusy('discover')
    setMessageArea('space')
    setMessage(null)
    try {
      const candidates = await discoverUserOwnedBackups()
      setBackupCandidates(candidates)
      if (candidates.length === 0) {
        setMessage(W.userOwnedBackupSettings.text38)
      }
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function connectExistingBackup(repository: string) {
    setBusy(`adopt:${repository}`)
    setMessageArea('space')
    setMessage(null)
    try {
      setOverview(await adoptUserOwnedBackup(repository))
      setBackupCandidates(null)
      await refresh('space')
      setMessage(W.userOwnedBackupSettings.text39)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function backUpNow() {
    setBusy('backup')
    setMessageArea('transfer')
    setMessage(null)
    setRestorePlan(null)
    try {
      const started = await startUserOwnedBackup()
      const result = await waitForTransfer<UserOwnedBackupResult>(started.id)
      if (result.recoveryKey) setRecoveryKey(result.recoveryKey)
      await refresh('transfer')
      setMessage(W.userOwnedBackupSettings.text40)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function inspectRestore() {
    setBusy('plan')
    setMessageArea('transfer')
    setMessage(null)
    try {
      setRestorePlan(await planUserOwnedRestore())
      setConfirmOverwrite(false)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function restoreNow() {
    if (!restorePlan) return
    setBusy('restore')
    setMessageArea('transfer')
    setMessage(null)
    try {
      const started = await startUserOwnedRestore(restorePlan.planDigest, confirmOverwrite)
      const result = await waitForTransfer<UserOwnedRestoreResult>(started.id)
      setRestorePlan(null)
      setConfirmOverwrite(false)
      await refresh('transfer')
      setMessage(W.userOwnedBackupSettings.text41(result.safetyBackupPath))
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function revealRecoveryKey() {
    setBusy('key')
    setMessageArea('key')
    setMessage(null)
    try {
      setRecoveryKey(await exportBackupRecoveryKey())
      await refresh('key')
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function importRecoveryKey() {
    setBusy('key-import')
    setMessageArea('key')
    setMessage(null)
    try {
      await importBackupRecoveryKey(recoveryKeyInput)
      setRecoveryKeyInput('')
      await refresh('key')
      setMessage(W.userOwnedBackupSettings.text42)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  async function confirmRecoveryKey() {
    setBusy('key-confirm')
    setMessageArea('key')
    setMessage(null)
    try {
      await confirmBackupRecoveryKey(recoveryKeyInput)
      setRecoveryKeyInput('')
      await refresh('key')
      setMessage(W.userOwnedBackupSettings.text43)
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(null)
    }
  }

  return (
    <>
      <section className="settings-group" aria-labelledby="backup-connections-title">
        <div className="settings-group-heading">
          <div>
            <h3 id="backup-connections-title">{W.userOwnedBackupSettings.text44}</h3>
            <p>{W.userOwnedBackupSettings.text45}</p>
          </div>
          <button className="icon-button subtle" type="button" onClick={() => void refresh()} aria-label={W.userOwnedBackupSettings.aria1} title={W.userOwnedBackupSettings.aria1}>
            <RefreshCw size={15} aria-hidden />
          </button>
        </div>
        <div className="connection-list">
          {BACKUP_TOOLS.map((item) => {
            const tool = connections?.tools.find((candidate) => candidate.id === item.id)
            const Icon = item.icon
            return (
              <div key={item.id} className="connection-action-row service-row">
                <span className="connection-icon" aria-hidden><Icon size={17} /></span>
                <div>
                  <strong>{item.label}</strong>
                  <p>{item.detail}</p>
                </div>
                <span className={`connection-state ${tool?.authenticated ? 'ready' : ''}`}>
                  {loadingConnections ? W.userOwnedBackupSettings.text46 : tool?.authenticated ? W.userOwnedBackupSettings.text47 : tool?.installed ? W.userOwnedBackupSettings.text48 : W.userOwnedBackupSettings.text49}
                </span>
                {!loadingConnections && !tool?.authenticated && (
                  <button className="secondary-button" type="button" disabled={busy !== null} onClick={() => void connectTool(item.id)}>
                    {busy === item.id ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <Wrench size={14} aria-hidden />}
                    {busy === item.id ? W.userOwnedBackupSettings.text50 : tool?.installed ? W.userOwnedBackupSettings.text51 : W.userOwnedBackupSettings.text52}
                  </button>
                )}
                {tool?.authenticated && <Check size={16} className="connection-check" aria-label={W.userOwnedBackupSettings.text47} />}
                {loadingConnections && <LoaderCircle size={15} className="is-spinning connection-checking" aria-label={W.userOwnedBackupSettings.aria2} />}
              </div>
            )
          })}
        </div>
        {messageArea === 'connections' && message && <div className="inline-status backup-message" role="status">{message}</div>}
      </section>

      <section className="settings-group" aria-labelledby="backup-space-title">
        <div className="settings-group-heading">
          <div>
            <h3 id="backup-space-title">{W.userOwnedBackupSettings.text53}</h3>
            <p>{W.userOwnedBackupSettings.text54}</p>
          </div>
          {overview?.syncState === 'needs_setup' && (
            <div className="inline-actions">
              <button className="secondary-button" type="button" disabled={!githubReady || busy !== null} onClick={() => void findExistingBackups()}>
                {busy === 'discover' ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <Search size={14} aria-hidden />}{W.userOwnedBackupSettings.text55}</button>
              <button className="confirm-button" type="button" disabled={!backupToolsReady || busy !== null || (overview.neonOrganizationSelectionRequired && !selectedOrganization)} onClick={() => void prepareSpaces()}>
                {busy === 'setup' ? <LoaderCircle size={15} className="is-spinning" aria-hidden /> : <Wrench size={15} aria-hidden />}{W.userOwnedBackupSettings.text56}</button>
            </div>
          )}
        </div>
        {overview?.neonOrganizationSelectionRequired && (
          <label className="setting-field compact-field">
            <span>{W.userOwnedBackupSettings.text57}</span>
            <select value={selectedOrganization} onChange={(event) => setSelectedOrganization(event.target.value)}>
              <option value="">{W.userOwnedBackupSettings.text58}</option>
              {overview.neonOrganizations.map((organization) => (
                <option key={organization.id} value={organization.id}>{organization.name}</option>
              ))}
            </select>
          </label>
        )}
        {!overview && (
          <div className="backup-loading-row" role="status">
            <LoaderCircle size={14} className="is-spinning" aria-hidden />{W.userOwnedBackupSettings.text59}</div>
        )}
        {overview?.profile.githubRepository && (
          <div className="backup-resource-list">
            <div><span>GitHub</span><code>{overview.profile.githubRepository}</code></div>
            <div><span>Cloudflare R2</span><code>{overview.profile.r2Bucket}</code></div>
            <div><span>Neon</span><code>{overview.profile.neonProjectId}</code></div>
          </div>
        )}
        {backupCandidates && backupCandidates.length > 0 && (
          <div className="backup-candidate-list" aria-label={W.userOwnedBackupSettings.aria3}>
            <p>{W.userOwnedBackupSettings.text60}</p>
            {backupCandidates.map((candidate) => (
              <div className="backup-candidate-row" key={candidate.workspaceId}>
                <span className="connection-icon" aria-hidden><GitBranch size={17} /></span>
                <div>
                  <strong>{candidate.repository}</strong>
                  <p>{W.userOwnedBackupSettings.text61(new Date(candidate.updatedAt).toLocaleString('ko-KR'))}</p>
                </div>
                <button className="secondary-button" type="button" disabled={!backupToolsReady || busy !== null} onClick={() => void connectExistingBackup(candidate.repository)}>
                  {busy === `adopt:${candidate.repository}` ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <Link2 size={14} aria-hidden />}{W.userOwnedBackupSettings.text62}</button>
              </div>
            ))}
          </div>
        )}
        {messageArea === 'space' && message && <div className="inline-status backup-message" role="status">{message}</div>}
      </section>

      <section className="settings-group" aria-labelledby="remote-backup-title">
        <div className="settings-group-heading">
          <div>
            <h3 id="remote-backup-title">{W.userOwnedBackupSettings.text63}</h3>
            <p>{W.userOwnedBackupSettings.text64}</p>
          </div>
          <div className="inline-actions">
            <button className="secondary-button" type="button" disabled={!overview?.remoteSnapshot || busy !== null} onClick={() => void inspectRestore()}>
              {busy === 'plan' ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <Download size={14} aria-hidden />}{W.userOwnedBackupSettings.text65}</button>
            <button className="confirm-button" type="button" disabled={!canBackUp || busy !== null} onClick={() => void backUpNow()}>
              {busy === 'backup' ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <UploadCloud size={15} aria-hidden />}{W.userOwnedBackupSettings.text66}</button>
          </div>
        </div>

        {stateText && (
          <div className={`backup-state-panel ${stateText.tone}`}>
            {stateText.tone === 'danger' ? <ShieldAlert size={18} aria-hidden /> : <Cloud size={18} aria-hidden />}
            <div><strong>{stateText.label}</strong><p>{stateText.detail}</p></div>
            <span>{W.userOwnedBackupSettings.text67(overview?.recordCount ?? 0, overview?.objectCount ?? 0)}</span>
          </div>
        )}

        {restorePlan && (
          <div className="restore-plan" role="status">
            <div>
              <strong>{W.userOwnedBackupSettings.text68(restorePlan.remoteSnapshot.snapshotId.slice(0, 8))}</strong>
              <p>{W.userOwnedBackupSettings.text69(restorePlan.remoteSnapshot.recordCount, restorePlan.remoteSnapshot.objectCount)}</p>
            </div>
            {restorePlan.requiresOverwriteConfirmation && (
              <label className="restore-confirmation">
                <input type="checkbox" checked={confirmOverwrite} onChange={(event) => setConfirmOverwrite(event.target.checked)} />
                <span>{W.userOwnedBackupSettings.text70}</span>
              </label>
            )}
            <div className="inline-actions">
              <button className="text-button" type="button" onClick={() => setRestorePlan(null)}>{W.userOwnedBackupSettings.text71}</button>
              <button className="confirm-button" type="button" disabled={busy !== null || (restorePlan.requiresOverwriteConfirmation && !confirmOverwrite)} onClick={() => void restoreNow()}>
                {busy === 'restore' ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <Download size={14} aria-hidden />}{W.userOwnedBackupSettings.text72}</button>
            </div>
          </div>
        )}
        {activeOperationId && transferStage && (
          <div className="restore-plan" role="status">
            <div>
              <strong>{TRANSFER_STAGE_LABELS[transferStage] ?? W.userOwnedBackupSettings.text73}</strong>
              <p>
                {transferProgress.total > 0
                  ? W.userOwnedBackupSettings.text74(transferProgress.completed, transferProgress.total)
                  : W.userOwnedBackupSettings.text75}
              </p>
            </div>
            <button className="secondary-button" type="button" onClick={() => void cancelTransfer()}>
              <X size={14} aria-hidden />{W.userOwnedBackupSettings.text76}</button>
          </div>
        )}
        {messageArea === 'transfer' && message && <div className="inline-status backup-message" role="status">{message}</div>}
      </section>

      <section className="settings-group" aria-labelledby="recovery-key-title">
        <div className="settings-group-heading">
          <div>
            <h3 id="recovery-key-title">{W.userOwnedBackupSettings.text77}</h3>
            <p>{W.userOwnedBackupSettings.text78}</p>
          </div>
          <div className="inline-actions">
            <span className={`connection-state ${overview?.recoveryKeyConfirmed ? 'ready' : ''}`}>
              {overview?.recoveryKeyConfirmed ? W.userOwnedBackupSettings.text79 : W.userOwnedBackupSettings.text80}
            </span>
            <button className="secondary-button" type="button" disabled={busy !== null || !overview} onClick={() => void revealRecoveryKey()}>
            {busy === 'key' ? <LoaderCircle size={14} className="is-spinning" aria-hidden /> : <KeyRound size={14} aria-hidden />}
              {overview?.recoveryKeyStored ? W.userOwnedBackupSettings.text81 : W.userOwnedBackupSettings.text82}
            </button>
          </div>
        </div>
        {recoveryKey && (
          <div className="recovery-key-view">
            <code>{recoveryKey}</code>
            <button className="icon-button subtle" type="button" aria-label={W.userOwnedBackupSettings.aria4} title={W.userOwnedBackupSettings.aria4} onClick={() => void navigator.clipboard.writeText(recoveryKey)}>
              <Copy size={14} aria-hidden />
            </button>
            <button className="icon-button subtle" type="button" aria-label={W.userOwnedBackupSettings.aria5} title={W.userOwnedBackupSettings.aria5} onClick={() => setRecoveryKey(null)}>
              <X size={14} aria-hidden />
            </button>
          </div>
        )}
        <div className="recovery-key-import">
          <input
            type="password"
            value={recoveryKeyInput}
            autoComplete="off"
            spellCheck={false}
            placeholder={W.userOwnedBackupSettings.hint1}
            onChange={(event) => setRecoveryKeyInput(event.target.value)}
          />
          <button className="secondary-button" type="button" disabled={busy !== null || !recoveryKeyInput.trim() || !overview?.recoveryKeyStored} onClick={() => void confirmRecoveryKey()}>
            <Check size={14} aria-hidden />{W.userOwnedBackupSettings.text83}</button>
          <button className="secondary-button" type="button" disabled={busy !== null || !recoveryKeyInput.trim()} onClick={() => void importRecoveryKey()}>
            <KeyRound size={14} aria-hidden />{W.userOwnedBackupSettings.text84}</button>
        </div>
        {messageArea === 'key' && message && <div className="inline-status backup-message" role="status">{message}</div>}
      </section>
    </>
  )
}
