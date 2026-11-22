import { useEffect, useState, type CSSProperties } from 'react'
import { Copy, ExternalLink } from 'lucide-react'
import type { DeviceLogin } from '../device-login'
import { W } from '../words'
import './DeviceLoginCard.css'

function providerName(provider: string) {
  return (W.deviceLogin.names as Record<string, string>)[provider] ?? provider
}

function left(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/**
 * 로그인 코드 카드(2026-11-18). VS Code·GitHub CLI·Codex CLI 의 장치 코드 로그인처럼 코드를 크게 한 번 보이고 복사 단추를 둔다.
 * 남은 시간을 세고, 로그인 페이지가 패널에 아직 안 열렸으면 [로그인 페이지 열기]. 클로드는 기본 브라우저로 연다는 안내만.
 * 뒤쪽이 닫힘을 보내면(provider-login-device-closed) 앱이 이 카드를 거둔다.
 */
export function DeviceLoginCard({ login, style, inline = false, openLabel, onCopy, onOpenPage, onRestart, onCancel, onDismiss }: {
  login: DeviceLogin
  style?: CSSProperties
  /** 화면 안에 놓는 꼴(momso anywhere 의 AI 연결 화면). 기본은 창 오른쪽 아래에 뜨는 카드. */
  inline?: boolean
  /** 여는 단추의 글(기본 "로그인 페이지 열기"). */
  openLabel?: string
  onCopy: (code: string) => void
  /** 있으면 [로그인 페이지 열기](패널에 아직 안 열렸을 때). */
  onOpenPage?: () => void
  /** 있으면 코드 시간이 지났을 때 [코드 다시 받기]. */
  onRestart?: () => void
  onCancel: () => void
  onDismiss: () => void
}) {
  const [now, setNow] = useState(() => Date.now())
  const expiresAt = login.kind === 'device' ? login.expiresAt : null
  useEffect(() => {
    if (!expiresAt) return
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [expiresAt])
  const name = providerName(login.provider)

  if (login.kind === 'browser') {
    return (
      <section className="device-login" role="dialog" aria-label={W.deviceLogin.title(name)} style={style}>
        <strong className="device-login-title">{W.deviceLogin.title(name)}</strong>
        <p>{W.deviceLogin.browserText(name)}</p>
        <div className="device-login-actions">
          <button className="approval-button is-primary" type="button" onClick={onDismiss}>{W.deviceLogin.ok}</button>
        </div>
      </section>
    )
  }

  const expired = expiresAt !== null && expiresAt <= now
  return (
    <section className={inline ? 'device-login is-inline' : 'device-login'} role={inline ? 'group' : 'dialog'} aria-label={W.deviceLogin.title(name)} style={style}>
      <strong className="device-login-title">{W.deviceLogin.title(name)}</strong>
      <p>{expired ? W.deviceLogin.expired : W.deviceLogin.text}</p>
      <div className={expired ? 'device-login-code is-expired' : 'device-login-code'}>
        <code aria-label={W.deviceLogin.codeLabel}>{login.code}</code>
        {!expired && (
          <button className="secondary-button" type="button" onClick={() => onCopy(login.code)}>
            <Copy size={13} aria-hidden />{W.deviceLogin.copy}
          </button>
        )}
      </div>
      {expiresAt !== null && !expired && <p className="device-login-left">{W.deviceLogin.left(left(expiresAt - now))}</p>}
      <div className="device-login-actions">
        {onOpenPage && !expired && (
          <button className="approval-button is-primary" type="button" onClick={onOpenPage}>
            <ExternalLink size={13} aria-hidden />{openLabel ?? W.deviceLogin.openPage}
          </button>
        )}
        {onRestart && expired && (
          <button className="approval-button is-primary" type="button" onClick={onRestart}>{W.deviceLogin.restart}</button>
        )}
        <button className="approval-button" type="button" onClick={onCancel}>{W.deviceLogin.cancel}</button>
      </div>
    </section>
  )
}
