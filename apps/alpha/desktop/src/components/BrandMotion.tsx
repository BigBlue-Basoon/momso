import { W } from '../words'
'use client'

import { useEffect, useRef, useState } from 'react'
import styles from './BrandMotion.module.css'

/**
 * 메인 히어로 CI 그리기 애니메이션.
 *
 * - 시작: 검은 자세만 보인다(옴 밑그림 없음).
 * - 옴을 붓으로 그리듯 세 획으로 드러낸다: 왼쪽 3 → 오른쪽 원(시계 방향) → 위쪽 나선.
 * - 옴과 자세가 겹치는 곳은 원본 CI의 색이 드러난다.
 * - 끝 장면은 원본 CI(agar-yoga.png)의 픽셀 그대로다. 다른 이미지로 바꿔 끼우지 않는다.
 * - 처음에는 히어로 가운데에 크게 나와 그려지고, 다 그려진 뒤 제자리로 줄어들며 옮겨 간다.
 *   그때 onStage('settled')를 알려 히어로 글자가 함께 나타나게 한다. 다시 보기는 이 흐름을 처음부터 반복한다.
 *
 * 원본 CI는 수정하지 않는다. 아래 두 보조 이미지는 원본에서 계산해 만든 것이다
 * (scripts/build-brand-motion.py):
 * - agar-yoga-figure.png: 옴을 지우고 자세만 검게 남긴 그림(시작 장면)
 * - agar-yoga-order.png: 옴의 각 점이 "몇 번째 획의 어느 시점"에 그려지는지 적은 순서 지도
 */
const MARK = '/brand/agar-yoga.png'
const FIGURE = '/brand/motion/agar-yoga-figure.png'
const ORDER = '/brand/motion/agar-yoga-order.png'

/**
 * 획별 시작 시각·길이(ms). 자세가 먼저 자리 잡은 뒤 그린다.
 * 앞 획이 끝나기 조금 전(OVERLAP)에 다음 획이 시작해 붓이 끊기지 않고 이어지는 느낌을 준다.
 */
const OVERLAP = 420
const THREE = { start: 950, duration: 1900 } // 왼쪽 3
const CIRCLE = { start: THREE.start + THREE.duration - OVERLAP, duration: 1200 } // 오른쪽 원
const SPIRAL = { start: CIRCLE.start + CIRCLE.duration - OVERLAP, duration: 1300 } // 위쪽 나선
const STROKES = [THREE, CIRCLE, SPIRAL]
const TOTAL = STROKES[2].start + STROKES[2].duration
/** 3의 가운데(위 곡선이 끝나고 아래 곡선이 시작되는 꺾임)의 위치. 여기서 붓이 잠깐 느려진다. */
const THREE_TURN = 0.4237
/** 붓끝의 부드러운 경계 폭(획 길이 대비). */
const FEATHER = 0.012
const MAX_PIXELS = 1600
/** 다시 보기: 옴이 옅어지며 사라지는 시간(ms)과, 그 뒤 다시 그리기 시작하는 시점. */
const FADE_OUT = 520
const REPLAY_FROM = 600
/** 다 그린 뒤 잠깐 머무는 시간, 제자리로 옮겨 가는 시간, 다시 보기 때 다시 커지는 시간(ms). */
const HOLD = 350
const SETTLE = 1100
const GROW = 900
const MOVE_EASE = 'cubic-bezier(0.65, 0, 0.35, 1)'

export type BrandStage = 'intro' | 'settled'

const ease = (x: number) => 0.5 - 0.5 * Math.cos(Math.PI * Math.min(1, Math.max(0, x)))

/** 경과 시간 → 그 획이 얼마나 그려졌는지(0..1). */
function strokeProgress(index: number, elapsed: number) {
  const { start, duration } = STROKES[index]
  const q = (elapsed - start) / duration
  if (q <= 0) return 0
  if (q >= 1) return 1
  if (index !== 0) return ease(q)
  // 숫자 3을 쓰듯: 위 곡선을 그리고 가운데에서 숨을 고른 뒤 아래 곡선을 그린다.
  const upper = 0.45
  return q < upper ? THREE_TURN * ease(q / upper) : THREE_TURN + (1 - THREE_TURN) * ease((q - upper) / (1 - upper))
}

function loadImage(src: string) {
  return new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image()
    image.decoding = 'async'
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error(`image failed: ${src}`))
    image.src = src
  })
}

function readPixels(image: HTMLImageElement, width: number, height: number) {
  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext('2d', { willReadFrequently: true })
  if (!context) throw new Error('no 2d context')
  context.imageSmoothingEnabled = true
  context.imageSmoothingQuality = 'high'
  context.drawImage(image, 0, 0, width, height)
  return context.getImageData(0, 0, width, height)
}

/** 순서 지도를 화면 해상도로 옮긴다. 값 = 획 번호 + 획 안에서의 위치(0..1). */
function sampleOrder(image: HTMLImageElement, width: number, height: number) {
  const ow = image.naturalWidth
  const oh = image.naturalHeight
  const raw = readPixels(image, ow, oh).data
  const order = new Float32Array(ow * oh)
  for (let i = 0; i < order.length; i++) order[i] = ((raw[i * 4] * 256 + raw[i * 4 + 1]) / 65535) * STROKES.length
  const out = new Float32Array(width * height)
  for (let y = 0; y < height; y++) {
    const fy = Math.min(oh - 1, Math.max(0, ((y + 0.5) * oh) / height - 0.5))
    const y0 = Math.floor(fy)
    const y1 = Math.min(oh - 1, y0 + 1)
    const wy = fy - y0
    for (let x = 0; x < width; x++) {
      const fx = Math.min(ow - 1, Math.max(0, ((x + 0.5) * ow) / width - 0.5))
      const x0 = Math.floor(fx)
      const x1 = Math.min(ow - 1, x0 + 1)
      const wx = fx - x0
      const a = order[y0 * ow + x0]
      const b = order[y0 * ow + x1]
      const c = order[y1 * ow + x0]
      const d = order[y1 * ow + x1]
      const stroke = Math.floor(a)
      // 서로 다른 획의 값은 섞지 않는다(섞으면 엉뚱한 시점에 조각이 먼저 나타난다).
      out[y * width + x] =
        Math.floor(b) === stroke && Math.floor(c) === stroke && Math.floor(d) === stroke
          ? (a * (1 - wx) + b * wx) * (1 - wy) + (c * (1 - wx) + d * wx) * wy
          : wy < 0.5
            ? wx < 0.5 ? a : b
            : wx < 0.5 ? c : d
    }
  }
  return out
}

export default function BrandMotion({
  onStage,
  inPlace = false,
  labels = { replay: W.brandMotion.text1, mark: W.brandMotion.text2 },
}: {
  onStage?: (stage: BrandStage) => void
  /**
   * 가운데에 크게 놓는 단계 없이 처음부터 제자리에서 그린다(다시 보기도). 세 획과 끝 장면은 같다.
   * 첫 화면은 글과 시작 단추를 일찍 보여서, 크게 놓이면 그 위를 덮는다(2026-11-20 본부).
   */
  inPlace?: boolean
  /** 화면 낭독기용 설명(언어별). */
  labels?: { replay: string; mark: string }
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const stageRef = useRef(onStage)
  stageRef.current = onStage
  const replayRef = useRef<(() => void) | null>(null)
  const [failed, setFailed] = useState(false)
  const [restingMark, setRestingMark] = useState(false)

  useEffect(() => {
    const canvas = canvasRef.current
    const button = buttonRef.current
    if (!canvas || !button) return
    let cancelled = false
    let frame = 0
    let timer = 0
    let resizeTimer = 0
    let observer: ResizeObserver | null = null
    let onResize: (() => void) | null = null

    Promise.all([loadImage(MARK), loadImage(FIGURE), loadImage(ORDER)])
      .then(([mark, figure, orderImage]) => {
        if (cancelled) return
        const context = canvas.getContext('2d')
        if (!context) throw new Error('no 2d context')
        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
        let elapsed = reduced ? TOTAL : 0
        let last = 0
        let running = false
        let fade = -1 // 다시 보기로 옅어지는 중이면 0 이상(ms)
        let paint: (() => void) | null = null
        let paintFade: ((amount: number) => void) | null = null
        let snapshot: (() => void) | null = null
        let stage: BrandStage = reduced ? 'settled' : 'intro'
        let renderScale = 1 // 크게 보일 때는 그만큼 촘촘하게 그린다

        // 히어로 가운데에 크게 놓일 때의 위치·배율(제자리 기준).
        const bigPlace = () => {
          if (inPlace) return { scale: 1, transform: 'translate(0px, 0px) scale(1)' }
          const slot = button.parentElement?.getBoundingClientRect() ?? button.getBoundingClientRect()
          // Center the visible mark in the window while keeping it below the window controls.
          const area = document.querySelector('.first-launch')?.getBoundingClientRect()
          const top = area?.top ?? 0
          const bottom = area?.bottom ?? window.innerHeight
          const areaWidth = area?.width ?? document.documentElement.clientWidth
          const areaLeft = area?.left ?? 0
          const centreY = (Math.max(0, top - 40) + Math.min(bottom, window.innerHeight)) / 2
          // The original 2000px image has transparent margins; the artwork spans about 72%.
          const availableHalfHeight = Math.min(centreY - top - 12, bottom - centreY - 12)
          const size = Math.max(1, Math.min(availableHalfHeight / 0.36, (areaWidth - 32) / 0.72, 1200))
          const scale = Math.max(1, size / slot.width)
          const dx = areaLeft + areaWidth / 2 - (slot.left + slot.width / 2)
          const dy = centreY - (slot.top + slot.height / 2)
          return { scale, transform: `translate(${dx}px, ${dy}px) scale(${scale})` }
        }
        const moveTo = (transform: string, duration: number) => {
          button.style.transition = duration ? `transform ${duration}ms ${MOVE_EASE}` : 'none'
          button.style.transform = transform
        }

        const build = () => {
          const cssWidth = canvas.clientWidth
          if (!cssWidth) return
          const width = Math.min(MAX_PIXELS, Math.round(cssWidth * renderScale * Math.min(3, window.devicePixelRatio || 1)))
          const height = Math.round((width * mark.naturalHeight) / mark.naturalWidth)
          if (canvas.width === width && canvas.height === height && paint) return
          canvas.width = width
          canvas.height = height
          const from = readPixels(figure, width, height).data // 검은 자세만
          const to = readPixels(mark, width, height).data // 원본 CI
          const order = sampleOrder(orderImage, width, height)
          // 두 그림이 다른 점(=옴이 지나가는 점)만 매 장면 다시 계산한다.
          let count = 0
          for (let i = 0; i < width * height; i++) {
            const p = i * 4
            if (from[p] !== to[p] || from[p + 1] !== to[p + 1] || from[p + 2] !== to[p + 2] || from[p + 3] !== to[p + 3]) count++
          }
          const index = new Uint32Array(count)
          const when = new Float32Array(count)
          count = 0
          for (let i = 0; i < width * height; i++) {
            const p = i * 4
            if (from[p] !== to[p] || from[p + 1] !== to[p + 1] || from[p + 2] !== to[p + 2] || from[p + 3] !== to[p + 3]) {
              index[count] = p
              when[count++] = order[i]
            }
          }
          const output = new ImageData(new Uint8ClampedArray(from), width, height)
          const out = output.data
          const progress = [0, 0, 0]
          paint = () => {
            for (let s = 0; s < STROKES.length; s++) progress[s] = strokeProgress(s, elapsed) * (1 + FEATHER)
            for (let k = 0; k < index.length; k++) {
              const p = index[k]
              const value = when[k]
              const stroke = value | 0
              const m = (progress[stroke] - (value - stroke)) / FEATHER
              if (m <= 0) {
                out[p] = from[p]; out[p + 1] = from[p + 1]; out[p + 2] = from[p + 2]; out[p + 3] = from[p + 3]
              } else if (m >= 1) {
                out[p] = to[p]; out[p + 1] = to[p + 1]; out[p + 2] = to[p + 2]; out[p + 3] = to[p + 3]
              } else {
                // 투명도까지 함께 섞어야 가장자리에 테두리가 생기지 않는다.
                const fa = from[p + 3]
                const ta = to[p + 3]
                const alpha = fa + (ta - fa) * m
                if (alpha <= 0) { out[p + 3] = 0; continue }
                out[p] = (from[p] * fa + (to[p] * ta - from[p] * fa) * m) / alpha
                out[p + 1] = (from[p + 1] * fa + (to[p + 1] * ta - from[p + 1] * fa) * m) / alpha
                out[p + 2] = (from[p + 2] * fa + (to[p + 2] * ta - from[p + 2] * fa) * m) / alpha
                out[p + 3] = alpha
              }
            }
            context.putImageData(output, 0, 0)
          }
          // 다시 보기: 지금 보이는 장면을 기억해 두고, 검은 자세만 남을 때까지 고르게 옅어지게 한다.
          const held = new Uint8ClampedArray(index.length * 4)
          snapshot = () => {
            for (let k = 0; k < index.length; k++) {
              const p = index[k]
              held[k * 4] = out[p]; held[k * 4 + 1] = out[p + 1]; held[k * 4 + 2] = out[p + 2]; held[k * 4 + 3] = out[p + 3]
            }
          }
          paintFade = (amount) => {
            for (let k = 0; k < index.length; k++) {
              const p = index[k]
              const h = k * 4
              const ha = held[h + 3]
              const fa = from[p + 3]
              const alpha = ha + (fa - ha) * amount
              if (alpha <= 0) { out[p + 3] = 0; continue }
              out[p] = (held[h] * ha + (from[p] * fa - held[h] * ha) * amount) / alpha
              out[p + 1] = (held[h + 1] * ha + (from[p + 1] * fa - held[h + 1] * ha) * amount) / alpha
              out[p + 2] = (held[h + 2] * ha + (from[p + 2] * fa - held[h + 2] * ha) * amount) / alpha
              out[p + 3] = alpha
            }
            context.putImageData(output, 0, 0)
          }
          fade = -1
          paint()
        }

        const tick = (now: number) => {
          // 탭이 가려졌다 돌아와도 건너뛰지 않도록 한 번에 최대 50ms만 진행한다.
          const step = last ? Math.min(50, now - last) : 0
          last = now
          if (fade >= 0) {
            fade += step
            paintFade?.(ease(fade / FADE_OUT))
            if (fade >= FADE_OUT) {
              fade = -1
              elapsed = REPLAY_FROM
            }
          } else {
            elapsed += step
            paint?.()
          }
          if (stage === 'intro' && fade < 0 && elapsed >= TOTAL + HOLD) settle()
          if (fade >= 0 || stage === 'intro') frame = requestAnimationFrame(tick)
          else running = false
        }
        // 다 그렸으면 제자리로 줄어들며 옮겨 가고, 글자가 함께 나타나게 알린다.
        const settle = () => {
          stage = 'settled'
          moveTo('translate(0px, 0px) scale(1)', SETTLE)
          window.clearTimeout(timer)
          timer = window.setTimeout(() => {
            stageRef.current?.('settled')
            timer = window.setTimeout(() => {
              // The finished mark is the original image and needs no canvas rebuild on resize.
              if (cancelled || stage !== 'settled') return
              renderScale = 1
              setRestingMark(true)
            }, SETTLE)
          }, 260)
        }
        const run = () => {
          if (running) return
          running = true
          last = 0
          frame = requestAnimationFrame(tick)
        }

        if (reduced) {
          stageRef.current?.('settled')
          setRestingMark(true)
        } else {
          const place = bigPlace()
          renderScale = place.scale
          moveTo(place.transform, 0)
          stageRef.current?.('intro')
        }
        build()
        // 첫 장면(검은 자세)이 화면에 올라간 다음 서서히 나타나게 한다.
        requestAnimationFrame(() => requestAnimationFrame(() => { if (!cancelled) button.dataset.ready = 'true' }))
        // Keep the current image visible while the window is resizing; rebuild once it settles.
        observer = new ResizeObserver(() => {
          if (stage === 'settled') return
          window.clearTimeout(resizeTimer)
          resizeTimer = window.setTimeout(() => {
            if (!cancelled) build()
          }, 180)
        })
        observer.observe(canvas)
        onResize = () => {
          if (stage === 'intro') {
            const place = bigPlace()
            renderScale = place.scale
            moveTo(place.transform, 0)
          }
        }
        window.addEventListener('resize', onResize)
        if (!reduced) run()
        replayRef.current = () => {
          if (reduced || stage !== 'settled' || fade >= 0 || !snapshot) return
          setRestingMark(false)
          // 글자가 먼저 사라지고, 옴이 옅어지면서 다시 가운데로 커진 뒤 처음부터 그린다.
          window.clearTimeout(timer)
          stage = 'intro'
          stageRef.current?.('intro')
          const place = bigPlace()
          button.style.transform = 'translate(0px, 0px) scale(1)'
          renderScale = place.scale
          build()
          snapshot?.()
          fade = 0
          void button.offsetWidth
          moveTo(place.transform, GROW)
          run()
        }
      })
      .catch(() => {
        if (cancelled) return
        setFailed(true)
        button.style.transform = ''
        button.dataset.ready = 'true'
        stageRef.current?.('settled')
      })

    return () => {
      cancelled = true
      replayRef.current = null
      cancelAnimationFrame(frame)
      window.clearTimeout(timer)
      window.clearTimeout(resizeTimer)
      if (onResize) window.removeEventListener('resize', onResize)
      observer?.disconnect()
    }
  }, [])

  return (
    <button
      ref={buttonRef}
      type="button"
      className={`brand-motion-button ${styles.replay}`}
      onClick={() => replayRef.current?.()}
      aria-label={labels.replay}
    >
      {failed ? (
        <img src={MARK} alt={labels.mark} draggable={false} className={styles.mark} />
      ) : (
        <>
          <canvas ref={canvasRef} className={`${styles.mark} ${restingMark ? styles.restingCanvas : ''}`} role="img" aria-label={labels.mark} />
          {restingMark && <img src={MARK} alt="" aria-hidden="true" draggable={false} className={styles.restingMark} />}
        </>
      )}
      <noscript>
        <img src={MARK} alt={labels.mark} className={styles.fallback} />
      </noscript>
    </button>
  )
}
