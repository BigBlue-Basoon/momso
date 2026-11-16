import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { PROVIDER_NAME, PROVIDER_ORDER, type AiProvider } from '../ai-models'
import {
  cancelProviderLogin,
  clearProviderApiKey,
  getProviderAuth,
  listenProviderAuth,
  logoutProvider,
  setProviderApiKey,
  startProviderLogin,
  type ProviderAuth,
} from '../provider-auth'
import { W } from '../words'
import { ProviderMark, type ProviderMarkId } from './ProviderMark'
import { QuestionDialog } from './ZetPanel'

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

/** provider_auth_status 가 있는지 알아본다. 있으면 그 목록, 없으면 null(예전 연결 칸을 그대로 보인다). */
export function useProviderAuth() {
  const [list, setList] = useState<ProviderAuth[] | null | undefined>(undefined)
  const refresh = useCallback(() => {
    getProviderAuth().then(setList).catch(() => setList(null))
  }, [])
  useEffect(() => {
    refresh()
    let stop: (() => void) | undefined
    let disposed = false
    void listenProviderAuth(() => refresh()).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [refresh])
  return { list, refresh }
}

/**
 * 설정 "AI와 개인정보"의 AI 연결: 회사 셋마다 한 줄. 그 회사 계정으로 로그인(브라우저가 열림)과 API 키 넣기, 로그아웃과 키 지우기.
 * 쓸 수 없는 방법(methods 에 없음)은 단추·칸이 눌리지 않게만 한다. 로그아웃과 키 지우기는 먼저 묻는다.
 * API 키는 입력 칸에서 받아 바로 넘기고 칸을 비운다. 저장된 키는 다시 보이지 않는다("API 키 저장됨"만).
 */
export function ProviderSignIn({ list, onChanged }: { list: ProviderAuth[]; onChanged: () => void }) {
  const byProvider = new Map(list.map((item) => [item.provider, item]))
  return (
    <section className="settings-group provider-sign-in" aria-labelledby="provider-sign-in-title">
      <div className="settings-group-heading">
        <div>
          <h3 id="provider-sign-in-title">{W.signIn.title}</h3>
          <p>{W.signIn.intro}</p>
        </div>
      </div>
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
  const [asking, setAsking] = useState<'logout' | 'clear' | null>(null)
  const keyInput = useRef<HTMLInputElement>(null)
  const canAccount = item.methods.includes('account')
  const canKey = item.methods.includes('apiKey')
  // 그 회사의 도구에서 로그인한다는 단추 글. 없으면(제미나이) 계정 단추를 그리지 않는다.
  const loginLabel = W.signIn.login[item.provider] ?? null

  // 백엔드가 로그인을 마쳤다고 알리면(목록이 다시 읽혀 상태가 바뀌면) 기다림을 거둔다.
  useEffect(() => { if (item.state === 'account') setPending(false) }, [item.state])
  useEffect(() => listenOnce(item.provider, (state) => { if (state && state !== 'inProgress') setPending(false) }), [item.provider])

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
      if (state === 'invalidKey') setError(W.signIn.invalidKey)
      else if (state !== 'keySaved') setError(W.signIn.keySaveFailed)
    } catch { setError(W.signIn.keySaveFailed) }
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

  const stateText = item.state === 'unavailable' ? null : W.signIn.state[item.state]
  const account = [item.accountLabel, item.planLabel].filter(Boolean).join(' · ')
  return (
    <li className={`provider-row${item.state === 'unavailable' ? ' is-unavailable' : ''}`}>
      <ProviderMark provider={MARK[item.provider]} className="provider-row-mark" />
      <div className="provider-row-body">
        <div className="provider-row-head">
          <strong>{name}</strong>
          {stateText && <span className={`provider-state is-${item.state}`}>{stateText}</span>}
        </div>
        {item.state === 'account' && account && <p className="provider-account">{account}</p>}

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
          {item.apiKeySaved ? (
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
        {W.signIn.apiCost[item.provider] && <p className="provider-hint">{W.signIn.apiCost[item.provider]}</p>}
        {error && <p className="provider-error" role="alert">{error}</p>}
      </div>

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
function listenOnce(provider: AiProvider, onState: (state: string | null) => void) {
  let stop: (() => void) | undefined
  let disposed = false
  void listenProviderAuth((changed, state) => { if (changed === provider) onState(state) })
    .then((unlisten) => { if (disposed) unlisten(); else stop = unlisten })
    .catch(() => {})
  return () => { disposed = true; stop?.() }
}
