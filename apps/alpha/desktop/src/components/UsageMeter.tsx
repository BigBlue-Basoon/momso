import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  SLOW_PROVIDERS,
  getUsageStatus,
  loadLastUsage,
  onUsageUpdated,
  readUsageNow,
  remainingPercent,
  resetDate,
  saveLastUsage,
  scopeName,
  type ProviderUsage,
  type UsageProvider,
  type UsageWindow,
  usageReason,
} from '../usage'
import { ProviderMark, useProviderMarkHover, type ProviderMarkId } from './ProviderMark'
import { W } from '../words'
import { ModelName } from './ModelName'
import './UsageMeter.css'

const PROVIDERS: Array<{ id: UsageProvider; mark: ProviderMarkId; name: string }> = [
  { id: 'codex', mark: 'chatgpt', name: W.provider.codex },
  { id: 'claude', mark: 'claude', name: W.provider.claude },
  { id: 'gemini', mark: 'gemini', name: W.provider.gemini },
]

// 알림을 주지 않는 곳을 위해 가끔 다시 묻는 간격.
const REFRESH_MS = 5 * 60 * 1000

// 마우스를 올려 새로 읽은 뒤 이 시간 안에는 다시 읽지 않는다(방금 읽은 값을 그대로 쓴다).
const READ_COOLDOWN_MS = 60 * 1000
// 새로 읽기를 기다리는 가장 긴 시간. 넘으면 불러오는 안내를 거두고 가진 값으로 둔다.
const READ_TIMEOUT_MS = 20 * 1000
// 불러오는 안내를 보이는 가장 짧은 시간(너무 빨리 끝나 깜빡이지 않게).
const READ_MIN_MS = 600
// 값이 바뀔 때 고리와 숫자가 전 값에서 새 값으로 움직이는 시간.
const GLIDE_MS = 1200

// 띠의 Claude 로고(21px)는 첫 화면 폭(5.5%)으로는 빛살이 1px 도 안 움직여 보이지 않는다. 폭과 빠르기를 키운다.
const SMALL_CLAUDE_MOTION = { strength: 0.2, speed: 1.8 }

// 고리: 지름 34px(단추 38px 안), 선 2px. 로고 칸은 26px(띠의 몸소 표시와 같은 크기).
const RING_RADIUS = 16
const RING_LENGTH = 2 * Math.PI * RING_RADIUS

const WINDOW_LABEL: Record<string, string> = {
  // 클로드가 주는 갈래(백엔드가 원래 값을 그대로 줌): 지금 세션(5시간 창), 주간 전체, 주간 특정 모델.
  session: W.usageMeter.text1,
  weekly_all: W.usageMeter.text2,
  weekly_scoped: W.usageMeter.text2,
  '5h': W.usageMeter.text1,
  five_hour: W.usageMeter.text1,
  primary: W.usageMeter.text1,
  daily: W.usageMeter.text3,
  day: W.usageMeter.text3,
  weekly: W.usageMeter.text2,
  week: W.usageMeter.text2,
  '7d': W.usageMeter.text2,
  secondary: W.usageMeter.text2,
  monthly: W.usageMeter.text4,
  month: W.usageMeter.text4,
}

// 백엔드는 정해진 이름이 없는 기간을 '2_days', '3_hours', '15_minutes' 처럼 준다.
const UNIT_LABEL: Record<string, string> = { days: W.usageMeter.text5, hours: W.usageMeter.text6, minutes: W.usageMeter.text7 }

const timeFormat = new Intl.DateTimeFormat('ko-KR', { hour: 'numeric', minute: '2-digit' })
const dayTimeFormat = new Intl.DateTimeFormat('ko-KR', { month: 'long', day: 'numeric', hour: 'numeric', minute: '2-digit' })

function formatWhen(value: number | string | null) {
  const date = resetDate(value)
  if (!date) return null
  return date.toDateString() === new Date().toDateString() ? timeFormat.format(date) : dayTimeFormat.format(date)
}

function windowLabel(window: UsageWindow) {
  if (window.label?.trim()) return window.label.trim()
  const known = WINDOW_LABEL[window.kind]
  if (known) return known
  const match = /^(\d+)_(days|hours|minutes)$/.exec(window.kind)
  return match ? `${match[1]}${UNIT_LABEL[match[2]]}` : window.kind
}

/** 숫자가 있는 기간만. 숫자가 하나도 없으면 그 곳은 펼칠 것이 없다. */
function knownWindows(usage: ProviderUsage | undefined) {
  return usage?.state === 'ok'
    ? usage.windows.filter((window) => typeof window.usedPercent === 'number' && Number.isFinite(window.usedPercent))
    : []
}

/**
 * 맨 왼쪽 아이콘 띠 아래쪽의 사용량. AI 곳마다 단추 하나: 로고와 그 둘레의 고리(남은 양).
 * 누르면 그 곳의 것만 펼쳐진다(이름, 요금제, 기간별 남은 양, 다시 차는 때). 한 번에 하나만.
 * 숫자가 없는 곳은 로고만 보이고 눌리지 않는다(글로 까닭을 알리지 않는다, SCREEN_RULES 1절).
 */
export function UsageMeter() {
  const [usage, setUsage] = useState<ProviderUsage[] | null>(null)
  // 마지막으로 확인한 실제 값(이 기기에 적어 둔 것). 지금 읽지 못한 곳은 이것으로 그린다.
  const [last, setLast] = useState(loadLastUsage)
  // 마우스를 올려 새로 읽는 중인 곳과 그 시작 시각.
  const [reading, setReading] = useState<Partial<Record<UsageProvider, number>>>({})
  const lastReadAt = useRef<Partial<Record<UsageProvider, number>>>({})
  const [openId, setOpenId] = useState<UsageProvider | null>(null)
  const buttons = useRef<Partial<Record<UsageProvider, HTMLButtonElement | null>>>({})
  const panel = useRef<HTMLDivElement>(null)
  const [anchor, setAnchor] = useState({ left: 0, bottom: 0 })

  const refresh = useCallback(() => {
    getUsageStatus()
      .then(setUsage)
      .catch(() => setUsage(null))
  }, [])

  useEffect(() => {
    refresh()
    const timer = window.setInterval(refresh, REFRESH_MS)
    let stop: (() => void) | undefined
    let disposed = false
    void onUsageUpdated(refresh).then((unlisten) => {
      if (disposed) unlisten()
      else stop = unlisten
    }).catch(() => {})
    return () => {
      disposed = true
      window.clearInterval(timer)
      stop?.()
    }
  }, [refresh])

  // 새로 받은 실제 값(보기 숫자가 아닌 ok)은 마지막으로 확인한 값으로 적어 둔다.
  useEffect(() => {
    if (!usage) return
    const fresh = usage.filter((item) => item.state === 'ok')
    if (fresh.length === 0) return
    setLast((current) => {
      const next = { ...current }
      for (const item of fresh) next[item.provider] = item
      saveLastUsage(next)
      return next
    })
  }, [usage])

  // 곳마다 그릴 값: 지금 ok 면 지금 값, 연결하지 않았으면 그대로, 그 밖에는 마지막으로 확인한 값.
  const byProvider = new Map<UsageProvider, ProviderUsage>()
  for (const provider of PROVIDERS) {
    const fresh = usage?.find((item) => item.provider === provider.id)
    const shown = fresh?.state === 'ok' || fresh?.state === 'not_connected' ? fresh : (last[provider.id] ?? fresh)
    if (shown) byProvider.set(provider.id, shown)
  }

  // 새로 읽기가 끝났는지: 그 곳의 확인 시각이 읽기를 시작한 뒤로 바뀌면 끝.
  useEffect(() => {
    const done = (Object.keys(reading) as UsageProvider[]).filter((provider) => {
      const startedAt = reading[provider] ?? 0
      const fresh = usage?.find((item) => item.provider === provider)
      const checked = fresh?.checkedAt ? Date.parse(fresh.checkedAt) : 0
      return checked >= startedAt - 1000
    })
    if (done.length === 0) return
    const wait = Math.max(0, ...done.map((provider) => READ_MIN_MS - (Date.now() - (reading[provider] ?? 0))))
    const timer = window.setTimeout(() => {
      setReading((current) => {
        const next = { ...current }
        for (const provider of done) delete next[provider]
        return next
      })
    }, wait)
    return () => window.clearTimeout(timer)
  }, [usage, reading])

  // 너무 오래 걸리면 안내를 거둔다(값은 가진 것으로 둔다).
  useEffect(() => {
    if (Object.keys(reading).length === 0) return
    const timer = window.setTimeout(() => setReading({}), READ_TIMEOUT_MS)
    return () => window.clearTimeout(timer)
  }, [reading])

  // 마우스를 올리면(또는 초점이 가면) 읽는 데 시간이 걸리는 곳만 한 번 새로 읽는다.
  // 읽는 중이거나 방금(1분 안) 읽었으면 다시 읽지 않는다. 읽는 일은 눈에 보이지 않고, 안내만 잠깐 보인다.
  const readOnHover = (provider: UsageProvider) => {
    if (!SLOW_PROVIDERS.includes(provider)) return
    if (reading[provider]) return
    if (byProvider.get(provider)?.state === 'not_connected') return
    const now = Date.now()
    if (now - (lastReadAt.current[provider] ?? 0) < READ_COOLDOWN_MS) return
    lastReadAt.current[provider] = now
    setReading((current) => ({ ...current, [provider]: now }))
    readUsageNow(provider).then(setUsage).catch(() => {})
  }

  // 펼친 것은 그 단추 오른쪽에, 아래 끝을 단추 아래 끝에 맞춰 띄운다.
  useLayoutEffect(() => {
    const button = openId ? buttons.current[openId] : null
    if (!button) return
    const rect = button.getBoundingClientRect()
    setAnchor({ left: rect.right + 8, bottom: Math.max(8, window.innerHeight - rect.bottom) })
  }, [openId])

  // 숫자가 사라진 곳은 펼친 것을 닫는다.
  const openUsage = openId ? byProvider.get(openId) : undefined
  const openHasNumbers = knownWindows(openUsage).length > 0
  useEffect(() => {
    if (openId && usage && !openHasNumbers) setOpenId(null)
  }, [openId, usage, openHasNumbers])

  useEffect(() => {
    if (!openId) return
    const close = (event: MouseEvent) => {
      const target = event.target as Node
      if (panel.current?.contains(target)) return
      if (Object.values(buttons.current).some((button) => button?.contains(target))) return
      setOpenId(null)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      buttons.current[openId]?.focus()
      setOpenId(null)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [openId])

  const openProvider = PROVIDERS.find((provider) => provider.id === openId)

  return (
    <>
      <div className="usage-meter" role="group" aria-label={W.usageMeter.aria1}>
        {PROVIDERS.map((provider) => (
          <UsageButton
            key={provider.id}
            buttonRef={(element) => { buttons.current[provider.id] = element }}
            name={provider.name}
            mark={provider.mark}
            usage={byProvider.get(provider.id)}
            open={openId === provider.id}
            reading={Boolean(reading[provider.id])}
            onHover={() => readOnHover(provider.id)}
            onToggle={() => {
              if (openId !== provider.id) refresh()
              setOpenId((current) => (current === provider.id ? null : provider.id))
            }}
          />
        ))}
      </div>

      {openProvider && openUsage && openHasNumbers && createPortal(
        <div
          key={openProvider.id}
          ref={panel}
          className="usage-panel"
          role="dialog"
          aria-label={W.usageMeter.aria2(openProvider.name)}
          style={{ left: anchor.left, bottom: anchor.bottom }}
        >
          <UsageDetail name={openProvider.name} usage={openUsage} />
        </div>,
        document.body,
      )}
    </>
  )
}

type UsageButtonProps = {
  buttonRef: (element: HTMLButtonElement | null) => void
  name: string
  mark: ProviderMarkId
  usage: ProviderUsage | undefined
  open: boolean
  /** 새로 읽는 중이면 단추 옆에 불러오는 안내를 잠깐 보인다. */
  reading: boolean
  onHover: () => void
  onToggle: () => void
}

/** 값이 바뀌면 전 값에서 새 값까지 부드럽게 움직이는 숫자. 처음 값은 움직이지 않는다. */
function useGlide(target: number | null) {
  const [value, setValue] = useState(target)
  const current = useRef(target)
  useEffect(() => {
    const from = current.current
    const still = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    if (target === null || from === null || from === target || still) {
      current.current = target
      setValue(target)
      return
    }
    const startedAt = performance.now()
    let frame = 0
    const step = (now: number) => {
      const progress = Math.min(1, (now - startedAt) / GLIDE_MS)
      const eased = 1 - Math.pow(1 - progress, 3)
      const next = from + (target - from) * eased
      current.current = next
      setValue(next)
      if (progress < 1) frame = window.requestAnimationFrame(step)
    }
    frame = window.requestAnimationFrame(step)
    return () => window.cancelAnimationFrame(frame)
  }, [target])
  return value
}

/** 곳 하나의 단추. 로고는 띠 바탕 위에 바로 두고, 둘레의 고리는 남은 양만큼 밝게 칠한다. */
function UsageButton({ buttonRef, name, mark, usage, open, reading, onHover, onToggle }: UsageButtonProps) {
  const hover = useProviderMarkHover()
  const target = usage ? remainingPercent(usage) : null
  const left = useGlide(target)
  const hasNumbers = knownWindows(usage).length > 0
  // 숫자가 없으면 이름만, 뒤쪽이 읽을 만한 까닭을 주면 이름 뒤에 그 까닭(풍선과 화면 읽기 이름, 있던 자리 그대로).
  const reason = target === null ? usageReason(usage, name) : null
  const label = target !== null ? W.usageMeter.text8(name, Math.round(target)) : reason ? `${name}: ${reason}` : name
  const element = useRef<HTMLButtonElement | null>(null)
  const [noticeAt, setNoticeAt] = useState<{ left: number; top: number } | null>(null)

  // 불러오는 안내는 단추 오른쪽, 단추 가운데 높이에 띄운다. 끝나면 짧게 흐려진 뒤 사라진다.
  const [noticeLeaving, setNoticeLeaving] = useState(false)
  useLayoutEffect(() => {
    if (reading && element.current) {
      const rect = element.current.getBoundingClientRect()
      setNoticeLeaving(false)
      setNoticeAt({ left: rect.right + 8, top: rect.top + rect.height / 2 })
      return
    }
    if (!reading && noticeAt) {
      setNoticeLeaving(true)
      const timer = window.setTimeout(() => setNoticeAt(null), 180)
      return () => window.clearTimeout(timer)
    }
  }, [reading])

  return (
    <>
    <button
      ref={(node) => { element.current = node; buttonRef(node) }}
      className="usage-button provider-mark-trigger"
      type="button"
      title={label}
      aria-label={label}
      aria-haspopup={hasNumbers ? 'dialog' : undefined}
      aria-expanded={hasNumbers ? open : undefined}
      aria-disabled={!hasNumbers}
      data-open={open}
      aria-busy={reading}
      onClick={() => { if (hasNumbers) onToggle() }}
      onPointerEnter={() => { hover.handlers.onPointerEnter(); onHover() }}
      onPointerLeave={hover.handlers.onPointerLeave}
      onFocus={() => { hover.handlers.onFocus(); onHover() }}
      onBlur={hover.handlers.onBlur}
    >
      {left !== null && (
        <svg className="usage-ring" viewBox="0 0 38 38" aria-hidden="true">
          <circle className="usage-ring-track" cx="19" cy="19" r={RING_RADIUS} />
          <circle
            className="usage-ring-fill"
            cx="19"
            cy="19"
            r={RING_RADIUS}
            strokeDasharray={`${(RING_LENGTH * left) / 100} ${RING_LENGTH}`}
          />
        </svg>
      )}
      <ProviderMark provider={mark} active={hover.active} onDark motion={SMALL_CLAUDE_MOTION} className="usage-logo" />
    </button>
    {noticeAt && createPortal(
      <span className="usage-loading" data-leaving={noticeLeaving} role="status" style={{ left: noticeAt.left, top: noticeAt.top }}>
        <span className="usage-loading-text">{W.usageMeter.text9}</span>
      </span>,
      document.body,
    )}
    </>
  )
}

/** 펼친 것: 이름, 요금제, 기간마다 남은 양과 다시 차는 때, 확인한 때. */
function UsageDetail({ name, usage }: { name: string; usage: ProviderUsage }) {
  const checked = formatWhen(usage.checkedAt)
  return (
    <>
      <div className="usage-row-head">
        <strong>{name}</strong>
        {usage.planLabel && <span className="usage-row-plan">{usage.planLabel}</span>}
      </div>
      <div className="usage-windows">
        {knownWindows(usage).map((window, index, all) => {
          // 띠의 고리가 그리는 갈래(가장 적게 남은 것)를 펼친 것에서 알 수 있게 한다.
          const lowest = all.every((other) => (other.usedPercent ?? 0) <= (window.usedPercent ?? 0))
          return <UsageWindowLine key={`${window.kind}-${window.limitId ?? scopeName(window.scope) ?? ''}-${index}`} window={window} lowest={lowest && all.length > 1} />
        })}
      </div>
      {checked && <p className="usage-panel-checked">{W.usageMeter.text10(checked)}</p>}
    </>
  )
}

/** 펼친 것의 기간 한 줄. 값이 바뀌면 숫자와 막대가 같이 움직인다. */
function UsageWindowLine({ window, lowest }: { window: UsageWindow; lowest: boolean }) {
  const left = useGlide(Math.max(0, Math.min(100, 100 - (window.usedPercent ?? 0)))) ?? 0
  const resets = formatWhen(window.resetsAt)
  const scopeText = scopeName(window.scope)
  const scope = scopeText ? <ModelName label={scopeText} /> : null
  return (
    <span className={lowest ? 'usage-window is-lowest' : 'usage-window'} title={lowest ? W.usageMeter.tip1 : undefined}>
      <span className="usage-window-line">
        <span className="usage-window-name">
          {windowLabel(window)}
          {scope && <span className="usage-window-scope">{scope}</span>}
        </span>
        <span>{W.usageMeter.text11(Math.round(left))}</span>
      </span>
      <span className="usage-window-bar" aria-hidden="true">
        <span style={{ width: `${left}%` }} />
      </span>
      {resets && <span className="usage-window-reset">{W.usageMeter.text12(resets)}</span>}
    </span>
  )
}
