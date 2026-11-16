import { useCallback, useEffect, useState } from 'react'
import { Database, GitBranch, Info, LoaderCircle } from 'lucide-react'
import {
  errorState,
  listenStoreFailed,
  listenStoreProgress,
  remoteConnect,
  remoteCreate,
  gitInstall,
  gitInstallStatus,
  listenGitInstall,
  type GitInstallEvent,
  type GitInstallState,
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
import { getUsername } from '../first-run'
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
// GitHub 의 까닭 한 줄. 예전에는 이력이 막힌 까닭이 사용자 정보("나에 대해")면 따로 적었지만, 2026-11-16 그가 사용자 정보도 올린다고 정해
// 뒤쪽은 historyReason 을 늘 비워 준다(archive_remote.rs). 그래서 그 갈래와 글을 지웠다.
const remoteMessage = (status: RemoteStatus) => message(status.reason)

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

/** 첫 실행에서 받은 영어 username(GitHub 저장소와 원자료 버킷 이름 momso-<username> 에 쓴다). 없거나 읽지 못하면 null. */
function useUsername() {
  const [name, setName] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    getUsername().then((result) => { if (alive) setName(result.state === 'ok' ? result.username : null) }).catch(() => {})
    return () => { alive = false }
  }, [])
  return name
}

function GithubBackup() {
  const [status, setStatus] = useState<RemoteStatus | null | undefined>(undefined)
  const [repo, setRepo] = useState('')
  const [working, setWorking] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [asking, setAsking] = useState<'disconnect' | 'create' | false>(false)
  const username = useUsername()
  const refresh = useCallback(() => { remoteStatus().then(setStatus).catch(() => setStatus(null)) }, [])
  useEffect(() => { refresh() }, [refresh])
  // 저장소 만들기(archive_remote_create)는 username 이 있을 때만 누를 수 있다. 명령이 없으면 눌리지 않는다(누른 뒤 알게 되면 막는다).
  const [createAvailable, setCreateAvailable] = useState(true)
  const newRepoName = username ? `momso-${username}` : null

  async function create() {
    setAsking(false)
    setWorking(true); setNote(null)
    try {
      const result = await remoteCreate()
      // 이미 같은 이름이 있으면 만들지 않았다. 비공개로 이어 쓸 수 있는 것이면 아래 칸에 그 이름을 넣어 둔다(연결은 사용자가 누른다).
      if ((result.state === 'exists_empty' || result.state === 'exists_matching' || result.state === 'exists_private_unknown') && result.repo) setRepo(result.repo)
      setNote(B.create[result.state] ?? B.failed)
    } catch (error) {
      if (/not found/i.test(String(error))) setCreateAvailable(false)
      else setNote(message(errorState(error)))
    }
    setWorking(false)
    refresh()
  }
  // unavailable(예: 이 PC에 Git이 없음 git_missing)이면 입력과 단추는 눌리지 않게 하고 그 까닭 한 줄만 보인다.
  const supported = Boolean(status) && status?.state !== 'unavailable'
  const connected = status?.state === 'connected'
  // Git 이 없어서 막혔으면(git_missing) 그 까닭 줄 대신 그 자리에서 바로 설치할 수 있는 줄을 둔다(GitInstallRow).
  const gitMissing = status?.reason === 'git_missing'
  const standing = !note && status?.reason && !gitMissing ? remoteMessage(status) : null

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
            <button className="secondary-button subtle" type="button" disabled={working} onClick={() => setAsking('disconnect')}>{B.disconnect}</button>
          </div>
        </>
      ) : (
        <>
          <p className="backup-notice"><Info size={14} aria-hidden />{B.githubNotice}</p>
          {/* 저장소를 미리 만들지 않아도 된다: momso-<username> 비공개 저장소를 만들어 연결한다. 이미 있는 저장소는 아래 칸에 적어 연결한다. */}
          <div className="backup-row backup-create">
            <span className="backup-create-text">{newRepoName ? B.createWhat(newRepoName) : B.createNeedsName}</span>
            <button className="secondary-button" type="button" disabled={!supported || working || !newRepoName || !createAvailable} onClick={() => setAsking('create')}>{B.createRepo}</button>
          </div>
          <form className="backup-row" onSubmit={(event) => { event.preventDefault(); if (repo.trim()) void run(() => remoteConnect(repo.trim())) }}>
            <label className="backup-field">
              <span>{B.repoLabel}</span>
              <input value={repo} spellCheck={false} autoComplete="off" disabled={!supported} onChange={(event) => setRepo(event.target.value)} />
            </label>
            <button className="secondary-button" type="submit" disabled={!supported || working || !repo.trim()}>{B.connect}</button>
          </form>
        </>
      )}
      {gitMissing && <GitInstallRow onInstalled={refresh} />}
      {(note ?? standing) && <p className="backup-note" role="status">{note ?? standing}</p>}
      {asking === 'disconnect' && (
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
      {asking === 'create' && newRepoName && (
        <QuestionDialog
          title={B.createTitle(newRepoName)}
          text={B.createText}
          actions={[
            { label: B.cancel, onSelect: () => setAsking(false) },
            { label: B.createRepo, primary: true, autoFocus: true, onSelect: () => void create() },
          ]}
          onDismiss={() => setAsking(false)}
        />
      )}
    </section>
  )
}

/**
 * Git 이 없어 GitHub 백업이 막혔을 때 그 자리의 한 줄: "GitHub에 올리려면 Git이라는 프로그램이 필요합니다." 와 "설치하기".
 * AI 도구를 설치할 때와 같은 꼴: 한 번 묻고(QuestionDialog), 설치하는 동안 도는 그림과 "설치하고 있습니다"(받은 양과 전체를 둘 다 알 때만 막대),
 * 실패하면 까닭 한 줄과 "다시 시도". 다 되면 GitHub 상태를 다시 읽는다(그러면 이 줄은 사라진다).
 * 뒤쪽이 unavailable(이 PC 에서 설치할 수 없음)을 주면 단추 없이 글만 둔다.
 */
function GitInstallRow({ onInstalled }: { onInstalled: () => void }) {
  const [phase, setPhase] = useState<GitInstallState | 'failed' | null>(null)
  const [reason, setReason] = useState<string | null>(null)
  const [bytes, setBytes] = useState<{ completed: number; total: number } | null>(null)
  const [asking, setAsking] = useState(false)

  useEffect(() => {
    let alive = true
    let stop: (() => void) | undefined
    gitInstallStatus().then((state) => {
      if (!alive) return
      if (state === 'installed') onInstalled()
      else setPhase(state)
    }).catch(() => { if (alive) setPhase('unavailable') })
    void listenGitInstall((event: GitInstallEvent) => {
      if (!alive) return
      if (event.state === 'inProgress') {
        setPhase('inProgress')
        setBytes(typeof event.total === 'number' && event.total > 0 ? { completed: event.completed, total: event.total } : null)
      } else if (event.state === 'installed') {
        setPhase('installed')
        onInstalled()
      } else if (event.state === 'installFailed') {
        setPhase('failed')
        setReason(event.stage || null)
        setBytes(null)
      }
    }).then((unlisten) => { if (alive) stop = unlisten; else unlisten() }).catch(() => {})
    return () => { alive = false; stop?.() }
  }, [onInstalled])

  async function start() {
    setAsking(false)
    setReason(null)
    setPhase('inProgress')
    try {
      const state = await gitInstall()
      if (state === 'installed') onInstalled()
      else if (state !== 'inProgress') setPhase(state)
    } catch {
      setPhase('failed')
    }
  }

  return (
    <div className="backup-row backup-git">
      <span className="backup-create-text">{B.gitNeeded}</span>
      {phase === 'inProgress' ? (
        <span className="provider-pending" role="status">
          <LoaderCircle size={14} className="is-spinning" aria-hidden />{W.signIn.installing}
          {bytes && <span className="provider-install-bar" aria-hidden><span style={{ width: `${Math.min(100, (bytes.completed / bytes.total) * 100)}%` }} /></span>}
        </span>
      ) : phase === 'approvalRequired' || phase === 'failed' ? (
        <button className="secondary-button" type="button" onClick={() => setAsking(true)}>{phase === 'failed' ? W.signIn.installRetry : W.signIn.install}</button>
      ) : null}
      {phase === 'failed' && <p className="provider-error" role="alert">{(reason && B.gitInstallReason[reason]) || B.gitInstallFailed}</p>}
      {asking && (
        <QuestionDialog
          title={B.gitInstallTitle}
          text={B.gitInstallText}
          actions={[
            { label: W.signIn.cancel, onSelect: () => setAsking(false) },
            { label: W.signIn.install, primary: true, autoFocus: true, onSelect: () => void start() },
          ]}
          onDismiss={() => setAsking(false)}
        />
      )}
    </div>
  )
}

// 원자료 저장소 회사 이름은 영어 그대로, 그 회사 로고를 붙인다(2026-11-16 김성균 개발자가 정함).
// 로고는 각 회사가 내놓은 공식 파일을 고치지 않고 그대로 쓴다(public/brand/storage, 어디서 받았는지는 zet-panel-notes.md 67절).
// NCP 는 네이버 로고(네이버 브랜드 지침에 동의하고 받은 "NAVER" 글자 로고, 밝은 바탕용 녹색). 앱에 어두운 화면이 없어 흰색 판은 쓰지 않는다.
// 2026-11-16 김성균 개발자: 딸려 온 것("powered by", 단추 틀, 작은 글자) 없이 회사 표지만. 각 회사 서버의 표지만 있는 공식 그림을 그대로 쓴다:
// AWS 웃는 모양 로고(aws_logo_smile_179x109.png), 구글 G(googleg_standard_color_128dp), Cloudflare 구름(logo.svg),
// 네이버 N(초록 네모, 네이버가 자기 사이트 아이콘으로 쓰는 nFavicon96.png. 그의 말 "네이버도 N 이거 글자 하나만 있는 CI").
// AWS 는 표지 안에 "aws" 글자가 있으니 옆에 "AWS" 를 또 적지 않는다(화면 읽기 이름에만 둔다).
const LOGOS: Partial<Record<StoreProvider, string>> = {
  aws: '/brand/storage/aws-logo-smile.png',
  gcp: '/brand/storage/google-g.png',
  cloudflare: '/brand/storage/cloudflare-cloud.svg',
  ncp: '/brand/storage/naver-n.png',
}
const LOGO_ONLY = new Set<StoreProvider>(['aws'])
const PROVIDERS: Array<{ id: StoreProvider; label: string; logo?: string }> = (['aws', 'gcp', 'cloudflare', 'ncp'] as StoreProvider[]).map((id) => ({ id, label: B.storeName[id], logo: LOGOS[id] }))

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
    // 백업이 실패하면(이 화면이 부른 것이든 아니든) 진행 막대를 거두고, 있던 알림 줄에 실패 까닭을 적는다.
    let stopFailed: (() => void) | undefined
    void listenStoreFailed((state) => { setProgress(null); setNote(message(state)); refresh() })
      .then((unlisten) => { if (disposed) unlisten(); else stopFailed = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.(); stopFailed?.() }
  }, [refresh])
  const supported = Boolean(status)
  const connected = status?.state === 'connected'
  const needsRegion = provider === 'aws' || provider === 'ncp'
  const needsEndpoint = provider === 'cloudflare' || provider === 'ncp'
  // 버킷 이름을 비워 두면 뒤쪽이 저장된 username 으로 momso-<username> 버킷을 만들어 연결한다(GCP 는 더 필요한 값이 있어 이름을 받는다).
  const username = useUsername()
  const autoBucket = provider !== 'gcp' && username ? `momso-${username}` : null
  const ready = supported && providerUsable(provider, status) && (fields.bucket.trim() || autoBucket) && keyId && secret && (!needsRegion || fields.region.trim()) && (!needsEndpoint || fields.endpoint.trim())

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
                  aria-label={item.label}
                >
                  {item.logo && <img className={`backup-provider-logo is-${item.id}`} src={item.logo} alt="" aria-hidden="true" />}
                  {!(item.logo && LOGO_ONLY.has(item.id)) && <span aria-hidden="true">{item.label}</span>}
                </button>
              )
            })}
          </div>
          <div className="backup-grid">
            {needsRegion && <label className="backup-field"><span>{B.region}</span><input value={fields.region} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, region: event.target.value })} /></label>}
            {needsEndpoint && <label className="backup-field"><span>{B.endpoint}</span><input value={fields.endpoint} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, endpoint: event.target.value })} /></label>}
            <label className="backup-field">
              <span>{B.bucket}</span>
              <input value={fields.bucket} disabled={!supported} spellCheck={false} onChange={(event) => setFields({ ...fields, bucket: event.target.value })} />
              {autoBucket && !fields.bucket.trim() && <small className="backup-field-hint">{B.autoBucket(autoBucket)}</small>}
            </label>
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
  // 받은 복구 키는 평소 가려 둔다(●). "보기"를 눌러야 보이고 "가리기"로 다시 가린다. 화면을 떠나면 이 부분이 사라져 다음에는 다시 가려져 있다.
  const [visible, setVisible] = useState(false)
  const [input, setInput] = useState('')
  const [note, setNote] = useState<string | null>(null)
  const refresh = useCallback(() => {
    getUserOwnedBackupOverview().then((o) => setState(o ? { stored: o.recoveryKeyStored, confirmed: o.recoveryKeyConfirmed } : null)).catch(() => setState(null))
  }, [])
  useEffect(() => { refresh() }, [refresh])
  async function reveal() { try { setVisible(false); setShown(await exportBackupRecoveryKey()) } catch { setNote(B.failed) } }
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
      {shown && (
        <div className="backup-key-row">
          <p className={visible ? 'backup-key-shown' : 'backup-key-shown is-masked'} aria-label={visible ? undefined : U.text77}>
            {visible ? shown : '●'.repeat(Math.min(Math.max(shown.length, 12), 32))}
          </p>
          <button className="secondary-button" type="button" aria-label={visible ? U.aria5 : U.keyShowAria} aria-pressed={visible} onClick={() => setVisible((current) => !current)}>
            {visible ? U.keyHide : U.keyShow}
          </button>
        </div>
      )}
      <form className="backup-row" onSubmit={(event) => { event.preventDefault(); if (input) void (shown ? confirm() : importKey()) }}>
        <input type="password" autoComplete="off" placeholder={U.hint1} value={input} disabled={!state} onChange={(event) => setInput(event.target.value)} aria-label={U.hint1} />
        <button className="secondary-button" type="submit" disabled={!state || !input}>{shown ? U.text83 : U.text84}</button>
      </form>
      {note && <p className="backup-note" role="status">{note}</p>}
    </section>
  )
}
