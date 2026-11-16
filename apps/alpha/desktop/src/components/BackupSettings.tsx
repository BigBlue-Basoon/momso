import { useCallback, useEffect, useState } from 'react'
import { Database, GitBranch, Info } from 'lucide-react'
import {
  errorState,
  listenStoreProgress,
  remoteConnect,
  remoteDisconnect,
  remotePull,
  remotePush,
  remoteStatus,
  storeBackup,
  storeConnect,
  storeDisconnect,
  storeEndpoint,
  storeRestore,
  storeStatus,
  type RemoteStatus,
  type StoreProgress,
  type StoreProvider,
  type StoreStatus,
} from '../backup'
import { confirmBackupRecoveryKey, exportBackupRecoveryKey, getUserOwnedBackupOverview, importBackupRecoveryKey } from '../runtime'
import { W } from '../words'
import { QuestionDialog } from './ZetPanel'

const B = W.backup
const dateFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' })
const when = (value: string | null) => (value ? dateFormat.format(new Date(value)) : null)
function size(bytes: number | null) {
  if (bytes === null) return ''
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))}KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)}MB`
  return `${(bytes / 1024 ** 3).toFixed(1)}GB`
}
const message = (state: string | null) => (state && B.reason[state]) || B.failed
// GitHub 의 까닭 한 줄. 이력이 막힌 까닭이 사용자 정보("나에 대해")면 그것을 가를 수 있게 따로 적는다(지우거나 이력을 고치는 단추는 없다).
const PROFILE_HELD = new Set(['profile_in_history', 'profile_in_rules_history'])
const remoteMessage = (status: RemoteStatus) => (
  status.reason === 'unsafe_history' && status.historyReason && PROFILE_HELD.has(status.historyReason) ? B.profileHeld : message(status.reason)
)

/**
 * 설정 "연결과 백업": (가) GitHub — 문서와 고친 이력을 내 비공개 저장소에, (나) 오브젝트 스토리지 — 원자료를 고른 회사(AWS·GCP·Cloudflare·NCP)에.
 * 명령이 없으면 그 칸의 단추·입력은 눌리지 않게만 한다. 키는 받아서 바로 넘기고 칸을 비운다.
 */
export function BackupSettings() {
  return (
    <>
      <GithubBackup />
      <StoreBackup />
      <RecoveryKey />
    </>
  )
}

function GithubBackup() {
  const [status, setStatus] = useState<RemoteStatus | null | undefined>(undefined)
  const [repo, setRepo] = useState('')
  const [working, setWorking] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [asking, setAsking] = useState(false)
  const refresh = useCallback(() => { remoteStatus().then(setStatus).catch(() => setStatus(null)) }, [])
  useEffect(() => { refresh() }, [refresh])
  // unavailable(예: 이 PC에 Git이 없음 git_missing)이면 입력과 단추는 눌리지 않게 하고 그 까닭 한 줄만 보인다.
  const supported = Boolean(status) && status?.state !== 'unavailable'
  const connected = status?.state === 'connected'
  const standing = !note && status?.reason ? remoteMessage(status) : null

  async function run(action: () => Promise<RemoteStatus>) {
    setWorking(true); setNote(null)
    try {
      const next = await action()
      setStatus(next)
      setNote(next.reason ? remoteMessage(next) : B.done)
    } catch (error) { setNote(message(errorState(error))) }
    setWorking(false)
  }

  return (
    <section className="settings-group backup-part" aria-labelledby="backup-github-title">
      <div className="backup-head">
        <GitBranch size={18} aria-hidden />
        <div>
          <h3 id="backup-github-title">{B.githubTitle}</h3>
          <p>{B.githubWhat}</p>
        </div>
      </div>
      {connected ? (
        <>
          <dl className="backup-facts">
            <div><dt>{B.repoLabel}</dt><dd>{status?.repo}</dd></div>
            {status?.signedInAs && <div><dt>{B.account}</dt><dd>{status.signedInAs}</dd></div>}
            {when(status?.lastPushedAt ?? null) && <div><dt>{B.lastPushed}</dt><dd>{when(status?.lastPushedAt ?? null)}</dd></div>}
          </dl>
          <div className="backup-actions">
            {(status?.ahead ?? 0) > 0 && <span className="backup-count">{B.ahead(status?.ahead)}</span>}
            {(status?.behind ?? 0) > 0 && <span className="backup-count">{B.behind(status?.behind)}</span>}
            <button className="secondary-button" type="button" disabled={working} onClick={() => void run(remotePull)}>{B.pull}</button>
            <button className="secondary-button" type="button" disabled={working} onClick={() => void run(remotePush)}>{B.push}</button>
            <button className="secondary-button subtle" type="button" disabled={working} onClick={() => setAsking(true)}>{B.disconnect}</button>
          </div>
        </>
      ) : (
        <>
          <p className="backup-notice"><Info size={14} aria-hidden />{B.githubNotice}</p>
          <form className="backup-row" onSubmit={(event) => { event.preventDefault(); if (repo.trim()) void run(() => remoteConnect(repo.trim())) }}>
            <label className="backup-field">
              <span>{B.repoLabel}</span>
              <input value={repo} spellCheck={false} autoComplete="off" disabled={!supported} onChange={(event) => setRepo(event.target.value)} />
            </label>
            <button className="secondary-button" type="submit" disabled={!supported || working || !repo.trim()}>{B.connect}</button>
          </form>
        </>
      )}
      {(note ?? standing) && <p className="backup-note" role="status">{note ?? standing}</p>}
      {asking && (
        <QuestionDialog
          title={B.githubDisconnectTitle}
          text={B.githubDisconnectText}
          actions={[
            { label: B.cancel, onSelect: () => setAsking(false), autoFocus: true },
            { label: B.disconnect, primary: true, onSelect: () => { setAsking(false); void run(remoteDisconnect) } },
          ]}
          onDismiss={() => setAsking(false)}
        />
      )}
    </section>
  )
}

// 원자료 저장소 회사 이름은 영어 그대로(2026-11-16 김성균 개발자가 정함). 글은 낱말 파일에.
const PROVIDERS: Array<{ id: StoreProvider; label: string }> = (['aws', 'gcp', 'cloudflare', 'ncp'] as StoreProvider[]).map((id) => ({ id, label: B.storeName[id] }))

/**
 * 고를 수 있는 회사: 넷 다. 뒤쪽이 연결할 때 시험 객체로 쓰기 방식(conditional 또는 checkThenWrite)을 스스로 정한다
 * (2026-11-16 원자료 백업 계약 16줄, 옮긴 것 절 "로그인, 추론 수준, Git과 원자료 저장소 보완").
 */
function providerUsable(_provider: StoreProvider, status: StoreStatus | null | undefined) {
  return Boolean(status)
}

/**
 * 앱 상태 되살리기(app_state_restore)를 이 앱에서 쓸 수 있는지. 지금 화면 폴더의 뒤쪽은 몸소 폴더 위치 이관과 잠금이 아직 없어
 * unavailable 만 돌려주고, 백엔드 노트가 "그 전에는 화면에서 앱 상태 복원 단추를 비활성화한다"고 정했다. 위치 작업이 옮겨지면 true.
 */
const APP_STATE_RESTORE_READY = false as boolean

function StoreBackup() {
  const [status, setStatus] = useState<StoreStatus | null | undefined>(undefined)
  const [provider, setProvider] = useState<StoreProvider>('aws')
  const [fields, setFields] = useState({ region: '', bucket: '', endpoint: '' })
  const [keyId, setKeyId] = useState('')
  const [secret, setSecret] = useState('')
  const [working, setWorking] = useState(false)
  const [progress, setProgress] = useState<StoreProgress | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [asking, setAsking] = useState<'disconnect' | 'restore' | null>(null)
  const refresh = useCallback(() => { storeStatus().then(setStatus).catch(() => setStatus(null)) }, [])
  useEffect(() => {
    refresh()
    let stop: (() => void) | undefined
    let disposed = false
    void listenStoreProgress(setProgress).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [refresh])
  const supported = Boolean(status)
  const connected = status?.state === 'connected'
  const needsRegion = provider === 'aws' || provider === 'ncp'
  const needsEndpoint = provider === 'cloudflare' || provider === 'ncp'
  const ready = supported && providerUsable(provider, status) && fields.bucket.trim() && keyId && secret && (!needsRegion || fields.region.trim()) && (!needsEndpoint || fields.endpoint.trim())

  async function connect() {
    const input = {
      provider,
      endpoint: storeEndpoint(provider, fields.region, fields.endpoint),
      region: provider === 'cloudflare' ? 'auto' : provider === 'gcp' ? 'auto' : fields.region.trim(),
      bucket: fields.bucket.trim(),
      accessKeyId: keyId,
      secretAccessKey: secret,
    }
    setKeyId(''); setSecret('')
    setWorking(true); setNote(null)
    try { setStatus(await storeConnect(input)); setNote(B.done) } catch (error) { setNote(message(errorState(error))) }
    setWorking(false)
  }
  async function act(action: () => Promise<unknown>) {
    setWorking(true); setNote(null); setProgress(null)
    try { await action(); setNote(B.done) } catch (error) { setNote(message(errorState(error))) }
    setWorking(false)
    refresh()
  }

  return (
    <section className="settings-group backup-part" aria-labelledby="backup-store-title">
      <div className="backup-head">
        <Database size={18} aria-hidden />
        <div>
          <h3 id="backup-store-title">{B.storeTitle}</h3>
          <p>{B.storeWhat}</p>
        </div>
      </div>
      {connected ? (
        <>
          <dl className="backup-facts">
            <div><dt>{B.provider}</dt><dd>{PROVIDERS.find((item) => item.id === status?.provider)?.label ?? status?.provider}</dd></div>
            <div><dt>{B.bucket}</dt><dd>{status?.bucket}</dd></div>
            {when(status?.lastBackupAt ?? null) && <div><dt>{B.lastBackup}</dt><dd>{when(status?.lastBackupAt ?? null)}</dd></div>}
          </dl>
          {(status?.pendingCount ?? 0) > 0 && <p className="backup-count">{B.pending(status?.pendingCount, size(status?.pendingBytes ?? null))}</p>}
          {working && progress && progress.totalBytes > 0 && (
            <div className="app-update-progress" role="status"><span style={{ width: `${Math.max(4, (progress.completedBytes / progress.totalBytes) * 100)}%` }} /></div>
          )}
          <div className="backup-actions">
            <button className="secondary-button" type="button" disabled={working} onClick={() => void act(storeBackup)}>{B.backupNow}</button>
            <button className="secondary-button" type="button" disabled={working || !APP_STATE_RESTORE_READY} onClick={() => setAsking('restore')}>{B.restore}</button>
            <button className="secondary-button subtle" type="button" disabled={working} onClick={() => setAsking('disconnect')}>{B.disconnect}</button>
          </div>
        </>
      ) : (
        <form className="backup-form" onSubmit={(event) => { event.preventDefault(); if (ready) void connect() }}>
          <div className="backup-providers" role="radiogroup" aria-label={B.provider}>
            {PROVIDERS.map((item) => {
              const usable = supported && providerUsable(item.id, status)
              return (
                <button
                  key={item.id}
                  type="button"
                  role="radio"
                  aria-checked={provider === item.id}
                  className={provider === item.id ? 'is-active' : undefined}
                  disabled={!usable}
                  onClick={() => setProvider(item.id)}
                >
                  {item.label}
                </button>
              )
            })}
          </div>
          <div className="backup-grid">
            {needsRegion && <label className="backup-field"><span>{B.region}</span><input value={fields.region} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, region: event.target.value })} /></label>}
            {needsEndpoint && <label className="backup-field"><span>{B.endpoint}</span><input value={fields.endpoint} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, endpoint: event.target.value })} /></label>}
            <label className="backup-field"><span>{B.bucket}</span><input value={fields.bucket} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, bucket: event.target.value })} /></label>
            <label className="backup-field"><span>{B.accessKey}</span><input type="password" autoComplete="off" value={keyId} disabled={!supported} onChange={(event) => setKeyId(event.target.value)} /></label>
            <label className="backup-field"><span>{B.secretKey}</span><input type="password" autoComplete="off" value={secret} disabled={!supported} onChange={(event) => setSecret(event.target.value)} /></label>
          </div>
          <div className="backup-actions">
            <button className="secondary-button" type="submit" disabled={!ready || working}>{B.connect}</button>
          </div>
        </form>
      )}
      {note && <p className="backup-note" role="status">{note}</p>}
      {asking === 'disconnect' && (
        <QuestionDialog
          title={B.storeDisconnectTitle}
          text={B.storeDisconnectText}
          actions={[
            { label: B.cancel, onSelect: () => setAsking(null), autoFocus: true },
            { label: B.disconnect, primary: true, onSelect: () => { setAsking(null); void act(storeDisconnect) } },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
      {asking === 'restore' && (
        <QuestionDialog
          title={B.restoreTitle}
          text={B.restoreText}
          actions={[
            { label: B.cancel, onSelect: () => setAsking(null), autoFocus: true },
            { label: B.restore, primary: true, onSelect: () => { setAsking(null); void act(storeRestore) } },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
    </section>
  )
}

/** 복구 키(예전 화면의 것과 같은 명령·같은 글): 암호화한 백업을 새 기기에서 열 때 필요. */
function RecoveryKey() {
  const U = W.userOwnedBackupSettings
  const [state, setState] = useState<{ stored: boolean; confirmed: boolean } | null>(null)
  const [shown, setShown] = useState<string | null>(null)
  const [input, setInput] = useState('')
  const [note, setNote] = useState<string | null>(null)
  const refresh = useCallback(() => {
    getUserOwnedBackupOverview().then((o) => setState(o ? { stored: o.recoveryKeyStored, confirmed: o.recoveryKeyConfirmed } : null)).catch(() => setState(null))
  }, [])
  useEffect(() => { refresh() }, [refresh])
  async function reveal() { try { setShown(await exportBackupRecoveryKey()) } catch { setNote(B.failed) } }
  async function confirm() { const value = input; setInput(''); try { await confirmBackupRecoveryKey(value); setShown(null); setNote(W.userOwnedBackupSettings.text43 ?? B.done); refresh() } catch { setNote(B.failed) } }
  async function importKey() { const value = input; setInput(''); try { await importBackupRecoveryKey(value); setNote(U.text42); refresh() } catch { setNote(B.failed) } }
  return (
    <section className="settings-group backup-part" aria-labelledby="backup-key-title">
      <div className="settings-group-heading">
        <div>
          <h3 id="backup-key-title">{U.text77}</h3>
          <p>{U.text78}</p>
        </div>
        <span className={`connection-state ${state?.confirmed ? 'ready' : ''}`}>{state?.confirmed ? U.text79 : U.text80}</span>
      </div>
      <div className="backup-actions">
        <button className="secondary-button" type="button" disabled={!state} onClick={() => void reveal()}>{state?.stored ? U.text81 : U.text82}</button>
      </div>
      {shown && <p className="backup-key-shown">{shown}</p>}
      <form className="backup-row" onSubmit={(event) => { event.preventDefault(); if (input) void (shown ? confirm() : importKey()) }}>
        <input type="password" autoComplete="off" placeholder={U.hint1} value={input} disabled={!state} onChange={(event) => setInput(event.target.value)} aria-label={U.hint1} />
        <button className="secondary-button" type="submit" disabled={!state || !input}>{shown ? U.text83 : U.text84}</button>
      </form>
      {note && <p className="backup-note" role="status">{note}</p>}
    </section>
  )
}
