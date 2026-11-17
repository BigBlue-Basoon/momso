import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { PROVIDER_NAME, PROVIDER_ORDER, type AiProvider } from '../ai-models'
import {
  cancelProviderLogin,
  clearProviderApiKey,
  getProviderAuth,
  installProvider,
  keyConnected,
  keyProblem,
  listenInstallProgress,
  listenProviderAuth,
  logoutProvider,
  setProviderApiKey,
  startProviderLogin,
  verifyProviderApiKey,
  type ProviderAuth,
} from '../provider-auth'
import { W } from '../words'
import { isMissingCommand } from '../ai-models'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import { QuestionDialog } from './ZetPanel'

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

/** provider_auth_status 가 있는지 알아본다. 있으면 그 목록, 없으면 null(예전 연결 칸을 그대로 보인다). */
export function useProviderAuth() {
  const [list, setList] = useState<ProviderAuth[] | null | undefined>(undefined)
  // 명령은 있는데 읽지 못했으면(명령이 없는 것과 다르게) 설정 "AI 연결"에서 알리고 다시 시도하게 한다(2026-11-17 틀렸을 때 점검).
  const [failed, setFailed] = useState(false)
  const refresh = useCallback(() => {
    getProviderAuth().then((value) => { setList(value); setFailed(false) }).catch((error) => { setList(null); setFailed(!isMissingCommand(error)) })
  }, [])
  useEffect(() => {
    refresh()
    let stop: (() => void) | undefined
    let disposed = false
    void listenProviderAuth(() => refresh()).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [refresh])
  return { list, refresh, failed }
}

/**
 * AI 연결: 회사 셋마다 한 줄. 설정 "AI 연결"과 첫 실행 화면이 같은 이 부품을 쓴다.
 * 그 회사의 도구가 이 컴퓨터에 없으면(installed=false) 설치부터 하게 한다(한 번 묻고, 설치 중과 실패를 보인다).
 * 설치된 뒤에는 그 회사 계정으로 로그인(브라우저가 열림)과 API 키 넣기, 로그아웃과 키 지우기.
 * 쓸 수 없는 방법(methods 에 없음)은 단추·칸이 눌리지 않게만 한다. 로그아웃과 키 지우기는 먼저 묻는다.
 * API 키는 입력 칸에서 받아 바로 넘기고 칸을 비운다. 저장된 키는 다시 보이지 않는다("API 키 저장됨"만).
 */
export function ProviderSignIn({ list, onChanged }: { list: ProviderAuth[]; onChanged: () => void }) {
  const byProvider = new Map(list.map((item) => [item.provider, item]))
  return (
    // 쪽 제목 "AI 연결"이 위에 있으니 같은 제목을 다시 두지 않는다(2026-11-16). 안내 한 줄은 쪽 머리말로 옮겼다.
    <section className="settings-group provider-sign-in" aria-label={W.settingsView.text4}>
      <ul className="provider-rows">
        {PROVIDER_ORDER.map((provider) => {
          const item = byProvider.get(provider)
          return item ? <ProviderRow key={provider} item={item} onChanged={onChanged} /> : null
        })}
      </ul>
    </section>
  )
}

function ProviderRow({ item, onChanged }: { item: ProviderAuth; onChanged: () => void }) {
  const name = PROVIDER_NAME[item.provider]
  const [pending, setPending] = useState(false)
  const [key, setKey] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [asking, setAsking] = useState<'logout' | 'clear' | 'install' | null>(null)
  // 설치: idle → working(설치하고 있습니다) → 끝나면 상태를 다시 읽는다. 실패면 까닭(reason)에 맞는 한 줄.
  // 뒤쪽이 unavailable 을 주거나 명령이 없으면 단추만 눌리지 않게 한다(SCREEN_RULES 1절).
  // 받은 양은 뒤쪽이 모르면 null 이라, 둘 다 숫자로 올 때만 막대를 보인다(퍼센트를 지어내지 않는다).
  const [install, setInstall] = useState<'idle' | 'working' | 'failed' | 'unavailable'>('idle')
  const [installReason, setInstallReason] = useState<string | null>(null)
  const [installBytes, setInstallBytes] = useState<{ received: number; total: number } | null>(null)
  const keyInput = useRef<HTMLInputElement>(null)
  const canAccount = item.methods.includes('account')
  const canKey = item.methods.includes('apiKey')
  // 그 회사의 도구에서 로그인한다는 단추 글. 없으면(제미나이) 계정 단추를 그리지 않는다.
  const loginLabel = W.signIn.login[item.provider] ?? null

  // 백엔드가 로그인을 마쳤다고 알리면(목록이 다시 읽혀 상태가 바뀌면) 기다림을 거둔다.
  useEffect(() => { if (item.state === 'account') setPending(false) }, [item.state])
  useEffect(() => { if (item.installed) setInstall('idle') }, [item.installed])
  // 로그인하는 중이던 것이 계정 연결(account)도 그만두기(cancelled)도 아닌 값으로 끝나면(5분 지남도 unavailable 로 온다) 말없이 거두지 않고 한 줄을 남긴다(2026-11-17 본부).
  const pendingRef = useRef(false)
  pendingRef.current = pending
  useEffect(() => listenOnce(item.provider, (state, reason) => {
    if (!state || state === 'inProgress') return
    // 시간 안에 끝나지 않은 것은 뒤쪽이 reason timeout 으로 가른다(2026-11-17 계약). 그 밖의 실패는 지금 글.
    if (pendingRef.current && state !== 'account' && state !== 'cancelled') setError(reason === 'timeout' ? W.signIn.loginTimeout : W.signIn.loginUnfinished)
    setPending(false)
  }), [item.provider])

  // 설치 진행 이벤트(이 회사 것만). 다른 곳에서 시작한 설치도 여기서 보인다.
  const changedRef = useRef(onChanged)
  changedRef.current = onChanged
  useEffect(() => {
    let stop: (() => void) | undefined
    let disposed = false
    void listenInstallProgress((progress) => {
      if (progress.provider !== item.provider) return
      if (progress.phase === 'installing') {
        setInstall('working')
        setInstallBytes(typeof progress.receivedBytes === 'number' && typeof progress.totalBytes === 'number' && progress.totalBytes > 0
          ? { received: progress.receivedBytes, total: progress.totalBytes } : null)
      } else if (progress.phase === 'verifying') {
        // 설치한 도구가 실행되는지 확인하는 중(실패가 아니다). 지금 있는 "설치하고 있습니다" 꼴 그대로 기다린다.
        setInstall('working')
        setInstallBytes(null)
      } else if (progress.phase === 'completed') {
        setInstallBytes(null)
        changedRef.current()
      } else {
        setInstallBytes(null)
        if (progress.reason === 'unavailable') setInstall('unavailable')
        else { setInstallReason(progress.reason); setInstall('failed') }
        changedRef.current()
      }
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [item.provider])

  async function runInstall() {
    setAsking(null)
    setError(null)
    setInstallReason(null)
    setInstall('working')
    try {
      const state = await installProvider(item.provider)
      if (state === 'unavailable') { setInstall('unavailable'); return }
      // 다른 설치가 진행 중이면 완료 이벤트를 기다린다(위의 듣기가 받는다).
      if (state === 'inProgress') return
      const fresh = (await getProviderAuth()).find((entry) => entry.provider === item.provider)
      if (fresh?.installed) setInstall('idle')
      else { setInstallReason((current) => current ?? 'installFailed'); setInstall('failed') }
    } catch (problem) {
      setInstall(/not found/i.test(String(problem)) ? 'unavailable' : 'failed')
    }
    onChanged()
  }

  async function login() {
    setError(null)
    try {
      const state = await startProviderLogin(item.provider)
      if (state === 'inProgress') setPending(true)
      else setError(W.signIn.loginFailed)
    } catch { setError(W.signIn.loginFailed) }
  }
  async function cancel() {
    await cancelProviderLogin(item.provider).catch(() => null)
    setPending(false)
    onChanged()
  }
  async function logout() {
    setAsking(null)
    setError(null)
    try {
      const state = await logoutProvider(item.provider)
      if (state !== 'signedOut') setError(W.signIn.logoutFailed)
    } catch { setError(W.signIn.logoutFailed) }
    onChanged()
  }
  async function saveKey() {
    const value = key
    setKey('')
    if (keyInput.current) keyInput.current.value = ''
    if (!value.trim()) return
    setError(null)
    try {
      const state = await setProviderApiKey(item.provider, value)
      // 새 뒤쪽은 저장한 뒤 확인을 시작하며 'verifying' 을 준다(예전 뒤쪽은 'keySaved'). 확인 결과는 provider-auth-changed 로 온다.
      if (state === 'invalidKey') setError(W.signIn.invalidKey)
      else if (state !== 'verifying' && state !== 'keySaved') setError(W.signIn.keySaveFailed)
    } catch { setError(W.signIn.keySaveFailed) }
    onChanged()
  }
  async function verifyKey() {
    setError(null)
    try {
      const state = await verifyProviderApiKey(item.provider)
      if (state !== 'verifying') setError(W.signIn.keyCheckFailed)
    } catch { setError(W.signIn.keyCheckFailed) }
    onChanged()
  }
  async function clearKey() {
    setAsking(null)
    setError(null)
    try {
      const state = await clearProviderApiKey(item.provider)
      if (state !== 'keyCleared') setError(W.signIn.keyClearFailed)
    } catch { setError(W.signIn.keyClearFailed) }
    onChanged()
  }

  const installed = item.installed
  // 제미나이는 키가 없을 때 state 가 unavailable 로 오지만 API 키는 넣을 수 있다(methods ['apiKey'], 2026-11-16 실제 앱 값). 그때는 "연결 안 됨"으로 본다.
  // 저장된 키가 아직 맞는지 모르거나(확인하는 중) 틀렸거나 확인하지 못했으면 "API 키로 연결됨"이라 하지 않는다(2026-11-17 계약 "AI 연결 확인").
  const problem = keyProblem(item)
  const keyState = item.state === 'apiKey' || item.apiKeySaved
    ? (keyConnected(item) ? 'apiKey' : problem === 'verifying' ? 'verifying' : 'signedOut')
    : null
  const baseState = item.state === 'unavailable' && (canAccount || canKey) ? 'signedOut' : item.state
  const shownState = item.state === 'account' ? 'account' : keyState ?? (baseState === 'apiKey' ? 'signedOut' : baseState)
  const stateText = !installed ? W.signIn.state.notInstalled : shownState === 'unavailable' ? null : W.signIn.state[shownState]
  const account = [item.accountLabel, item.planLabel].filter(Boolean).join(' · ')
  return (
    <li className={`provider-row${installed && shownState === 'unavailable' ? ' is-unavailable' : ''}`}>
      <ProviderMark provider={MARK[item.provider]} className="provider-row-mark" />
      <div className="provider-row-body">
        <div className="provider-row-head">
          <strong>{name}</strong>
          {stateText && <span className={`provider-state is-${installed ? shownState : 'notInstalled'}`}>{stateText}</span>}
        </div>
        {installed && item.state === 'account' && account && <p className="provider-account">{account}</p>}

        {/* 도구가 없으면 설치만 보인다. 로그인과 API 키는 설치한 뒤에 나타난다. */}
        {!installed && (
          <div className="provider-actions">
            {install === 'working' ? (
              <span className="provider-pending" role="status">
                <LoaderCircle size={14} className="is-spinning" aria-hidden />{W.signIn.installing}
                {installBytes && (
                  <span className="provider-install-bar" aria-hidden><span style={{ width: `${Math.min(100, (installBytes.received / installBytes.total) * 100)}%` }} /></span>
                )}
              </span>
            ) : (
              <button className="secondary-button" type="button" disabled={install === 'unavailable'} onClick={() => setAsking('install')}>
                {install === 'failed' ? W.signIn.installRetry : W.signIn.install}
              </button>
            )}
          </div>
        )}
        {!installed && install === 'failed' && <p className="provider-error" role="alert">{(installReason && W.signIn.installReason[installReason]) || W.signIn.installFailed}</p>}

        {installed && <>
        {/* 제미나이는 계정 로그인을 받지 않아(뒤쪽이 unavailable) 계정 단추를 두지 않고 API 키 칸만 둔다(2026-11-16 김성균 개발자가 정함). */}
        {loginLabel && (
          <div className="provider-actions">
            {item.state === 'account' ? (
              <button className="secondary-button" type="button" disabled={!canAccount} onClick={() => setAsking('logout')}>{W.signIn.logout}</button>
            ) : pending ? (
              <>
                <span className="provider-pending"><LoaderCircle size={14} className="is-spinning" aria-hidden />{W.signIn.loggingIn}</span>
                <button className="secondary-button" type="button" onClick={() => void cancel()}>{W.signIn.cancel}</button>
              </>
            ) : (
              <button className="secondary-button" type="button" disabled={!canAccount} onClick={() => void login()}>{loginLabel}</button>
            )}
          </div>
        )}
        {pending && <p className="provider-hint">{W.signIn.browserHint}</p>}

        <div className="provider-key">
          <span className="provider-key-label">{W.signIn.apiKey}</span>
          {item.apiKeySaved && problem !== 'rejected' ? (
            <>
              <span className="provider-key-saved">{W.signIn.keySaved}</span>
              <button className="secondary-button" type="button" disabled={!canKey} onClick={() => setAsking('clear')}>{W.signIn.keyClear}</button>
            </>
          ) : (
            <form className="provider-key-form" onSubmit={(event) => { event.preventDefault(); void saveKey() }}>
              <input
                ref={keyInput}
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder={W.signIn.keyPlaceholder}
                aria-label={`${name} ${W.signIn.apiKey}`}
                disabled={!canKey}
                value={key}
                onChange={(event) => setKey(event.target.value)}
              />
              <button className="secondary-button" type="submit" disabled={!canKey || !key.trim()}>{W.signIn.keySave}</button>
            </form>
          )}
        </div>
        {/* 틀린 키: 저장은 됐지만 연결되지 않았다. 위 칸에 새 키를 넣으면 바뀐다. 확인하지 못함(인터넷 등)은 틀렸다고 하지 않고 다시 확인하게 한다. */}
        {installed && problem === 'rejected' && (
          <div className="provider-key-problem" role="alert">
            <p className="provider-error">{W.signIn.keyRejected}</p>
            <button className="secondary-button" type="button" disabled={!canKey} onClick={() => setAsking('clear')}>{W.signIn.keyClear}</button>
          </div>
        )}
        {installed && problem === 'unavailable' && (
          <div className="provider-key-problem" role="alert">
            <p className="provider-error">{W.signIn.keyUnchecked}</p>
            <button className="secondary-button" type="button" disabled={!canKey} onClick={() => void verifyKey()}>{W.signIn.keyRetry}</button>
          </div>
        )}
        {W.signIn.apiCost[item.provider] && <p className="provider-hint">{W.signIn.apiCost[item.provider]}</p>}
        {error && <p className="provider-error" role="alert">{error}</p>}
        </>}
      </div>

      {asking === 'install' && (
        <QuestionDialog
          title={W.signIn.installTitle(name)}
          text={W.signIn.installText}
          actions={[
            { label: W.signIn.cancel, onSelect: () => setAsking(null) },
            { label: W.signIn.install, primary: true, autoFocus: true, onSelect: () => void runInstall() },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}

      {asking === 'logout' && (
        <QuestionDialog
          title={W.signIn.logoutTitle(name)}
          text={W.signIn.logoutText}
          actions={[
            { label: W.signIn.cancel, onSelect: () => setAsking(null), autoFocus: true },
            { label: W.signIn.logout, primary: true, onSelect: () => void logout() },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
      {asking === 'clear' && (
        <QuestionDialog
          title={W.signIn.clearTitle(name)}
          text={W.signIn.clearText}
          actions={[
            { label: W.signIn.cancel, onSelect: () => setAsking(null), autoFocus: true },
            { label: W.signIn.keyClear, primary: true, onSelect: () => void clearKey() },
          ]}
          onDismiss={() => setAsking(null)}
        />
      )}
    </li>
  )
}

/** 한 회사의 provider-auth-changed 를 듣는다(정리 함수를 돌려준다). */
function listenOnce(provider: AiProvider, onState: (state: string | null, reason: string | null) => void) {
  let stop: (() => void) | undefined
  let disposed = false
  void listenProviderAuth((changed, state, reason) => { if (changed === provider) onState(state, reason) })
    .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten })
    .catch(() => {})
  return () => { disposed = true; stop?.() }
}
