import {
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from 'react'
import { createPortal } from 'react-dom'
import { Check, ChevronDown, CircleAlert, Gauge, Hand, Mic, Paperclip, Send, ShieldCheck, Square, Wand2, X, Zap, type LucideIcon } from 'lucide-react'
import type { SkillEntry } from '../skills'
import {
  PROVIDER_NAME,
  PROVIDER_ORDER,
  effortOptions,
  modelLabelSource,
  modelName,
  resolveEffort,
  type AiProvider,
  type ChosenModel,
  type EffortOption,
  type PermissionModeOption,
  type ProviderModels,
  fastChoosable,
} from '../ai-models'
import type { ChatContext } from '../chats'
import { effortKorean, effortNoteKorean, isStrongestPermission, permissionKorean, sortPermissionModes } from '../permission-labels'
import { W, modelShortName } from '../words'
import { getVoiceTypingStatus, openVoiceTyping } from '../voice-typing'
import { ModelName } from './ModelName'
import { ProviderMark, useProviderMarkHover, type ProviderMarkId } from './ProviderMark'
import './Composer.css'

const MARK: Record<AiProvider, ProviderMarkId> = { codex: 'chatgpt', claude: 'claude', gemini: 'gemini' }

// 안내 글. 칸이 좁아 다 들어가지 않으면 앞부분만 보인다(글 칸의 안내 글은 말줄임표가 되지 않고 잘린다).
const PLACEHOLDER = W.composer.text1
const PLACEHOLDER_SHORT = W.composer.text2
const PLACEHOLDER_MIN_WIDTH = 340

// 입력 칸이 자라는 한계. Claude 앱처럼 한 줄로 시작해 적는 만큼 자라고, 이 높이를 넘으면 칸 안에서 내린다.
const TEXTAREA_MAX = 200

export type PermissionState = {
  /** 이 목록을 준 회사. 회사마다 단계가 다르다. */
  provider: AiProvider
  mode: string
  modes: PermissionModeOption[]
}

type ComposerProps = {
  prompt: string
  isGenerating: boolean
  onPromptChange: (prompt: string) => void
  onSend: () => void
  /** 모델 목록을 다 읽었는지. 아니면 보내기가 잠깐 눌리지 않는다. */
  sendReady?: boolean
  /** 켜진 스킬. null 이면 스킬 명령이 없어 / 를 쳐도 목록이 뜨지 않는다(글자 그대로 들어간다). */
  skills?: SkillEntry[] | null
  /** 이 말에 고른 스킬(하나만). 입력 칸 안에 조각으로 보인다. */
  skill?: SkillEntry | null
  onSkillChange?: (skill: SkillEntry | null) => void
  onStop: () => void
  onAttach: () => void
  /** 모델 목록. null 이면 읽지 못했다(이때는 설정의 AI 곳만 고를 수 있다). */
  models: ProviderModels[] | null
  /** 모델 목록을 읽지 못했을 때 고를 수 있는 곳(설정의 AI 연결). */
  fallbackProviders: AiProvider[]
  /** 지금 보낼 수 있는 곳. 여기 없는 곳의 모델은 눌리지 않는다. */
  sendableProviders: AiProvider[]
  chosen: ChosenModel
  onChooseProvider: (provider: AiProvider) => void
  onChooseModel: (modelId: string) => void
  onChooseEffort: (effort: string) => void
  onToggleFast: (on: boolean) => void
  /** 고른 모델로 고속 모드를 보내 봤는데 쓸 수 없다고 온 적이 있는지(그 뒤로 눌리지 않는다). */
  fastRefused?: boolean
  /** 권한 단계. 읽지 못했으면 null 이고 칸 아래에 보이지 않는다. */
  permission: PermissionState | null
  onPermissionChange: (mode: string) => void
  /** 대화가 AI 가 볼 수 있는 양을 얼마나 채웠는지. 숫자가 올 때만 보인다. */
  context: ChatContext | null
  /** 마이크(Windows 받아쓰기)를 열지 못했을 때 짧게 알릴 곳. */
  onVoiceProblem?: (message: string) => void
}

/**
 * 가운데 대화 칸 아래의 입력 칸. 글은 한 줄 높이로 시작해 적는 만큼 자란다.
 * 칸 안 윗줄: (고른 스킬 조각) 글. 칸 안 아랫줄(Aside 앱처럼): 왼쪽에 첨부와 권한 단계, 오른쪽에 회사·모델·추론 수준·고속 모드,
 * 대화가 찬 정도(숫자가 있을 때만), 마이크, 보내기(답을 쓰는 중이면 멈추기).
 */
export function Composer({
  prompt,
  isGenerating,
  onPromptChange,
  onSend,
  sendReady = true,
  skills = null,
  skill = null,
  onSkillChange,
  onStop,
  onAttach,
  models,
  fallbackProviders,
  sendableProviders,
  chosen,
  onChooseProvider,
  onChooseModel,
  onChooseEffort,
  onToggleFast,
  fastRefused = false,
  permission,
  onPermissionChange,
  context,
  onVoiceProblem,
}: ComposerProps) {
  const textarea = useRef<HTMLTextAreaElement>(null)
  // 마이크: 받아쓰기 명령이 있고 unsupported 가 아니면 누를 수 있다. 명령이 없거나 unsupported 면 단추만 눌리지 않는다.
  const [voiceUsable, setVoiceUsable] = useState(false)
  useEffect(() => {
    let alive = true
    getVoiceTypingStatus().then((status) => { if (alive) setVoiceUsable(status.state !== 'unsupported') }).catch(() => { if (alive) setVoiceUsable(false) })
    return () => { alive = false }
  }, [])
  async function startVoice() {
    // 받아쓴 글은 Windows 가 지금 글자 칸에 넣으므로 입력 칸에 커서를 먼저 둔다.
    textarea.current?.focus()
    try {
      const state = await openVoiceTyping()
      if (state === 'unsupported') { setVoiceUsable(false); return }
      const problem = W.composer.voiceProblem[state]
      if (problem) onVoiceProblem?.(problem)
    } catch {
      onVoiceProblem?.(W.composer.voiceProblem.input_rejected)
    }
  }
  // / 로 스킬 고르기: 맨 앞이거나 빈칸 뒤의 / 부터 글자 칸 커서까지가 찾는 말.
  const [slash, setSlash] = useState<{ query: string; start: number } | null>(null)
  const [slashIndex, setSlashIndex] = useState(0)
  const caretAfterPick = useRef<number | null>(null)
  const slashMatches = slash && skills
    ? skills.filter((item) => item.enabled && (item.title.includes(slash.query) || item.name.includes(slash.query.toLowerCase())))
    : []
  const slashOpen = slashMatches.length > 0
  const canSend = Boolean(prompt.trim() || skill)

  function readSlash(value: string, caret: number) {
    if (!skills) { setSlash(null); return }
    const match = /(^|\s)\/([^\s/]*)$/.exec(value.slice(0, caret))
    if (!match) { setSlash(null); return }
    setSlash({ query: match[2], start: caret - match[2].length - 1 })
    setSlashIndex(0)
  }

  function pickSkill(item: SkillEntry) {
    if (!slash) return
    const caret = textarea.current?.selectionStart ?? prompt.length
    const next = prompt.slice(0, slash.start) + prompt.slice(caret)
    caretAfterPick.current = slash.start
    onPromptChange(next)
    onSkillChange?.(item)
    setSlash(null)
    textarea.current?.focus()
  }

  useLayoutEffect(() => {
    if (caretAfterPick.current === null || !textarea.current) return
    textarea.current.setSelectionRange(caretAfterPick.current, caretAfterPick.current)
    caretAfterPick.current = null
  })
  const [narrow, setNarrow] = useState(false)

  useEffect(() => {
    const element = textarea.current
    if (!element) return
    const observer = new ResizeObserver(() => setNarrow(element.clientWidth < PLACEHOLDER_MIN_WIDTH))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])

  // 적은 만큼 자란다.
  useLayoutEffect(() => {
    const element = textarea.current
    if (!element) return
    element.style.height = 'auto'
    const next = Math.min(element.scrollHeight, TEXTAREA_MAX)
    element.style.height = `${next}px`
    element.style.overflowY = element.scrollHeight > TEXTAREA_MAX ? 'auto' : 'hidden'
  }, [prompt])

  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (slashOpen && !event.nativeEvent.isComposing) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        const step = event.key === 'ArrowDown' ? 1 : -1
        setSlashIndex((current) => (current + step + slashMatches.length) % slashMatches.length)
        return
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault()
        pickSkill(slashMatches[Math.min(slashIndex, slashMatches.length - 1)])
        return
      }
      if (event.key === 'Escape') {
        event.preventDefault()
        event.stopPropagation()
        setSlash(null)
        return
      }
    }
    // 글자 칸 맨 앞에서 지우면 스킬 조각이 통째로 지워진다.
    if (event.key === 'Backspace' && skill && event.currentTarget.selectionStart === 0 && event.currentTarget.selectionEnd === 0) {
      event.preventDefault()
      onSkillChange?.(null)
      return
    }
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault()
      if (canSend && !isGenerating && sendReady) onSend()
    }
  }

  return (
    <div className="composer-dock">
      {slashOpen && (
        <div className="slash-menu" role="listbox" aria-label={W.skills.pickerLabel}>
          {slashMatches.map((item, index) => (
            <button
              key={item.name}
              type="button"
              role="option"
              aria-selected={index === slashIndex}
              className={index === slashIndex ? 'slash-item is-active' : 'slash-item'}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setSlashIndex(index)}
              onClick={() => pickSkill(item)}
            >
              <Wand2 size={14} aria-hidden />
              <span className="slash-item-text">
                <span className="slash-item-title">{item.title}</span>
                {item.description && <span className="slash-item-description">{item.description}</span>}
              </span>
              <span className="slash-item-name">/{item.name}</span>
            </button>
          ))}
        </div>
      )}
      {/* Aside 앱처럼(2026-11-16 그의 말 "우리도 저렇게 하자"): 칸 안 윗줄에 글, 칸 안 아랫줄에 단추들.
          아랫줄 왼쪽: 첨부, 권한 단계. 오른쪽: 회사, 모델, 추론 수준, 고속 모드, 채팅방이 찬 정도(고리), 마이크, 보내기. */}
      <div className="prompt-composer composer-boxed">
        <div className="composer-input-row">
          {skill && (
            <span className="composer-skill" title={skill.description || undefined}>
              <Wand2 size={13} aria-hidden />
              <span className="composer-skill-title">{skill.title}</span>
              <button type="button" aria-label={W.skills.removeChip(skill.title)} onClick={() => { onSkillChange?.(null); textarea.current?.focus() }}>
                <X size={12} aria-hidden />
              </button>
            </span>
          )}
          <textarea
            ref={textarea}
            value={prompt}
            rows={1}
            onChange={(event) => { onPromptChange(event.target.value); readSlash(event.target.value, event.target.selectionStart ?? event.target.value.length) }}
            onBlur={() => setSlash(null)}
            onKeyDown={handleKeyDown}
            placeholder={narrow ? PLACEHOLDER_SHORT : PLACEHOLDER}
            aria-label={W.composer.aria2}
          />
        </div>
        <div className="composer-toolbar">
          <div className="composer-toolbar-start">
            <button
              className="composer-icon"
              type="button"
              aria-label={W.composer.aria1}
              title={W.composer.aria1}
              onClick={onAttach}
            >
              <Paperclip size={16} aria-hidden />
            </button>
            {permission && <PermissionPicker permission={permission} onChange={onPermissionChange} />}
          </div>
          <div className="composer-toolbar-end">
            <CompanyPicker models={models} fallbackProviders={fallbackProviders} sendableProviders={sendableProviders} chosen={chosen} onChoose={onChooseProvider} />
            <FamilyPicker models={models} chosen={chosen} onChoose={onChooseModel} />
            <EffortPicker models={models} chosen={chosen} onChoose={onChooseEffort} />
            <FastToggle models={models} chosen={chosen} refused={fastRefused} onToggle={onToggleFast} />
            {context?.state === 'ok' && <ContextMeter context={context} />}
            {/* 보내기 단추 바로 왼쪽에 마이크. 누르는 동안 입력 칸의 커서를 빼앗지 않는다(mousedown 을 막음). */}
            <button
              className="composer-icon composer-mic"
              type="button"
              aria-label={W.composer.voice}
              title={W.composer.voice}
              disabled={!voiceUsable}
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => void startVoice()}
            >
              <Mic size={16} aria-hidden />
            </button>
            <button
              className="send-button"
              type="button"
              aria-label={isGenerating ? W.composer.aria3 : W.composer.aria4}
              title={isGenerating ? W.composer.aria3 : W.composer.aria4}
              disabled={!isGenerating && (!canSend || !sendReady)}
              onClick={isGenerating ? onStop : onSend}
            >
              {isGenerating ? <Square size={12} fill="currentColor" aria-hidden /> : <Send size={15} aria-hidden />}
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

/**
 * 칸 안 아랫줄의 단추가 여는 작은 창. 단추 위에 띄운다. 오른쪽 무리의 단추는 창의 오른쪽 끝을 단추 오른쪽 끝에,
 * 왼쪽 무리의 단추(권한 단계)는 창의 왼쪽 끝을 단추 왼쪽 끝에 맞춘다(좁은 창에서 왼쪽으로 잘리지 않게).
 */
function usePopover(align: 'end' | 'start' = 'end') {
  const [open, setOpen] = useState(false)
  const button = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const [anchor, setAnchor] = useState<{ right?: number; left?: number; bottom: number }>({ right: 0, bottom: 0 })

  useLayoutEffect(() => {
    if (!open || !button.current) return
    const rect = button.current.getBoundingClientRect()
    const bottom = window.innerHeight - rect.top + 6
    setAnchor(align === 'start'
      ? { left: Math.max(8, rect.left), bottom }
      : { right: Math.max(8, window.innerWidth - rect.right), bottom })
  }, [open, align])

  useEffect(() => {
    if (!open) return
    const close = (event: MouseEvent) => {
      const target = event.target as Node
      if (panel.current?.contains(target) || button.current?.contains(target)) return
      setOpen(false)
    }
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation()
        setOpen(false)
        button.current?.focus()
      }
    }
    const resize = () => setOpen(false)
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape, true)
    window.addEventListener('resize', resize)
    // 열리면 고른 항목(없으면 첫 항목)에 초점을 둔다.
    window.requestAnimationFrame(() => {
      const slider = panel.current?.querySelector<HTMLElement>('[role="slider"]')
      if (slider) { slider.focus(); return }
      const items = panel.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)')
      const checked = panel.current?.querySelector<HTMLButtonElement>('[role="menuitemradio"][aria-checked="true"]:not(:disabled)')
      ;(checked ?? items?.[0])?.focus()
    })
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape, true)
      window.removeEventListener('resize', resize)
    }
  }, [open])

  return { open, setOpen, button, panel, anchor }
}

/**
 * 다른 곳(채팅의 알림 줄 "다른 AI로 바꾸기"·"모델 바꾸기")에서 이 고르는 창을 열어 달라고 할 때(momso:open-picker 이벤트, detail 'company'|'model').
 */
function useOpenFromOutside(which: 'company' | 'model', popover: ReturnType<typeof usePopover>) {
  const { setOpen, button } = popover
  useEffect(() => {
    const open = (event: Event) => {
      if ((event as CustomEvent<string>).detail !== which) return
      button.current?.focus()
      setOpen(true)
    }
    window.addEventListener('momso:open-picker', open)
    return () => window.removeEventListener('momso:open-picker', open)
  }, [which, setOpen, button])
}

/** 위아래 화살표로 항목 사이를 옮긴다. */
function moveFocus(event: KeyboardEvent<HTMLDivElement>) {
  if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp' && event.key !== 'Home' && event.key !== 'End') return
  const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)'))
  if (items.length === 0) return
  event.preventDefault()
  const at = items.indexOf(document.activeElement as HTMLButtonElement)
  const next = event.key === 'Home' ? 0
    : event.key === 'End' ? items.length - 1
      : event.key === 'ArrowDown' ? (at + 1) % items.length
        : (at - 1 + items.length) % items.length
  items[next].focus()
}

function Popover({
  popover,
  label,
  className,
  children,
  role = 'menu',
}: {
  popover: ReturnType<typeof usePopover>
  label: string
  className: string
  children: ReactNode
  role?: 'menu' | 'dialog'
}) {
  if (!popover.open) return null
  return createPortal(
    <div
      ref={popover.panel as RefObject<HTMLDivElement>}
      className={`composer-menu ${className}`}
      role={role}
      aria-label={label}
      style={{ right: popover.anchor.right, left: popover.anchor.left, bottom: popover.anchor.bottom }}
      onKeyDown={moveFocus}
    >
      {children}
    </div>,
    document.body,
  )
}

/** 지금 고른 모델(목록에서). 목록을 읽지 못했으면 undefined. */
function findChosenModel(models: ProviderModels[] | null, chosen: ChosenModel) {
  const list = models?.find((item) => item.provider === chosen.provider)
  if (!list || list.state !== 'ok') return undefined
  return chosen.model ? list.models.find((model) => model.id === chosen.model) : undefined
}

/**
 * 빠른 모드: 고른 모델이 받을 때만 그린다. 켜짐은 번개가 채워지고 글이 짙어지며, 꺼짐은 흐리다.
 * 잠시 못 씀(쿨다운)과 쓸 수 없음은 눌리지 않는다(까닭 글 없음). 이름은 백엔드가 준 그 회사의 이름.
 * 상태가 확인되지 않아도(unavailable) 고를 수 있고, 보내 보고 안 된 모델은 그 뒤로 눌리지 않는다(fastChoosable).
 */
function FastToggle({ models, chosen, refused, onToggle }: { models: ProviderModels[] | null; chosen: ChosenModel; refused: boolean; onToggle: (on: boolean) => void }) {
  const current = findChosenModel(models, chosen)
  const fast = current?.fast
  if (!current || !fast?.supported) return null
  const usable = fastChoosable(chosen.provider, current, refused)
  const on = usable && (chosen.fast ?? fast.state === 'on')
  const label = W.fastName[chosen.provider] ?? (fast.label?.trim() || null)
  return (
    <button
      className={`composer-chip fast-chip${on ? ' is-on' : ''}`}
      type="button"
      aria-pressed={on}
      aria-label={label ?? 'fast'}
      aria-disabled={!usable || undefined}
      data-state={fast.state}
      title={label ?? undefined}
      onClick={() => { if (usable) onToggle(!on) }}
    >
      <Zap size={14} fill={on ? 'currentColor' : 'none'} aria-hidden />
      {label && <ChipText text={label} />}
    </button>
  )
}

/** 단추 안의 글. 값이 바뀌면 새 글이 부드럽게 나타난다. */
function ChipText({ text, className = 'composer-chip-label' }: { text: string; className?: string }) {
  return <span key={text} className={`${className} composer-chip-swap`}>{text}</span>
}

/** 회사 고르기: 세 회사만. 고르면 그 회사의 기본 모델과 기본 추론 수준, 그 회사에서 기억해 둔 권한 단계로 바뀐다. */
function CompanyPicker({
  models,
  fallbackProviders,
  sendableProviders,
  chosen,
  onChoose,
}: {
  models: ProviderModels[] | null
  fallbackProviders: AiProvider[]
  sendableProviders: AiProvider[]
  chosen: ChosenModel
  onChoose: (provider: AiProvider) => void
}) {
  const popover = usePopover()
  const hover = useProviderMarkHover()
  useOpenFromOutside('company', popover)
  // 모델 목록을 읽었으면 목록이 있는 회사만, 못 읽었으면 설정의 AI 곳만(까닭 글 없음, SCREEN_RULES 1절).
  const providers = models
    ? PROVIDER_ORDER.filter((provider) => models.some((list) => list.provider === provider && list.state === 'ok' && list.models.length > 0))
    : fallbackProviders
  return (
    <>
      <button
        ref={popover.button}
        className="composer-chip company-chip provider-mark-trigger"
        type="button"
        aria-haspopup="menu"
        aria-expanded={popover.open}
        aria-label={`AI: ${PROVIDER_NAME[chosen.provider]}`}
        title={W.composer.tip1}
        onClick={() => popover.setOpen((current) => !current)}
        {...hover.handlers}
      >
        <ProviderMark provider={MARK[chosen.provider]} active={hover.active} className="composer-chip-mark" />
        <ChipText text={PROVIDER_NAME[chosen.provider]} />
        <ChevronDown size={13} aria-hidden />
      </button>
      <Popover popover={popover} label="AI" className="model-menu">
        {providers.map((provider) => {
          const checked = chosen.provider === provider
          return (
            <button
              key={provider}
              className="composer-menu-item composer-menu-provider"
              type="button"
              role="menuitemradio"
              aria-checked={checked}
              disabled={!sendableProviders.includes(provider)}
              onClick={() => {
                onChoose(provider)
                popover.setOpen(false)
                popover.button.current?.focus()
              }}
            >
              <ProviderHeading provider={provider} inline />
              {checked && <Check size={14} aria-hidden />}
            </button>
          )
        })}
      </Popover>
    </>
  )
}

/** 모델 고르기: 지금 고른 회사의 모델 계열만(백엔드가 준 이름 그대로). 목록이 없으면 그리지 않는다. */
function FamilyPicker({ models, chosen, onChoose }: { models: ProviderModels[] | null; chosen: ChosenModel; onChoose: (modelId: string) => void }) {
  const popover = usePopover()
  useOpenFromOutside('model', popover)
  const list = models?.find((item) => item.provider === chosen.provider)
  const current = findChosenModel(models, chosen)
  if (!list || list.state !== 'ok' || list.models.length === 0 || !current) return null
  return (
    <>
      <button
        ref={popover.button}
        className="composer-chip"
        type="button"
        aria-haspopup="menu"
        aria-expanded={popover.open}
        aria-label={W.composer.aria5(modelName(current))}
        title={modelName(current)}
        onClick={() => popover.setOpen((open) => !open)}
      >
        {/* 단추에는 이름만, 올리면 "솔 · GPT-6.1"(사진 495 의 "나"). */}
        <ChipText text={modelShortName(modelLabelSource(current))} />
        <ChevronDown size={13} aria-hidden />
      </button>
      <Popover popover={popover} label={W.composer.label1} className="model-menu">
        {list.models.map((model) => {
          const checked = model.id === current.id
          return (
            <button
              key={model.id}
              className="composer-menu-item"
              type="button"
              role="menuitemradio"
              aria-checked={checked}
              onClick={() => {
                onChoose(model.id)
                popover.setOpen(false)
                popover.button.current?.focus()
              }}
            >
              <ModelMenuName model={model} />
              {model.description && <span className="composer-menu-note model-menu-note">{model.description}</span>}
              <span className="model-menu-check">{checked && <Check size={14} aria-hidden />}</span>
            </button>
          )
        })}
      </Popover>
    </>
  )
}

/** 목록 안의 모델 이름(사진 495 의 "나"): 줄마다 이름이 왼쪽에서 시작하고 판 번호는 바로 뒤에 작고 흐리게. */
function ModelMenuName({ model }: { model: Parameters<typeof modelName>[0] }) {
  return <ModelName label={modelLabelSource(model)} className="model-menu-name" />
}

/**
 * 추론 수준 고르기(김성균 개발자의 안: Codex 앱처럼 슬라이더). 단추를 누르면 작은 창이 뜨고, 위에 지금 수준의 이름이 크게,
 * 아래에 가로 슬라이더(수준마다 점, 지금 수준까지 채움)가 있다. 구간의 수는 그 모델이 받는 수준의 수와 같다.
 * 끌어서, 점을 눌러서, 좌우 화살표 키로 바꾼다. 쓸 수 없는 수준은 흐리고 넘어가지 않는다. 그 회사의 설명이 있으면 한 줄.
 * 켜고 끄는 특별한 항목(kind: 'toggle')은 슬라이더에 넣지 않고 아래 스위치 자리에 둔다(보낼 계약이 없어 지금은 눌리지 않는다).
 * 수준이 하나뿐이거나 없으면 단추를 그리지 않는다.
 */
function EffortPicker({ models, chosen, onChoose }: { models: ProviderModels[] | null; chosen: ChosenModel; onChoose: (effort: string) => void }) {
  const popover = usePopover()
  const current = findChosenModel(models, chosen)
  const options = effortOptions(current)
  const levels = options.filter((option) => option.kind !== 'toggle')
  const toggles = options.filter((option) => option.kind === 'toggle')
  if (!current || levels.length < 2) return null
  // 보내는 값과 같은 것을 보인다. 고르지 않았으면 실제로 쓰이는 수준(effectiveDefaultEffort)이 단추와 슬라이더에 나온다
  // (2026-11-16 그의 말 "고르지 않더라도 실제로 뭐가 선택되어있는거 아니야? … 보여야지"). 값이 없으면 지어내지 않는다.
  const effort = resolveEffort(current, chosen.effort)
  const text = (option: EffortOption) => {
    const korean = effortKorean(chosen.provider, option.id, option.label)
    return korean?.label ?? option.label
  }
  const currentOption = levels.find((option) => option.id === effort)
  const currentLabel = currentOption ? text(currentOption) : null
  const note = effortNoteKorean(chosen.provider) ?? current.effortNote ?? null
  return (
    <>
      <button
        ref={popover.button}
        className="composer-chip"
        type="button"
        aria-haspopup="dialog"
        aria-expanded={popover.open}
        aria-label={currentLabel ? W.composer.aria6(currentLabel) : W.composer.aria7}
        title={W.composer.tip3}
        onClick={() => popover.setOpen((open) => !open)}
      >
        {currentLabel ? <ChipText text={currentLabel} /> : <Gauge size={14} aria-hidden />}
        <ChevronDown size={13} aria-hidden />
      </button>
      <Popover popover={popover} label={W.composer.aria7} className="effort-menu effort-slider-menu" role="dialog">
        <EffortSlider levels={levels} value={currentOption?.id ?? null} labelOf={text} onChange={onChoose} />
        {note && <p className="effort-slider-note">{note}</p>}
        {toggles.length > 0 && (
          <div className="effort-toggles" role="group" aria-label={W.composer.aria8}>
            {toggles.map((option) => (
              <div key={option.id} className="effort-toggle" aria-disabled="true">
                <span className="effort-toggle-label">{text(option)}</span>
                <span className="effort-switch" role="switch" aria-checked="false" aria-disabled="true" aria-label={text(option)} />
              </div>
            ))}
          </div>
        )}
      </Popover>
    </>
  )
}

// 길의 높이(28px)의 반. 채움은 늘 손잡이(24px)를 둘레 2px 로 감싼다.
const SLIDER_RADIUS = 14

/** 추론 수준 슬라이더. value 가 없으면(회사 기본값도 없음) 손잡이 없이 점만 보이고, 점을 누르면 고른다. */
function EffortSlider({ levels, value, labelOf, onChange }: {
  levels: EffortOption[]
  value: string | null
  labelOf: (option: EffortOption) => string
  onChange: (id: string) => void
}) {
  const track = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)
  const count = levels.length
  const index = levels.findIndex((option) => option.id === value)
  const usable = levels.map((option, at) => (option.available ? at : -1)).filter((at) => at >= 0)
  const lastUsable = usable.length > 0 ? usable[usable.length - 1] : -1
  const nearestUsable = (at: number) => usable.reduce((best, candidate) => (Math.abs(candidate - at) < Math.abs(best - at) ? candidate : best), usable[0] ?? at)
  const choose = (at: number) => {
    if (usable.length === 0) return
    const next = nearestUsable(Math.max(0, Math.min(count - 1, at)))
    if (levels[next] && levels[next].id !== value) onChange(levels[next].id)
  }
  const fromPointer = (clientX: number) => {
    const rect = track.current?.getBoundingClientRect()
    if (!rect) return
    const ratio = (clientX - rect.left - SLIDER_RADIUS) / Math.max(1, rect.width - SLIDER_RADIUS * 2)
    choose(Math.round(ratio * (count - 1)))
  }
  const step = (direction: 1 | -1) => {
    const from = index < 0 ? (direction > 0 ? -1 : count) : index
    const next = direction > 0 ? usable.find((at) => at > from) : [...usable].reverse().find((at) => at < from)
    if (next !== undefined) choose(next)
  }
  const at = (i: number) => `calc((100% - ${SLIDER_RADIUS * 2}px) * ${count > 1 ? i / (count - 1) : 0})`
  const option = index >= 0 ? levels[index] : null
  const name = option ? labelOf(option) : W.composer.aria7
  return (
    <div className="effort-slider">
      <div className="effort-slider-head" aria-live="polite">
        <span key={name} className="effort-slider-name">{name}</span>
        {option?.recommended && <span className="effort-slider-tag">{W.composer.recommended}</span>}
      </div>
      <div
        ref={track}
        className="effort-track"
        onPointerDown={(event) => {
          dragging.current = true
          try { event.currentTarget.setPointerCapture(event.pointerId) } catch { /* 잡을 수 없는 포인터면 끌기 없이 누른 자리만 */ }
          fromPointer(event.clientX)
          event.currentTarget.querySelector<HTMLElement>('[role="slider"]')?.focus()
        }}
        onPointerMove={(event) => { if (dragging.current) fromPointer(event.clientX) }}
        onPointerUp={(event) => { dragging.current = false; try { event.currentTarget.releasePointerCapture(event.pointerId) } catch { /* 잡지 않았던 포인터 */ } }}
        onPointerCancel={() => { dragging.current = false }}
      >
        {lastUsable < count - 1 && (
          <span className="effort-track-off" style={{ left: `calc(${at(lastUsable + 0.5)} + ${SLIDER_RADIUS}px)` }} aria-hidden />
        )}
        {index >= 0 && <span className="effort-track-fill" style={{ width: `calc(${at(index)} + ${SLIDER_RADIUS * 2}px)` }} aria-hidden />}
        {levels.map((level, i) => (
          <span
            key={level.id}
            className={`effort-track-dot${i <= index ? ' is-covered' : ''}${level.available ? '' : ' is-off'}${level.recommended ? ' is-recommended' : ''}`}
            style={{ left: `calc(${at(i)} + ${SLIDER_RADIUS}px)` }}
            title={level.recommended ? `${labelOf(level)} · ${W.composer.recommended}` : labelOf(level)}
            aria-hidden
          />
        ))}
        <span
          className={`effort-thumb${index < 0 ? ' is-empty' : ''}`}
          style={{ left: `calc(${at(Math.max(0, index))} + 2px)` }}
          role="slider"
          tabIndex={0}
          aria-label={W.composer.effortSlider}
          aria-valuemin={1}
          aria-valuemax={count}
          aria-valuenow={index >= 0 ? index + 1 : undefined}
          aria-valuetext={option ? labelOf(option) : undefined}
          onKeyDown={(event) => {
            if (event.key === 'ArrowRight' || event.key === 'ArrowUp') { event.preventDefault(); event.stopPropagation(); step(1) }
            else if (event.key === 'ArrowLeft' || event.key === 'ArrowDown') { event.preventDefault(); event.stopPropagation(); step(-1) }
            else if (event.key === 'Home') { event.preventDefault(); event.stopPropagation(); if (usable.length) choose(usable[0]) }
            else if (event.key === 'End') { event.preventDefault(); event.stopPropagation(); if (usable.length) choose(lastUsable) }
          }}
        />
      </div>
    </div>
  )
}

function ProviderHeading({ provider, inline = false }: { provider: AiProvider; inline?: boolean }) {
  return (
    <span className={inline ? 'model-menu-heading is-inline' : 'model-menu-heading'}>
      <ProviderMark provider={MARK[provider]} className="model-menu-mark" />
      {PROVIDER_NAME[provider]}
    </span>
  )
}

// 코덱스 권한 단계의 작은 그림(Codex 앱 메뉴처럼). 다른 회사는 그 앱에 그림이 없어 그리지 않는다.
const PERMISSION_ICON: Record<string, LucideIcon> = {
  'codex:Ask for approval': Hand,
  'codex:Approve for me': ShieldCheck,
  'codex:Full access': CircleAlert,
}

/** 화면에 보이는 단계 이름과 설명: 화면용 한국어 표에 있으면 그것, 없으면 회사의 원문 그대로. */
function permissionText(provider: AiProvider, option: PermissionModeOption) {
  const korean = permissionKorean(provider, option.id)
  return { label: korean?.label ?? option.label, description: korean ? korean.description ?? null : option.description ?? null }
}

/**
 * 권한 단계 고르기. 지금 고른 회사의 단계만 보인다(회사마다 다르다). 틀은 세 회사가 같다(Claude 앱의 고르기처럼
 * 이름과 그 아래 한 줄 설명, 고른 것에 체크). 쓸 수 없는 단계는 흐리고 눌리지 않는다(까닭 글 없음).
 */
function PermissionPicker({ permission, onChange }: { permission: PermissionState; onChange: (mode: string) => void }) {
  const popover = usePopover('start')
  const current = permission.modes.find((item) => item.id === permission.mode)
  const currentLabel = current ? permissionText(permission.provider, current).label : null
  if (!currentLabel || !current) return null
  const strongest = isStrongestPermission(permission.provider, current.id)
  return (
    <>
      <button
        ref={popover.button}
        className={`composer-chip permission-chip${strongest ? ' is-strongest' : ''}`}
        type="button"
        aria-haspopup="menu"
        aria-expanded={popover.open}
        aria-label={W.composer.aria9(currentLabel)}
        title={W.composer.tip4}
        onClick={() => popover.setOpen((open) => !open)}
      >
        {strongest && <CircleAlert size={13} aria-hidden />}
        <span className="composer-chip-label">{currentLabel}</span>
        <ChevronDown size={13} aria-hidden />
      </button>
      <Popover popover={popover} label={W.composer.tip4} className="permission-menu">
        {sortPermissionModes(permission.provider, permission.modes).map((option) => {
          const checked = permission.mode === option.id
          const text = permissionText(permission.provider, option)
          const Icon = PERMISSION_ICON[`${permission.provider}:${option.id}`]
          return (
            <button
              key={option.id}
              className={`composer-menu-item${isStrongestPermission(permission.provider, option.id) ? ' is-strongest' : ''}`}
              type="button"
              role="menuitemradio"
              aria-checked={checked}
              disabled={!option.available && !checked}
              aria-disabled={!option.available}
              onClick={() => {
                if (!option.available) return
                onChange(option.id)
                popover.setOpen(false)
                popover.button.current?.focus()
              }}
            >
              {Icon && <Icon className="permission-menu-icon" size={15} aria-hidden />}
              <span className="composer-menu-text">
                <span className="composer-menu-name">{text.label}</span>
                {text.description && <span className="composer-menu-note">{text.description}</span>}
                {option.warning && <span className="composer-menu-warning">{option.warning}</span>}
              </span>
              {checked && <Check size={14} aria-hidden />}
            </button>
          )
        })}
      </Popover>
    </>
  )
}

const numberFormat = new Intl.NumberFormat('ko-KR')
const METER_RADIUS = 6.5
const METER_LENGTH = 2 * Math.PI * METER_RADIUS

/**
 * 채팅방이 AI 가 한 번에 볼 수 있는 양을 얼마나 채웠는지. 칸 안 아랫줄, 마이크 왼쪽의 작은 고리 하나.
 * Aside 앱처럼 올리거나(키보드로 가면) 고리 위에 작은 카드가 뜬다: 얼마나 찼는지, 쓴 양 / 한계(숫자가 올 때만).
 * 값은 세 회사 공통 값(chat_context·chat-context-updated 의 usedTokens, limitTokens, usedPercent)만 쓴다. 찬 정도를 알 수 없으면
 * (percent 도 한계도 없음) 고리를 그리지 않는다(숫자를 지어내지 않음). 줄여 담기 시작하는 때는 뒤쪽이 값으로 주지 않아 적지 않는다.
 * Aside 는 같은 카드에 요금제 남은 양도 두지만,
 * 몸소는 그것을 따로 보이므로(사용량) 여기에 다시 두지 않는다.
 */
function ContextMeter({ context }: { context: Extract<ChatContext, { state: 'ok' }> }) {
  const anchor = useRef<HTMLSpanElement>(null)
  const timer = useRef<number | null>(null)
  const [open, setOpen] = useState(false)
  const [place, setPlace] = useState<{ right: number; bottom: number } | null>(null)
  useEffect(() => () => { if (timer.current) window.clearTimeout(timer.current) }, [])
  useLayoutEffect(() => {
    if (!open || !anchor.current) return
    const rect = anchor.current.getBoundingClientRect()
    setPlace({ right: Math.max(8, window.innerWidth - rect.right), bottom: window.innerHeight - rect.top + 6 })
  }, [open])
  const limit = context.limitTokens ?? 0
  const known = typeof context.usedPercent === 'number' || limit > 0
  const percent = Math.max(0, Math.min(100, context.usedPercent ?? (limit > 0 ? (context.usedTokens / limit) * 100 : 0)))
  if (!known) return null
  const lines = [W.composer.text3(Math.round(percent))]
  const amount = limit > 0 ? `${numberFormat.format(context.usedTokens)} / ${numberFormat.format(limit)}` : null
  const text = [...lines, ...(amount ? [`(${amount})`] : [])].join(' ')
  const show = () => {
    if (timer.current) window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setOpen(true), 150)
  }
  const hide = () => {
    if (timer.current) window.clearTimeout(timer.current)
    setOpen(false)
    setPlace(null)
  }
  return (
    <span
      ref={anchor}
      className="composer-context"
      role="img"
      tabIndex={0}
      aria-label={text}
      onPointerEnter={show}
      onPointerLeave={hide}
      onFocus={show}
      onBlur={hide}
    >
      {open && createPortal(
        <span
          className="composer-context-card"
          role="tooltip"
          style={place ? { right: place.right, bottom: place.bottom } : { left: -9999, top: -9999 }}
        >
          <span className="composer-context-card-main">{lines[0]}</span>
          {lines[1] && <span className="composer-context-card-note">{lines[1]}</span>}
          {amount && <span className="composer-context-card-amount">{amount}</span>}
        </span>,
        document.body,
      )}
      <svg viewBox="0 0 16 16" aria-hidden="true">
        <circle className="composer-context-track" cx="8" cy="8" r={METER_RADIUS} />
        <circle
          className="composer-context-fill"
          cx="8"
          cy="8"
          r={METER_RADIUS}
          strokeDasharray={`${(METER_LENGTH * percent) / 100} ${METER_LENGTH}`}
        />
      </svg>
    </span>
  )
}
