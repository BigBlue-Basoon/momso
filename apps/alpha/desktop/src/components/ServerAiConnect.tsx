import { useCallback, useEffect, useRef, useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { PROVIDER_NAME } from '../ai-models'
import { listenDeviceLoginAnywhere, type DeviceLogin } from '../device-login'
import { cancelProviderLogin, getProviderAuth, listenProviderAuth, setProviderApiKey, startProviderLogin, type ProviderAuth } from '../provider-auth'
import { W } from '../words'
import { DeviceLoginCard } from './DeviceLoginCard'
import { ProviderMark } from './ProviderMark'

/**
 * momso anywhere(네트워크 길)의 AI 연결(2026-11-21 본부 지시: 사용자가 휴대폰 화면 안에서 스스로 끝낸다).
 * 채팅을 보내 보고 실패하기 전에, 코덱스가 연결돼 있지 않으면 이 화면이 먼저 뜬다(ServerGate). 채팅 중 AI 를 못 써 실패해도 이리로 온다.
 * 데스크톱의 첫 실행 AI 연결 말(제목·상태·"코덱스에서 로그인"·"브라우저에서 로그인을 마치면 여기에 나타납니다.")과 로그인 코드 카드(DeviceLoginCard)를 그대로 쓴다.
 * 휴대폰 한 손: 한 단추로 코드를 복사하고 로그인 페이지를 새 탭으로 연다(VS Code 기기 로그인의 "Copy & Continue" 꼴. 코덱스 공식 기기 페이지는 먼저 로그인을 묻고
 * 그다음 코드를 받으므로 코드를 미리 복사해 둔다). 로그인을 마치고 이 탭으로 돌아오면 server-client.ts 가 바로 상태를 읽어 저절로 넘어간다.
 * 코드와 주소, server 가 준 로그인 세션 값은 화면 메모리에만 둔다(줄3 S25 계약, server-client.ts).
 * 로그인이 끝나면(provider-auth-changed 의 reason) 까닭을 한 줄로: 시간 지남 → 카드의 그 말, 코덱스 CLI 없음 → 채팅 실패 줄의 그 말,
 * 코드 전에 실패 → "AI 로그인을 시작하지 못했습니다", 코드 뒤 실패 → 설정의 "로그인이 끝나지 않았습니다". 연결됨·취소는 말 없음.
 * 클로드·제미나이(2026-11-22 본부 지시, server S55): 코덱스 줄과 같은 꼴의 줄 아래에 데스크톱 설정의 API 키 칸(ProviderSignIn 의 provider-key)을 그대로.
 * 키는 입력 칸에만 있고(React 상태에 두지 않음) 저장을 누르면 칸을 비운 뒤 server 에 한 번 넘긴다. 저장된 키는 다시 보이지 않는다.
 * credential_saved 는 키를 보관했다는 뜻이지 맞는 키라는 뜻이 아니라 "연결됨"이라 하지 않고 "API 키 저장됨"만 적는다. 이 server 에는 키를 지우는 길이 없어 "키 삭제"는 두지 않는다.
 */
const KEY_PROVIDERS = ['claude', 'gemini'] as const

function ServerKeyRow({ item, onSaved }: { item: ProviderAuth; onSaved: () => void }) {
  const input = useRef<HTMLInputElement>(null)
  const [filled, setFilled] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const name = PROVIDER_NAME[item.provider]
  const storage = item.runnable === false

  async function save() {
    const value = input.current?.value ?? ''
    if (input.current) input.current.value = ''
    setFilled(false)
    if (!value.trim()) return
    setError(null)
    setSaving(true)
    try {
      const state = await setProviderApiKey(item.provider, value)
      if (state === 'invalidKey') setError(W.signIn.invalidKey)
      else if (state === 'storageUnavailable') setError(W.serverAi.keyStorageUnavailable)
      else if (state !== 'keySaved') setError(W.signIn.keySaveFailed)
    } catch { setError(W.signIn.keySaveFailed) }
    setSaving(false)
    onSaved()
  }

  return (
    <div className="server-ai-row is-key">
      <ProviderMark provider={item.provider === 'gemini' ? 'gemini' : 'claude'} className="server-ai-mark" />
      <strong>{name}</strong>
      {!item.apiKeySaved && !storage && <span className="provider-state is-signedOut">{W.signIn.state.signedOut}</span>}
      {storage ? (
        <p className="provider-error" role="alert">{W.serverAi.keyStorageUnavailable}</p>
      ) : (
        <div className="provider-key">
          <span className="provider-key-label">{W.signIn.apiKey}</span>
          {item.apiKeySaved ? (
            <span className="provider-key-saved">{W.signIn.keySaved}</span>
          ) : (
            <form className="provider-key-form" onSubmit={(event) => { event.preventDefault(); void save() }}>
              <input
                ref={input}
                type="password"
                autoComplete="off"
                spellCheck={false}
                placeholder={W.signIn.keyPlaceholder}
                aria-label={`${name} ${W.signIn.apiKey}`}
                disabled={saving}
                onChange={(event) => setFilled(Boolean(event.target.value.trim()))}
              />
              <button className="secondary-button" type="submit" disabled={saving || !filled}>
                {saving ? <LoaderCircle size={13} className="is-spinning" aria-hidden /> : null}{W.signIn.keySave}
              </button>
            </form>
          )}
        </div>
      )}
      {error && !storage && <p className="provider-error" role="alert">{error}</p>}
    </div>
  )
}
function endNote(reason: string | null, codeShown: boolean) {
  if (!reason || reason === 'connected' || reason === 'cancelled') return null
  if (reason === 'expired') return W.deviceLogin.expired
  if (reason === 'cli_unavailable') return W.app.failure.serverCliUnavailable(PROVIDER_NAME.codex)
  return codeShown ? W.signIn.loginUnfinished : W.serverAi.startFailed
}

export function ServerAiConnect({ onConnected }: { onConnected: () => void }) {
  const [codex, setCodex] = useState<ProviderAuth | null | 'loading'>('loading')
  const [keyRows, setKeyRows] = useState<ProviderAuth[]>([])
  const [login, setLogin] = useState<DeviceLogin | null>(null)
  const [starting, setStarting] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const codeShown = useRef(false)

  const refresh = useCallback(() => {
    getProviderAuth().then((list) => {
      const item = list.find((entry) => entry.provider === 'codex') ?? null
      setCodex(item)
      setKeyRows(KEY_PROVIDERS.flatMap((provider) => list.filter((entry) => entry.provider === provider)))
      setProblem(null)
      if (item?.state === 'account') onConnected()
    }).catch(() => { setCodex(null); setProblem(W.firstRun.checkFailed) })
  }, [onConnected])
  useEffect(() => { refresh() }, [refresh])
  useEffect(() => {
    let disposed = false
    const stops: Array<() => void> = []
    const keep = (stop: () => void) => { if (disposed) stop(); else stops.push(stop) }
    void listenDeviceLoginAnywhere({
      onShow: (value) => { if (value.provider === 'codex') { codeShown.current = true; setLogin(value); setStarting(false) } },
      onClose: (provider) => { if (provider === 'codex') setLogin(null) },
    }).then(keep).catch(() => {})
    void listenProviderAuth((provider, _state, reason) => {
      if (provider !== 'codex') return
      setStarting(false)
      const text = endNote(reason, codeShown.current)
      if (text) setNote(text)
      refresh()
    }).then(keep).catch(() => {})
    return () => { disposed = true; stops.forEach((stop) => stop()) }
  }, [refresh])

  async function start() {
    setProblem(null)
    setNote(null)
    setStarting(true)
    codeShown.current = false
    try {
      const state = await startProviderLogin('codex')
      if (state !== 'inProgress') { setStarting(false); setProblem(W.serverAi.startFailed) }
    } catch {
      setStarting(false)
      setProblem(W.serverAi.startFailed)
    }
  }
  function copy(code: string) {
    void navigator.clipboard.writeText(code).then(() => setNote(W.deviceLogin.copied)).catch(() => setNote(W.deviceLogin.copyFailed))
  }
  // 누른 그 자리에서 복사와 새 탭 열기를 함께 한다(브라우저가 누름 없이 연 창·복사를 막지 않게).
  function copyAndOpen(value: DeviceLogin) {
    if (value.kind !== 'device') return
    copy(value.code)
    window.open(value.url, '_blank', 'noopener')
  }

  const connected = codex !== 'loading' && codex?.state === 'account'
  return (
    <div className="server-gate-card server-ai" role="region" aria-labelledby="server-ai-title">
      <h1 id="server-ai-title">{W.firstRun.aiTitle}</h1>
      <p>{W.serverAi.intro}</p>
      <div className="server-ai-row">
        <ProviderMark provider="chatgpt" className="server-ai-mark" />
        <strong>{PROVIDER_NAME.codex}</strong>
        {codex !== 'loading' && <span className={`provider-state is-${connected ? 'account' : 'signedOut'}`}>{W.signIn.state[connected ? 'account' : 'signedOut']}</span>}
      </div>
      {codex === 'loading' ? (
        <p className="server-gate-checking" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{W.firstRun.checking}</p>
      ) : login ? (
        <>
          <DeviceLoginCard
            inline
            login={login}
            openLabel={W.deviceLogin.copyAndOpen}
            onCopy={copy}
            onOpenPage={() => copyAndOpen(login)}
            onRestart={() => void start()}
            onCancel={() => { setLogin(null); setNote(null); void cancelProviderLogin('codex').catch(() => {}) }}
            onDismiss={() => setLogin(null)}
          />
          <p className="server-ai-hint">{W.signIn.browserHint}</p>
        </>
      ) : !connected ? (
        <button className="server-gate-button" type="button" disabled={starting} onClick={() => void start()}>
          {starting ? <><LoaderCircle size={15} className="is-spinning" aria-hidden />{W.signIn.loggingIn}</> : W.signIn.login.codex}
        </button>
      ) : null}
      {keyRows.map((item) => <ServerKeyRow key={item.provider} item={item} onSaved={refresh} />)}
      {note && <p className="server-ai-note" role="status">{note}</p>}
      {problem && (
        <p className="server-ai-problem" role="alert">
          {problem}
          {/* 같은 기능의 단추는 화면에 하나: 시작하지 못함에는 바로 위 "코덱스에서 로그인"이 다시 하기다. 상태를 읽지 못했을 때만 "다시 시도"(상태 다시 읽기). */}
          {problem === W.firstRun.checkFailed && <> <button className="secondary-button" type="button" onClick={refresh}>{W.firstRun.retry}</button></>}
        </p>
      )}
    </div>
  )
}
