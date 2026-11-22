import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Check, Clock, Copy, LoaderCircle, X } from 'lucide-react'
import { W } from '../words'
import { REDIRECTING, loadStart, setupErrorStep, type StartBillingAccount, type StartKit, type StartProgress, type StartProject, type StartResume } from './start-host'
import './StartScreen.css'

/**
 * 휴대폰에서 시작하기(줄2 S26). 공개 시작 페이지에서 고객의 Google Cloud 에 몸소(VM·입구)를 만들고, 끝나면 그 주소와 홈 화면에 추가하는 법을 보인다.
 * 첫 실행(FirstRun)의 꼴: 가운데 한 줄 칸, 제목 + 흐린 설명, 아래에 단추. 진행은 단계 목록(배포 서비스들처럼 단계마다 기다림·하는 중·끝남·멈춤),
 * 멈추면 그 단계에 표시하고 까닭 한 줄과 "다시 시도"(같은 설치 기록으로 멈춘 곳부터 다시 확인), 결제 준비가 필요하면 Google Cloud 결제로 가는 단추.
 * 끝나면 주소(복사)·"몸소 열기"·홈 화면에 추가하는 법(아이폰은 설치 알림이 없어 공유 메뉴로, 안드로이드는 크롬의 설치. 공식 도움말 단추 이름 그대로).
 * Google 권한 창은 누른 그 자리에서 열려야 하므로 시작·찾기는 단추 누름에서 곧바로 부른다(설정·스크립트는 화면을 열 때 미리).
 * S64(2026-11-22): "Google 계정으로 시작하기"를 누르면 그 누름에서 프로젝트·열린 결제 계정 목록을 읽고, "설치할 곳 고르기"에서 고른 뒤 "설치 시작".
 *   고르는 칸은 앱 설정의 고르기 칸(.setting-field 의 select: 휴대폰은 기기의 고르기 창). 프로젝트는 "새 프로젝트 만들기"가 처음 값,
 *   결제 계정은 하나뿐이면 그것, 여럿이면 고르게 한다. 기존 프로젝트는 결제를 고르지 않아도 시작할 수 있다(이미 연결된 결제는 모듈이 바꾸지 않음).
 *   고른 것 때문에 멈추면(결제 고르지 않음·고른 결제 못 씀·고른 프로젝트 못 씀) "다시 시도" 대신 "다시 고르기". 목록은 이 화면 메모리에만.
 * S65(2026-11-22): 로그인이 창 전체가 Google 로 갔다 돌아오는 방식이라, 함수가 { redirecting } 이면 기다리는 모습 그대로 둔다(창이 떠난다).
 *   화면이 열릴 때마다 한 번 resumeAfterRedirect: 프로젝트 목록이 돌아오면 고르기(프로젝트 칸 + "다음"), 결제 목록이 돌아오면 고르기(고른 프로젝트는 그대로 보이고
 *   "다시 고르기"로 바꿈 + 결제 칸 + "설치 시작"). 두 목록이 한 번에 오는 예전 모듈은 전처럼 한 화면. 고른 값은 모듈이 돌려준 selection 그대로.
 *   설치가 돌아오면 진행 화면(단계가 이어지고 끝에 주소), 찾기는 찾은 결과, 지우기는 지운 결과와 남은 것(지우기 단추는 그의 결정 전이라 두지 않는다).
 *   단계의 pending(나중에 다시 해야 함)은 점선 칸과 시계.
 * S65b: 돌아온 토큰을 모듈이 메모리에 두어 "다음"(결제 목록)·설치 등이 Google 에 다시 가지 않고 이 화면에서 끝난다(화면은 같은 함수를 그대로 부름).
 *   멈춘 뒤 "다시 시도"는 retryLastAction(이 페이지에서 같은 일·같은 고르기로 다시 하고 결과를 resume 과 같은 모양으로 줌). 없는 예전 모듈은 retryFromGesture(Google 로),
 *   그것도 Google 에서 돌아온 뒤에만. 다시 할 기록이 없으면(checkpoint_unavailable) 이 화면에서 새로 시작한다.
 */
const STEPS = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady']
type StepState = 'waiting' | 'working' | 'done' | 'failed' | 'pending'
type View =
  | { kind: 'loading' }
  | { kind: 'unavailable' }
  | { kind: 'intro' }
  | { kind: 'choosing' }
  | { kind: 'choose' }
  | { kind: 'listFailed'; code: string }
  | { kind: 'running' }
  | { kind: 'failed'; code: string }
  | { kind: 'done'; url: string }
  | { kind: 'finding' }
  | { kind: 'found'; urls: string[] }
  | { kind: 'findFailed' }
  | { kind: 'destroying' }
  | { kind: 'destroyed'; remaining: string[] }
  | { kind: 'resumeFailed'; code: string }

/** SetupError 의 step 값 → 까닭 글의 묶음. */
function errorKind(code: string) {
  if (code === 'billing_selection_required') return 'billingChoice'
  if (code === 'billing_account_unavailable') return 'billingAccount'
  if (code === 'project_unavailable') return 'project'
  if (code === 'server_not_ready') return 'serverNotReady'
  if (code === 'gateway_not_ready') return 'gatewayNotReady'
  if (code.startsWith('billing')) return 'billing'
  if (code === 'authorization_declined') return 'declined'
  if (code === 'authorization_expired' || code === 'invalid_authorization') return 'expired'
  if (code === 'permission_denied') return 'permission'
  if (code === 'quota_exceeded') return 'quota'
  if (code === 'release_not_ready' || code === 'invalid_client' || code === 'provider_not_ready') return 'notReady'
  if (code === 'checkpoint_unavailable' || code === 'invalid_checkpoint') return 'checkpoint'
  if (code === 'identity_unavailable') return 'identity'
  if (code.endsWith('_ownership')) return 'ownership'
  if (code === 'network' || code === 'google_unavailable' || code === 'google_request' || code === 'google_response') return 'google'
  return 'other'
}

/** 고른 것 때문에 멈춘 까닭: 다시 시도해도 같아 다시 고르게 한다. */
const CHOICE_KINDS = new Set(['billingChoice', 'billingAccount', 'project'])

function projectLabel(project: StartProject) {
  return project.displayName && project.displayName !== project.projectId ? `${project.displayName} (${project.projectId})` : project.projectId
}

function platform() {
  const ua = navigator.userAgent
  if (/iPhone|iPad|iPod/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)) return 'iphone'
  if (/Android/.test(ua)) return 'android'
  return 'both'
}

export function StartScreen() {
  const words = W.start
  const [view, setView] = useState<View>({ kind: 'loading' })
  const [steps, setSteps] = useState<Record<string, StepState>>({})
  const [note, setNote] = useState<string | null>(null)
  const kit = useRef<StartKit | null>(null)
  // null 인 목록은 아직 읽지 않은 것(S65 에서는 프로젝트와 결제가 따로 돌아온다).
  const [choices, setChoices] = useState<{ projects: StartProject[] | null; accounts: StartBillingAccount[] | null } | null>(null)
  // 지금 보이는 멈춤이 Google 에서 돌아온 뒤의 일인지(그러면 다시 시도는 retryFromGesture).
  const resumed = useRef(false)
  const [projectId, setProjectId] = useState('')
  const [billing, setBilling] = useState('')
  const heading = useRef<HTMLHeadingElement>(null)

  useEffect(() => {
    let alive = true
    loadStart().then((value) => {
      if (!alive) return
      kit.current = value
      // 화면을 열 때마다 한 번: Google 에서 돌아왔으면 하던 일을 잇는다(이어 갈 것이 없으면 첫 화면).
      follow(value.resume, () => alive)
    }).catch(() => { if (alive) setView({ kind: 'unavailable' }) })
    return () => { alive = false }
  }, [])

  // resume·retry 의 결과를 따라간다: 진행이 오면 진행 화면(지우기 단계면 지우는 중), 끝나면 그 결과, 멈추면 까닭.
  // 지우기의 단계(vm·router 등)는 설치 단계 이름에 없다: 그런 단계가 오면 지우는 중으로 본다(첫 단계 gateway 는 둘 다에 있다).
  // fallback: 결과가 null 이거나 다시 할 기록이 없을 때(다시 시도에서만) 대신 할 일.
  function follow(run: (onProgress: (progress: StartProgress) => void) => Promise<StartResume | null>, alive: () => boolean, fallback?: () => void) {
    let shown = false
    let destroying = false
    const onProgress = (progress: StartProgress) => {
      if (!alive()) return
      record(progress)
      if (!STEPS.includes(progress.step)) destroying = true
      shown = true
      setView((now) => (now.kind === 'loading' || (destroying && now.kind === 'running') ? { kind: destroying ? 'destroying' : 'running' } : now))
    }
    run(onProgress).then((result) => {
      if (!alive()) return
      if (result === null && fallback) { fallback(); return }
      showResume(result)
    }).catch((error) => {
      if (!alive()) return
      const code = setupErrorStep(error) ?? 'other'
      if (fallback && code === 'checkpoint_unavailable') { fallback(); return }
      resumed.current = true
      setView(shown && !destroying ? { kind: 'failed', code } : { kind: 'resumeFailed', code })
    })
  }

  function record(progress: StartProgress) {
    const state: StepState = progress.state === 'working' || progress.state === 'done' || progress.state === 'failed' || progress.state === 'pending' ? progress.state : 'waiting'
    setSteps((now) => ({ ...now, [progress.step]: state }))
  }

  function showResume(result: StartResume | null) {
    if (!result) { setView({ kind: 'intro' }); return }
    resumed.current = true
    if (result.kind === 'redirecting') return
    if (result.kind === 'projects') {
      setChoices((now) => ({ projects: result.projects, accounts: now?.accounts ?? null }))
      setProjectId(result.selection.projectId ?? '')
      setBilling(result.selection.billingAccountName ?? '')
      setView({ kind: 'choose' })
    } else if (result.kind === 'billing') {
      setChoices((now) => ({ projects: now?.projects ?? null, accounts: result.accounts }))
      setProjectId(result.selection.projectId ?? '')
      setBilling(result.selection.billingAccountName ?? (result.accounts.length === 1 ? result.accounts[0].name : ''))
      setView({ kind: 'choose' })
    } else if (result.kind === 'start') {
      setProjectId(result.selection.projectId ?? '')
      setBilling(result.selection.billingAccountName ?? '')
      setView(result.url ? { kind: 'done', url: result.url } : { kind: 'failed', code: 'gatewayReady' })
    } else if (result.kind === 'find') {
      setView({ kind: 'found', urls: result.urls })
    } else {
      setView({ kind: 'destroyed', remaining: result.remaining })
    }
  }

  const selection = () => ({ projectId: projectId || null, billingAccountName: billing || null })
  // 화면이 바뀌면 제목에 초점(읽어 주기).
  useEffect(() => { heading.current?.focus() }, [view.kind])

  // 시작하기 누름: 목록 함수가 있으면(S64) 그 누름에서 프로젝트·결제 계정을 읽어 고르는 화면으로, 없으면(예전 모듈) 바로 시작.
  function begin() {
    const current = kit.current
    if (!current) return
    const { projects, billingAccounts } = current
    if (!projects || !billingAccounts) { start(); return }
    setView({ kind: 'choosing' })
    resumed.current = false
    // projects() 는 이 누름 안에서 곧바로 불린다(await 앞). Google 로 보내면(S65) 기다리는 모습 그대로 창이 떠난다.
    void (async () => {
      const projectList = await projects({ projectId: null, billingAccountName: null })
      if (projectList === REDIRECTING) return
      const accounts = await billingAccounts({ projectId: null, billingAccountName: null })
      if (accounts === REDIRECTING) return
      setChoices({ projects: projectList, accounts })
      setProjectId('')
      setBilling(accounts.length === 1 ? accounts[0].name : '')
      setView({ kind: 'choose' })
    })().catch((error) => setView({ kind: 'listFailed', code: setupErrorStep(error) ?? 'other' }))
  }

  // 프로젝트를 고른 뒤 "다음": 결제 계정 목록(고른 프로젝트를 넘겨 돌아온 뒤에도 남게).
  function loadBilling() {
    const billingAccounts = kit.current?.billingAccounts
    if (!billingAccounts) return
    setView({ kind: 'choosing' })
    void billingAccounts(selection()).then((accounts) => {
      if (accounts === REDIRECTING) return
      setChoices((now) => ({ projects: now?.projects ?? null, accounts }))
      setBilling(accounts.length === 1 ? accounts[0].name : '')
      setView({ kind: 'choose' })
    }).catch((error) => setView({ kind: 'listFailed', code: setupErrorStep(error) ?? 'other' }))
  }

  // 결제 화면에서 프로젝트를 바꾸려면 "다시 고르기": 프로젝트 목록을 다시(지금 고른 값을 넘긴다).
  function reloadProjects() {
    const projects = kit.current?.projects
    if (!projects) return
    setView({ kind: 'choosing' })
    void projects(selection()).then((list) => {
      if (list === REDIRECTING) return
      setChoices((now) => ({ projects: list, accounts: now?.accounts ?? null }))
      setView({ kind: 'choose' })
    }).catch((error) => setView({ kind: 'listFailed', code: setupErrorStep(error) ?? 'other' }))
  }

  // 멈춘 뒤 "다시 시도": Google 에서 돌아온 뒤의 일이면 같은 일을 같은 고르기로 다시(retryFromGesture), 아니면 이 화면에서 다시 시작.
  // otherwise: 다시 할 길이 없을 때. 설치가 멈춘 화면은 새로 시작, 돌아온 뒤 멈춘 화면(무슨 일인지 모름)은 첫 화면으로.
  function retry(otherwise: () => void = start) {
    const current = kit.current
    if (current?.retry && (current.retryInPage || resumed.current)) {
      setSteps({})
      setView({ kind: 'loading' })
      follow(current.retry, () => true, otherwise)
      return
    }
    otherwise()
  }

  function start() {
    const current = kit.current
    if (!current) return
    setNote(null)
    setSteps({})
    setView({ kind: 'running' })
    resumed.current = false
    current.start({ projectId: projectId || undefined, billingAccountName: billing || undefined }, record).then((value) => {
      if (value === REDIRECTING) return
      if (value.url) setView({ kind: 'done', url: value.url })
      else setView({ kind: 'failed', code: 'gatewayReady' })
    }).catch((error) => setView({ kind: 'failed', code: setupErrorStep(error) ?? 'other' }))
  }

  function find() {
    const current = kit.current
    if (!current) return
    setView({ kind: 'finding' })
    current.find().then((urls) => { if (urls !== REDIRECTING) setView({ kind: 'found', urls }) }).catch(() => setView({ kind: 'findFailed' }))
  }

  function copy(url: string) {
    void navigator.clipboard.writeText(url).then(() => setNote(words.copied)).catch(() => setNote(words.copyFailed))
  }

  const stepList = (
    <ol className="start-steps">
      {STEPS.map((step) => {
        const state = steps[step] ?? 'waiting'
        return (
          <li key={step} className={`is-${state}`}>
            <span className="start-step-mark" aria-hidden>
              {state === 'working' ? <LoaderCircle size={15} className="is-spinning" /> : state === 'done' ? <Check size={14} /> : state === 'failed' ? <X size={14} /> : state === 'pending' ? <Clock size={13} /> : null}
            </span>
            <span className="start-step-name">{words.steps[step]}</span>
            <span className="start-hidden">{words.stepState[state]}</span>
          </li>
        )
      })}
    </ol>
  )

  let body: ReactNode
  if (view.kind === 'loading') {
    body = <p className="start-status" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{words.preparing}</p>
  } else if (view.kind === 'unavailable') {
    body = <p className="start-problem" role="alert">{words.unavailable}</p>
  } else if (view.kind === 'intro') {
    body = (
      <>
        <header className="start-head">
          <img className="start-mark" src="/app-icons/icon-192.png" alt="" />
          <h1 ref={heading} tabIndex={-1}>{words.title}</h1>
          <p>{words.text}</p>
        </header>
        <section className="start-what" aria-label={words.whatTitle}>
          <h2>{words.whatTitle}</h2>
          <ol>{words.what.map((line) => <li key={line}>{line}</li>)}</ol>
        </section>
        <p className="start-cost">{words.cost}</p>
        <div className="start-actions">
          <button className="start-primary" type="button" onClick={begin}>
            <img src="/brand/storage/google-g.png" alt="" aria-hidden />{words.begin}
          </button>
          <button className="start-link" type="button" onClick={find}>{words.find}</button>
        </div>
      </>
    )
  } else if (view.kind === 'choosing') {
    body = (
      <header className="start-head">
        <h1 ref={heading} tabIndex={-1}>{words.chooseTitle}</h1>
        <p className="start-status" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{words.preparing}</p>
      </header>
    )
  } else if (view.kind === 'choose' && choices) {
    const { projects, accounts } = choices
    const canStart = Boolean(projectId) || Boolean(billing)
    const chosenProject = projects?.find((project) => project.projectId === projectId)
    body = (
      <>
        <header className="start-head">
          <h1 ref={heading} tabIndex={-1}>{words.chooseTitle}</h1>
          <p>{words.chooseText}</p>
        </header>
        <div className="start-choose">
          <label className="setting-field">
            <span>{words.project}</span>
            {projects ? (
              <select value={projectId} onChange={(event) => setProjectId(event.target.value)}>
                <option value="">{words.newProject}</option>
                {projects.map((project) => <option key={project.projectId} value={project.projectId}>{projectLabel(project)}</option>)}
              </select>
            ) : (
              // 결제 목록만 돌아왔을 때(S65): 고른 프로젝트를 그대로 보이고 바꾸려면 아래 "다시 고르기".
              <select value={projectId} disabled>
                <option value={projectId}>{projectId ? (chosenProject ? projectLabel(chosenProject) : projectId) : words.newProject}</option>
              </select>
            )}
          </label>
          {accounts && accounts.length > 0 && (
            <label className="setting-field">
              <span>{words.billingAccount}</span>
              <select value={billing} onChange={(event) => setBilling(event.target.value)}>
                {accounts.length > 1 && <option value="" disabled={!projectId}>{words.billingPick}</option>}
                {accounts.map((account) => <option key={account.name} value={account.name}>{account.displayName || account.name}</option>)}
              </select>
              {projectId && <small>{words.billingKept}</small>}
            </label>
          )}
        </div>
        {accounts && accounts.length === 0 && <p className="start-problem" role="alert">{words.billingNone}</p>}
        <div className="start-actions">
          {accounts ? (
            <button className="start-primary" type="button" disabled={!canStart} onClick={start}>{words.install}</button>
          ) : (
            <button className="start-primary" type="button" onClick={loadBilling}>{words.next}</button>
          )}
          {accounts && accounts.length === 0 && (
            <a className="start-secondary" href="https://console.cloud.google.com/billing" target="_blank" rel="noopener noreferrer">{words.openBilling}</a>
          )}
          {!projects && <button className="start-link" type="button" onClick={reloadProjects}>{words.chooseAgain}</button>}
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  } else if (view.kind === 'listFailed') {
    body = (
      <>
        <header className="start-head">
          <h1 ref={heading} tabIndex={-1}>{words.chooseTitle}</h1>
        </header>
        <p className="start-problem" role="alert">{words.errors[errorKind(view.code)]}</p>
        <div className="start-actions">
          <button className="start-primary" type="button" onClick={begin}>{words.retry}</button>
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  } else if (view.kind === 'running') {
    body = (
      <>
        <header className="start-head">
          <h1 ref={heading} tabIndex={-1}>{words.runningTitle}</h1>
          <p>{words.runningText}</p>
        </header>
        {stepList}
      </>
    )
  } else if (view.kind === 'failed') {
    const kind = errorKind(view.code)
    body = (
      <>
        <header className="start-head">
          <h1 ref={heading} tabIndex={-1}>{words.failedTitle}</h1>
        </header>
        {stepList}
        <p className="start-problem" role="alert">{words.errors[kind]}</p>
        <div className="start-actions">
          {CHOICE_KINDS.has(kind) && choices ? (
            <button className="start-primary" type="button" onClick={() => setView({ kind: 'choose' })}>{words.chooseAgain}</button>
          ) : CHOICE_KINDS.has(kind) && kit.current?.projects ? (
            <button className="start-primary" type="button" onClick={reloadProjects}>{words.chooseAgain}</button>
          ) : (
            <button className="start-primary" type="button" onClick={() => retry()}>{words.retry}</button>
          )}
          {kind === 'billing' && (
            <a className="start-secondary" href="https://console.cloud.google.com/billing" target="_blank" rel="noopener noreferrer">{words.openBilling}</a>
          )}
        </div>
      </>
    )
  } else if (view.kind === 'done') {
    const which = platform()
    body = (
      <>
        <header className="start-head">
          <span className="start-done-mark" aria-hidden><Check size={20} /></span>
          <h1 ref={heading} tabIndex={-1}>{words.doneTitle}</h1>
          <p>{words.doneText}</p>
        </header>
        <div className="start-address">
          <span className="start-address-label">{words.address}</span>
          <div className="start-address-row">
            <code>{view.url}</code>
            <button className="icon-button" type="button" aria-label={words.copy} title={words.copy} onClick={() => copy(view.url)}><Copy size={16} aria-hidden /></button>
          </div>
          {note && <p className="start-note" role="status">{note}</p>}
        </div>
        <div className="start-actions">
          <a className="start-primary" href={view.url}>{words.open}</a>
        </div>
        <section className="start-home" aria-labelledby="start-home-title">
          <h2 id="start-home-title">{words.homeTitle}</h2>
          <p>{words.homeText}</p>
          {(which === 'iphone' || which === 'both') && (
            <div className="start-home-guide">
              {which === 'both' && <h3>{words.iphone}</h3>}
              <ol>{words.iphoneSteps.map((line) => <li key={line}>{line}</li>)}</ol>
            </div>
          )}
          {(which === 'android' || which === 'both') && (
            <div className="start-home-guide">
              {which === 'both' && <h3>{words.android}</h3>}
              <ol>{words.androidSteps.map((line) => <li key={line}>{line}</li>)}</ol>
            </div>
          )}
          <p className="start-small">{words.homeNote}</p>
          <p className="start-small">{words.forgetNote}</p>
        </section>
      </>
    )
  } else if (view.kind === 'destroying') {
    body = (
      <header className="start-head">
        <h1 ref={heading} tabIndex={-1}>{words.destroyingTitle}</h1>
        <p>{words.destroyingText}</p>
        <p className="start-status" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{words.stepState.working}</p>
      </header>
    )
  } else if (view.kind === 'destroyed') {
    const partial = view.remaining.length > 0
    body = (
      <>
        <header className="start-head">
          {!partial && <span className="start-done-mark" aria-hidden><Check size={20} /></span>}
          <h1 ref={heading} tabIndex={-1}>{partial ? words.destroyPartialTitle : words.destroyedTitle}</h1>
          <p>{partial ? words.destroyPartialText : words.destroyedText}</p>
        </header>
        {partial && <ul className="start-remaining">{view.remaining.map((item) => <li key={item}>{words.destroyItems[item] ?? item}</li>)}</ul>}
        <div className="start-actions">
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  } else if (view.kind === 'resumeFailed') {
    body = (
      <>
        <p className="start-problem" role="alert">{words.errors[errorKind(view.code)]}</p>
        <div className="start-actions">
          <button className="start-primary" type="button" onClick={() => retry(() => setView({ kind: 'intro' }))}>{words.retry}</button>
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  } else if (view.kind === 'finding') {
    body = (
      <header className="start-head">
        <h1 ref={heading} tabIndex={-1}>{words.findingTitle}</h1>
        <p className="start-status" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{words.preparing}</p>
      </header>
    )
  } else if (view.kind === 'found') {
    body = (
      <>
        <header className="start-head">
          <h1 ref={heading} tabIndex={-1}>{words.foundTitle}</h1>
          <p>{view.urls.length ? words.foundText : words.foundNone}</p>
        </header>
        {view.urls.length > 0 && (
          <ul className="start-found">
            {view.urls.map((url) => (
              <li key={url}><code>{url}</code><a className="start-secondary" href={url}>{words.open}</a></li>
            ))}
          </ul>
        )}
        <div className="start-actions">
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  } else {
    body = (
      <>
        <p className="start-problem" role="alert">{words.findFailed}</p>
        <div className="start-actions">
          <button className="start-primary" type="button" onClick={find}>{words.retry}</button>
          <button className="start-link" type="button" onClick={() => setView({ kind: 'intro' })}>{words.back}</button>
        </div>
      </>
    )
  }

  return (
    <main className="start-page" aria-busy={view.kind === 'loading' || view.kind === 'choosing' || view.kind === 'running' || view.kind === 'finding' || view.kind === 'destroying'}>
      <div className="start-column">{body}</div>
    </main>
  )
}
