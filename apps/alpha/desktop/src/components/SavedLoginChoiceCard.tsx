import { Check } from 'lucide-react'
import { CHOICE_DONE, CHOICE_LABEL } from '../approvals'
import { siteHost, type LoginChoiceRequest } from '../saved-logins'
import { W } from '../words'
import { closedText } from './ApprovalCard'
import './ApprovalCard.css'

export type LoginChoiceItem = {
  request: LoginChoiceRequest
  /** 물음이 왔을 때 그 채팅방의 말 수(고른 뒤 남는 한 줄을 그 자리에 둔다). */
  at: number
  /**
   * cancelled: 선생님이 그만둠. closed: 고르기 전에 뒤쪽이 물음을 닫음(saved-login-selection-closed 의 timeout·cancelled,
   * 또는 고를 때 unavailable). 셋 다 그 자리에 한 줄로 남는다.
   */
  status: 'pending' | 'sending' | 'chosen' | 'cancelled' | 'closed'
  /** 고른(또는 보내는 중인) 로그인. */
  chosenId?: string
  /** 보내는 중인 것이 그만두기(saved_login_cancel). */
  cancelling?: boolean
  /** 닫힌 까닭(timeout 이면 승인 카드의 "답이 없어 …" 줄). */
  closedReason?: string | null
  error?: string | null
}

/**
 * 같은 로그인 화면에 저장된 로그인이 여럿이라 몸소가 고르지 못할 때(saved-login-selection-request) 채팅방 안에 뜨는 물음.
 * 지금 있는 몸소 브라우저 승인 카드의 꼴: 머리 글, 사이트, 로그인마다 단추 하나("이름 · 아이디"), 끝에 승인 카드의 거절 단추 자리·모양으로 "취소".
 * 고르거나 취소하거나 뒤쪽이 닫으면 승인 카드처럼 한 줄로 줄어 그 자리에 남는다.
 */
export function SavedLoginChoiceCard({ item, onChoose, onCancel }: { item: LoginChoiceItem; onChoose: (loginId: string) => void; onCancel?: () => void }) {
  const words = W.savedLogins.choice
  const { entries } = item.request
  const site = entries[0] ? siteHost(entries[0].origin) : null

  if (item.status === 'chosen' || item.status === 'cancelled' || item.status === 'closed') {
    const chosen = item.status === 'chosen' ? entries.find((entry) => entry.id === item.chosenId) : undefined
    const text = item.status === 'chosen' ? words.chosen
      : item.status === 'cancelled' ? CHOICE_DONE.cancel
        : closedText(item.closedReason, W.approvals.closed, W.approvals.closedTimeout)
    return (
      <p className="approval-done">
        {item.status !== 'closed' && <Check size={13} aria-hidden />}
        <span>{text}</span>
        {(chosen || site) && <span className="approval-done-subject">{chosen ? `${chosen.label} · ${site}` : site}</span>}
      </p>
    )
  }

  const sending = item.status === 'sending'
  return (
    <section className="approval-card is-setup is-browser" role="group" aria-label={words.aria} aria-busy={sending}>
      <div className="approval-head">
        <strong>{words.head}</strong>
      </div>
      {site && (
        <dl className="approval-facts">
          <div><dt>{W.approvalCard.browser.site}</dt><dd>{site}</dd></div>
        </dl>
      )}
      {item.error && <p className="approval-error" role="alert">{item.error}</p>}
      <div className="approval-actions">
        {entries.map((entry) => (
          <button key={entry.id} className="approval-button" type="button" disabled={sending} onClick={() => onChoose(entry.id)}>
            {entry.label} · {entry.username}
          </button>
        ))}
        {onCancel && (
          <button className="approval-button" type="button" disabled={sending} onClick={onCancel}>
            {CHOICE_LABEL.cancel}
          </button>
        )}
      </div>
    </section>
  )
}
