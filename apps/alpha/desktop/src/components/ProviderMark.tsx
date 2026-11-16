import { useState } from 'react'
import { ClaudeMotion } from './ClaudeMotion'
import './ProviderMark.css'

export type ProviderMarkId = 'chatgpt' | 'claude' | 'gemini'

// 세 곳의 로고 파일(첫 화면을 만들 때 넣은 것). public/brand/ 에 있다.
const LOGO: Record<ProviderMarkId, string> = {
  chatgpt: '/brand/chatgpt-official.svg',
  claude: '/brand/claude-official.svg',
  gemini: '/brand/gemini-official.png',
}

/**
 * AI 세 곳의 로고와 올렸을 때의 효과. 첫 화면(AiWelcome)과 사용량 표시가 함께 쓴다.
 * 효과는 이 로고를 감싼 요소에 provider-mark-trigger 를 붙이면 그 요소에 마우스를 올리거나 초점이 갈 때 켜진다:
 * ChatGPT 는 천천히 돌고, Claude 는 빛살이 움직이고(active), Gemini 는 빛이 지나간다.
 */
export function ProviderMark({
  provider,
  active = false,
  onDark = false,
  motion,
  className = '',
}: {
  provider: ProviderMarkId
  /** Claude 의 움직임을 켤지. useProviderMarkHover 로 얻는다. */
  active?: boolean
  /** 어두운 바탕 위에 둘 때. ChatGPT 로고만 같은 파일의 흰색판으로 바뀐다(나머지는 색이 있어 그대로). */
  onDark?: boolean
  /** Claude 빛살 움직임의 폭과 빠르기(작게 그리는 곳에서 키운다). 없으면 첫 화면 그대로. */
  motion?: { strength: number; speed: number }
  className?: string
}) {
  return (
    <span className={`provider-mark provider-mark--${provider}${onDark ? ' provider-mark--on-dark' : ''} ${className}`.trim()} aria-hidden="true">
      <img src={LOGO[provider]} alt="" />
      {provider === 'claude' && <ClaudeMotion active={active} strength={motion?.strength} speed={motion?.speed} />}
    </span>
  )
}

/** Claude 로고의 움직임은 그림을 매 장면 다시 그려야 해서 마우스와 초점을 직접 따라간다. */
export function useProviderMarkHover() {
  const [hovered, setHovered] = useState(false)
  const [focused, setFocused] = useState(false)
  return {
    active: hovered || focused,
    handlers: {
      onPointerEnter: () => setHovered(true),
      onPointerLeave: () => setHovered(false),
      onFocus: () => setFocused(true),
      onBlur: () => setFocused(false),
    },
  }
}
