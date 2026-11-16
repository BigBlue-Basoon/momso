import { useEffect, useRef, useState } from 'react'
import { LoaderCircle } from 'lucide-react'
import { FirstLaunch } from './FirstLaunch'
import { ProviderSignIn, useProviderAuth } from './ProviderSignIn'
import { PROVIDER_NAME, PROVIDER_ORDER } from '../ai-models'
import { isConnected } from '../provider-auth'
import { checkUsername, markFirstRunDone, setUsername } from '../first-run'
import { W } from '../words'
import './FirstRun.css'

type Step = 'brand' | 'name' | 'ai'

/**
 * 첫 실행: 몸소 첫 화면 → (이름 명령이 있으면) 영어 사용자 이름 → AI 하나 이상 연결 → 작업 화면.
 * 방향 문서 22줄: 셋 중 하나도 연결하지 않으면 다음으로 갈 수 없다. 건너뛰기 단추를 두지 않는다. 순서는 ChatGPT → 클로드 → 제미나이.
 * AI 연결 칸은 설정 "AI 연결"과 같은 부품(ProviderSignIn)이다. 도구가 없으면 거기서 설치부터 한다.
 */
export function FirstRun({ username, nameStep, onDone }: { username: string | null; nameStep: boolean; onDone: () => void }) {
  const [step, setStep] = useState<Step>('brand')
  const askName = nameStep && !username
  const afterBrand: Step = askName ? 'name' : 'ai'

  if (step === 'brand') return <FirstLaunch onStart={() => setStep(afterBrand)} />
  return (
    <main className="first-run">
      {step === 'name'
        ? <NameStep onNext={() => setStep('ai')} />
        : <AiStep onBack={askName ? () => setStep('name') : null} onDone={() => { markFirstRunDone(); onDone() }} />}
    </main>
  )
}

function NameStep({ onNext }: { onNext: () => void }) {
  const [value, setValue] = useState('')
  const [problem, setProblem] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => { input.current?.focus() }, [])

  const check = checkUsername(value.trim())
  // 쓸 수 없는 글자는 치는 동안 바로 알린다. 길이·끝말은 뒤쪽이 거절할 때 알린다.
  const shown = check === 'chars' ? W.firstRun.nameChars : problem

  async function save() {
    const name = value.trim()
    const result = checkUsername(name)
    if (result !== 'ok') { setProblem(result === 'chars' ? W.firstRun.nameChars : result === 'other' ? W.firstRun.nameRejected : null); return }
    setSaving(true)
    setProblem(null)
    try {
      const reply = await setUsername(name)
      if (reply.state === 'ok') { onNext(); return }
      setProblem(reply.state === 'invalid_username' ? W.firstRun.nameRejected : W.firstRun.nameSaveFailed)
    } catch {
      setProblem(W.firstRun.nameSaveFailed)
    }
    setSaving(false)
  }

  return (
    <form className="first-run-step" onSubmit={(event) => { event.preventDefault(); void save() }}>
      <div className="first-run-head">
        <h1>{W.firstRun.nameTitle}</h1>
        <p>{W.firstRun.nameText}</p>
      </div>
      <input
        ref={input}
        className="first-run-input"
        aria-label={W.firstRun.nameLabel}
        aria-invalid={shown ? true : undefined}
        autoComplete="off"
        autoCapitalize="off"
        spellCheck={false}
        maxLength={94}
        value={value}
        onChange={(event) => { setValue(event.target.value); setProblem(null) }}
      />
      {shown && <p className="first-run-error" role="alert">{shown}</p>}
      <div className="first-run-actions">
        <button className="confirm-button first-run-primary" type="submit" disabled={saving || check === 'empty' || check === 'chars'}>
          {saving && <LoaderCircle size={15} className="is-spinning" aria-hidden />}{W.firstRun.next}
        </button>
      </div>
    </form>
  )
}

function AiStep({ onBack, onDone }: { onBack: (() => void) | null; onDone: () => void }) {
  const { list, refresh } = useProviderAuth()
  const connected = Array.isArray(list) && list.some(isConnected)
  const [a, b, c] = PROVIDER_ORDER.map((id) => PROVIDER_NAME[id])
  return (
    <div className="first-run-step is-wide">
      <div className="first-run-head">
        <h1>{W.firstRun.aiTitle}</h1>
        <p>{W.firstRun.aiText(a, b, c)}</p>
      </div>
      {list === undefined && (
        <p className="first-run-checking" role="status"><LoaderCircle size={15} className="is-spinning" aria-hidden />{W.firstRun.checking}</p>
      )}
      {list === null && (
        <div className="first-run-problem" role="alert">
          <p>{W.firstRun.checkFailed}</p>
          <button className="secondary-button" type="button" onClick={refresh}>{W.firstRun.retry}</button>
        </div>
      )}
      {Array.isArray(list) && <ProviderSignIn list={list} onChanged={refresh} />}
      <div className="first-run-actions">
        {onBack && <button className="secondary-button first-run-secondary" type="button" onClick={onBack}>{W.firstRun.back}</button>}
        <button className="confirm-button first-run-primary" type="button" disabled={!connected} onClick={onDone}>{W.firstRun.start}</button>
      </div>
    </div>
  )
}
