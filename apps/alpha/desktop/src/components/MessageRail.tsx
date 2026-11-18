import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { observeSize } from '../resize-later'
import './MessageRail.css'
import { W } from '../words'

// 막대 하나의 높이(간격 포함). MessageRail.css 의 .message-rail button 높이와 같다.
const TICK_PITCH = 10

type MessageRailProps = {
  /** 대화가 스크롤되는 칸. */
  streamRef: RefObject<HTMLDivElement | null>
  /** 내가 한 말의 개수. 막대는 내가 한 말마다 하나씩 생긴다. */
  userMessageCount: number
}

function userMessages(stream: HTMLElement) {
  return Array.from(stream.querySelectorAll<HTMLElement>('.message.user'))
}

function topInStream(stream: HTMLElement, element: HTMLElement) {
  return element.getBoundingClientRect().top - stream.getBoundingClientRect().top + stream.scrollTop
}

/**
 * 대화 칸 왼쪽 가장자리의 막대 목차. 내가 한 말마다 막대가 하나씩 있고, 누르면 그 말로 간다.
 * 지금 보고 있는 자리의 막대는 더 진하게 보인다. 내가 한 말이 없으면 아무것도 그리지 않는다.
 */
export function MessageRail({ streamRef, userMessageCount }: MessageRailProps) {
  const railRef = useRef<HTMLDivElement>(null)
  const [current, setCurrent] = useState(0)
  const [frame, setFrame] = useState({ top: 0, height: 0 })

  // 지금 보고 있는 말: 보이는 칸의 맨 위(조금 아래)보다 위에서 시작한 마지막 말.
  // 긴 답을 읽는 중이면 그 답을 부른 내 말이 지금 자리다.
  const updateCurrent = useCallback(() => {
    const stream = streamRef.current
    if (!stream) return
    const marks = userMessages(stream)
    const line = stream.scrollTop + 24
    let next = 0
    marks.forEach((mark, index) => {
      if (topInStream(stream, mark) <= line) next = index
    })
    // 끝까지 내렸으면 마지막 말을 보고 있는 것으로 친다.
    if (stream.scrollTop + stream.clientHeight >= stream.scrollHeight - 2) next = Math.max(0, marks.length - 1)
    setCurrent(next)
  }, [streamRef])

  // 띠는 대화가 스크롤되는 칸의 높이에 맞춘다(아래 입력 칸에 가려지는 부분은 뺀다).
  const updateFrame = useCallback(() => {
    const stream = streamRef.current
    if (!stream) return
    const bottomPadding = Number.parseFloat(window.getComputedStyle(stream).paddingBottom) || 0
    const next = { top: stream.offsetTop, height: Math.max(0, stream.clientHeight - bottomPadding) }
    setFrame((previous) => (previous.top === next.top && previous.height === next.height ? previous : next))
  }, [streamRef])

  useLayoutEffect(() => {
    updateFrame()
    updateCurrent()
  }, [updateCurrent, updateFrame, userMessageCount])

  useEffect(() => {
    const stream = streamRef.current
    if (!stream) return
    const onScroll = () => updateCurrent()
    stream.addEventListener('scroll', onScroll, { passive: true })
    const stopObserving = observeSize(stream, () => {
      updateFrame()
      updateCurrent()
    })
    return () => {
      stream.removeEventListener('scroll', onScroll)
      stopObserving()
    }
  }, [streamRef, updateCurrent, updateFrame])

  // 막대가 띠보다 많으면 지금 보고 있는 막대가 띠 안에 들어오게 띠를 민다.
  useEffect(() => {
    const rail = railRef.current
    if (!rail) return
    const top = current * TICK_PITCH
    if (top < rail.scrollTop) rail.scrollTop = top
    else if (top + TICK_PITCH > rail.scrollTop + rail.clientHeight) rail.scrollTop = top + TICK_PITCH - rail.clientHeight
  }, [current, frame.height, userMessageCount])

  if (userMessageCount === 0 || frame.height < TICK_PITCH) return null

  function go(index: number) {
    const stream = streamRef.current
    if (!stream) return
    const mark = userMessages(stream)[index]
    if (!mark) return
    stream.scrollTo({ top: Math.max(0, topInStream(stream, mark) - 10), behavior: 'smooth' })
  }

  const overflowing = userMessageCount * TICK_PITCH > frame.height

  return (
    <div
      ref={railRef}
      className="message-rail"
      role="group"
      aria-label={W.messageRail.aria1}
      data-overflowing={overflowing}
      style={{ top: frame.top, height: Math.min(frame.height, userMessageCount * TICK_PITCH) }}
    >
      {Array.from({ length: userMessageCount }, (_, index) => (
        <button
          key={index}
          type="button"
          className={index === current ? 'current' : ''}
          aria-label={W.messageRail.aria2(index + 1)}
          aria-current={index === current ? 'true' : undefined}
          onClick={() => go(index)}
        />
      ))}
    </div>
  )
}
