import { useRef, useState } from 'react'
import { Check, LockKeyhole } from 'lucide-react'
import type { SecretRequest } from '../setup-secrets'
import { W } from '../words'
import { siteHost } from '../saved-logins'
import { closedText, serviceName } from './ApprovalCard'
import './ApprovalCard.css'

export type SecretItem = {
  request: SecretRequest
  /** 물음이 왔을 때 그 채팅방의 말 수(끝난 뒤 남는 한 줄을 그 자리에 둔다). */
  at: number
  /** closed: 키를 넣기 전에 뒤쪽이 요청을 닫음(agent-setup-secret-closed). 그 자리에 한 줄로 남는다. */
  status: 'pending' | 'sending' | 'saved' | 'cancelled' | 'closed'
  /** 닫힌 까닭(timeout·provider_cancelled, 옛 값이면 null). */
  closedReason?: string | null
  /** 선생님이 이 채팅방의 답을 멈췄다(승인 카드와 같음). */
  stopping?: boolean
  error?: string | null
}

/**
 * 스킬로 하는 연결에서 키가 필요할 때 채팅방 안에 뜨는 보안 입력(agent-setup-secret-request).
 * 무엇을 위한 키인지(회사, 계정, 만들 이름), 뒤쪽이 준 칸들(가려서 입력), "저장"과 "그만두기".
 * Claude 앱이 "Claude는 비밀번호를 볼 수 없습니다"라고 알리듯, 넣은 값은 AI 에게 가지 않고 몸소가 보관한다는 것을 한 줄로 알린다.
 * 칸은 React 상태에 담지 않고(입력 칸 자체에만 있다) 보낼 때 읽어 넘긴 뒤 바로 비운다. 채팅 글·기록·화면의 기억 어디에도 남기지 않는다.
 * 휴대폰 몸소의 입력 물음(줄3 S32, request.kind): 인증 번호(one_time_code)·이 사이트의 로그인 저장(saved_login)도 같은 꼴로 받는다.
 * 머리 글·안내 줄·끝난 줄은 W.serverInput, 사이트는 승인 카드의 "사이트" 줄, 칸 이름은 열쇠 창의 말(이름·아이디·비밀번호), 인증 번호 칸은 one-time-code 자동 채움.
 */
export function SecretRequestCard({ item, onSubmit, onCancel }: {
  item: SecretItem
  onSubmit: (values: Record<string, string>) => void
  /** 그만두기(agent_setup_secret_cancel). 넘기지 않으면 "그만두기" 단추를 두지 않는다. */
  onCancel?: () => void
}) {
  const { request } = item
  const words = W.secretRequest
  const input = request.kind ? W.serverInput[request.kind] ?? null : null
  const where = serviceName(request.service)
  const site = request.origin ? siteHost(request.origin) : null
  const form = useRef<HTMLFormElement>(null)
  // 채운 칸 수만 센다(값은 담지 않는다). 모두 채우면 저장이 눌린다.
  const [filled, setFilled] = useState(0)

  if (input && (item.status === 'saved' || item.status === 'cancelled' || item.status === 'closed')) {
    return (
      <p className="approval-done">
        {item.status !== 'closed' && <Check size={13} aria-hidden />}
        <span>{item.status === 'saved' ? input.done : item.status === 'cancelled' ? input.cancelled : closedText(item.closedReason, input.closed, input.closedTimeout)}</span>
        {site && <span className="approval-done-subject">{site}</span>}
      </p>
    )
  }
  if (item.status === 'saved' || item.status === 'cancelled' || item.status === 'closed') {
    return (
      <p className="approval-done">
        {item.status !== 'closed' && <Check size={13} aria-hidden />}
        <span>{item.status === 'saved' ? words.saved : item.status === 'cancelled' ? words.cancelled : closedText(item.closedReason, words.closed, words.closedTimeout)}</span>
        <span className="approval-done-subject">{request.resourceName ? `${where} ${request.resourceName}` : where}</span>
      </p>
    )
  }

  function count() {
    const inputs = [...(form.current?.querySelectorAll('input') ?? [])]
    setFilled(inputs.filter((input) => input.value.trim()).length)
  }

  function submit() {
    const inputs = [...(form.current?.querySelectorAll<HTMLInputElement>('input[data-field]') ?? [])]
    const values: Record<string, string> = {}
    for (const input of inputs) values[input.dataset.field as string] = input.value
    // 넘긴 뒤 바로 비운다.
    for (const input of inputs) input.value = ''
    setFilled(0)
    onSubmit(values)
  }

  const sending = item.status === 'sending'
  const ready = request.fields.length > 0 && filled === request.fields.length
  return (
    <section className="approval-card is-secret" role="group" aria-label={input ? input.aria : words.aria} aria-busy={sending}>
      {/* 자물쇠는 "AI에게 보내지 않고 몸소가 보관합니다" 줄 앞에만 둔다(그 줄이 자물쇠의 뜻이다). 같은 그림을 두 번 두지 않는다(2026-11-16 본부). */}
      <div className="approval-head">
        <strong>{input ? input.title : words.title(where)}</strong>
      </div>
      <dl className="approval-facts">
        {/* 회사 이름은 머리 글에 있으니 다시 적지 않는다(같은 것을 두 번 두지 않음). */}
        {request.account && <div><dt>{W.approvalCard.setup.account}</dt><dd>{request.account}</dd></div>}
        {request.resourceName && <div><dt>{W.approvalCard.setup.name}</dt><dd className="approval-fact-name">{request.resourceName}</dd></div>}
        {input && site && <div><dt>{W.approvalCard.browser.site}</dt><dd>{site}</dd></div>}
      </dl>
      <form
        ref={form}
        className="secret-fields"
        autoComplete="off"
        onInput={count}
        onSubmit={(event) => { event.preventDefault(); if (ready && !sending) submit() }}
      >
        {request.fields.map((field) => (
          <label key={field.id} className="secret-field">
            <span className="secret-field-label">{input ? inputLabel(request.kind, field.id) : words.fieldLabel[request.service]?.[field.id] ?? field.label}</span>
            {!input && words.help[field.id] && <span className="secret-field-help">{words.help[field.id]}</span>}
            <input
              data-field={field.id}
              type={field.secret ? 'password' : 'text'}
              autoComplete={request.kind === 'one_time_code' ? 'one-time-code' : 'off'}
              spellCheck={false}
              disabled={sending}
            />
          </label>
        ))}
        <p className="secret-note"><LockKeyhole size={12} aria-hidden />{input ? input.note : words.note}</p>
        {item.error && <p className="approval-error" role="alert">{item.error}</p>}
        <div className="approval-actions">
          {onCancel && <button className="approval-button" type="button" disabled={sending} onClick={onCancel}>{words.cancel}</button>}
          <button className="approval-button is-primary" type="submit" disabled={sending || !ready}>{input ? input.submit : words.save}</button>
        </div>
      </form>
    </section>
  )
}

/** 휴대폰 몸소 입력 칸의 이름: 인증 번호, 로그인 저장은 열쇠 창의 이름·아이디·비밀번호. 모르는 칸은 받은 이름. */
function inputLabel(kind: string | null | undefined, id: string) {
  if (kind === 'one_time_code' && id === 'code') return W.serverInput.one_time_code.code ?? id
  if (id === 'label') return W.savedLogins.fieldName
  if (id === 'username') return W.savedLogins.fieldUsername
  if (id === 'password') return W.savedLogins.fieldPassword
  return id
}
