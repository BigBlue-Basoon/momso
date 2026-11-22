import { useCallback, useEffect, useState, type ReactNode } from 'react'
import { LoaderCircle } from 'lucide-react'
import { aiConnection, checkServerHealth, onServerAiNeeded, onServerSignedOut } from '../server-client'
import { W } from '../words'
import { ServerAiConnect } from './ServerAiConnect'
import './NetworkHost.css'

type GateState = 'checking' | 'ready' | 'signed_out' | 'unreachable' | 'ai'

/**
 * 네트워크 길(momso server, 같은 출처) 화면의 문(2026-11-21 S19b 계약: 토큰·주소를 묻지 않는다. 로그인은 앞단 Cloudflare Access 가 맡는다).
 * 열 때 GET /health 를 확인하고, 되면 바로 채팅 화면. 앞단 로그인이 풀리면(처음이든 쓰는 중이든) "로그인이 풀렸습니다"와 다시 로그인
 * (새로 불러오면 앞단 로그인 문이 다시 묻는다). 서버에 닿지 못하면(VM 이 꺼짐·인터넷 끊김) 알리고 다시 시도.
 * 쓰는 중에 풀리면 채팅 화면은 그대로 두고 그 위를 덮는다. 카드에 그림을 두지 않는다(로그인이 풀리면 앞단이 그림 파일도 막아 깨진 그림이 된다).
 * AI 연결(2026-11-21): 서버가 되면 코덱스가 연결돼 있는지 보고, 아니면 채팅 전에 AI 연결 화면(ServerAiConnect). 채팅이 AI 를 못 써 실패하고 연결돼 있지 않아도
 * 그 화면이 채팅 위를 덮는다. 연결되면 저절로 채팅으로. server 에 AI 연결이 아직 없으면 묻지 않는다.
 */
export function ServerGate({ children }: { children: ReactNode }) {
  const words = W.serverGate
  const [state, setState] = useState<GateState>('checking')
  const [opened, setOpened] = useState(false)

  const check = useCallback(() => {
    setState('checking')
    void checkServerHealth().then(async (result) => {
      if (result !== 'ready') { setState(result); return }
      if ((await aiConnection()) === 'needed') { setState('ai'); return }
      setState('ready')
      setOpened(true)
    })
  }, [])
  const connected = useCallback(() => { setState('ready'); setOpened(true) }, [])
  useEffect(() => { check() }, [check])
  useEffect(() => onServerSignedOut(() => setState('signed_out')), [])
  useEffect(() => onServerAiNeeded(() => setState((now) => (now === 'signed_out' ? now : 'ai'))), [])
  // 휴대폰에서 다른 앱을 쓰다 돌아오면(30초 넘게 뒤에 있었을 때) 조용히 서버를 다시 본다: 앞단 로그인이 풀렸으면 글을 쓰기 전에 바로 알리고,
  // 닿지 못하면(VM 이 꺼짐) 다시 시도 카드, 다시 되면 그 카드를 거둔다.
  useEffect(() => {
    let hiddenAt = 0
    const onChange = () => {
      if (document.visibilityState === 'hidden') { hiddenAt = Date.now(); return }
      if (!hiddenAt || Date.now() - hiddenAt < 30_000) return
      hiddenAt = 0
      void checkServerHealth().then((result) => {
        setState((now) => (now === 'ai' || now === 'checking' ? now : result === 'ready' ? (now === 'unreachable' ? 'ready' : now) : result))
      })
    }
    document.addEventListener('visibilitychange', onChange)
    return () => document.removeEventListener('visibilitychange', onChange)
  }, [])

  if (state === 'checking' && !opened) {
    return (
      <main className="server-gate" aria-busy="true">
        <p className="server-gate-checking" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{words.checking}</p>
      </main>
    )
  }
  const card = state === 'signed_out' ? (
    <div className="server-gate-card" role="alertdialog" aria-modal="true" aria-labelledby="server-gate-title" aria-describedby="server-gate-text">
      <h1 id="server-gate-title">{words.signedOutTitle}</h1>
      <p id="server-gate-text">{words.signedOutText}</p>
      <button className="server-gate-button" type="button" autoFocus onClick={() => window.location.reload()}>{words.signIn}</button>
    </div>
  ) : state === 'ai' ? (
    <ServerAiConnect onConnected={connected} />
  ) : state === 'unreachable' ? (
    <div className="server-gate-card" role="alertdialog" aria-modal="true" aria-labelledby="server-gate-title" aria-describedby="server-gate-text">
      <h1 id="server-gate-title">{words.unreachableTitle}</h1>
      <p id="server-gate-text">{words.unreachableText}</p>
      <button className="server-gate-button" type="button" autoFocus onClick={check}>{W.firstRun.retry}</button>
    </div>
  ) : null

  if (!opened) return <main className="server-gate">{card}</main>
  return (
    <>
      {children}
      {card && <div className="server-gate server-gate-cover">{card}</div>}
    </>
  )
}
