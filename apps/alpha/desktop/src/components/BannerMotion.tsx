'use client'

import { useEffect, useRef, useState } from 'react'

/**
 * 메인 화면 아래의 요가 그림 띠 — 움직이는 버전.
 *
 * - 바닥 선은 움직이지 않는다.
 * - 세 가지 그림(삼각 자세 · 긴 고리 · 아치)이 반복되며 오른쪽에서 왼쪽으로 천천히 흘러간다.
 * - 오른쪽 끝에서는 붓으로 그리듯 나타나고(왼발에서 시작해 몸 선을 따라),
 *   왼쪽 끝에서는 같은 순서로 지워지며 사라진다.
 * - 머리 점은 세 그림 모두 같은 방식이다: 몸 선이 머리 옆을 처음 지나간 직후 가운데서부터 번지듯 찍히고,
 *   지워질 때는 머리 옆의 몸 선이 마지막으로 지워질 때 가운데로 줄어들며 사라진다
 *   (옆에 몸이 아직 남아 있는데 얼굴만 먼저 사라지지 않는다).
 * - 그리기와 지우기는 띠의 가장자리에 붙어서 일어난다. 단, 붓끝이 가장자리 밖에 있는 동안에는 그리지 않는다
 *   (그려진 선이 가려졌다가 나중에 밀려 나오면 어색하다). 선이 왼쪽에서 오른쪽으로만 가는 두 그림은
 *   그림이 들어오는 만큼 그리면 되지만, 긴 고리는 선이 오른쪽으로 갔다가 왼쪽으로 되돌아오므로
 *   구간을 따로 준다(아래 FIGURES의 draw · erase).
 *
 * 원본 띠(banner.png)는 수정하지 않는다. 그림 조각 세 장(banner-fig-N.png)은 원본에서 잘라 만든 자료 이미지로,
 * 빨강에 "그 점이 몇 번째로 그려지는지", 초록에 "몇 번째로 지워지는지", 파랑에 그림의 모양(불투명도)이 들어 있다
 * (scripts/build-banner-motion.py). 아래 숫자들은 그 스크립트가 출력한 값이다.
 *
 * 긴 고리는 선이 자기 자신과 한 번 겹친다(가는 선과 굵은 선의 이음새). 원본에서 굵은 선의 가장자리는 어디나
 * 부드럽지만, 이음새에서는 가는 선이 그 가장자리를 덮고 있어 "굵은 선만의 가장자리"가 원본에 없다. 그래서 이음새의
 * 점을 한쪽 선의 것으로만 치면, 한 선만 보이는 동안(그릴 때는 가는 선만, 지울 때는 굵은 선만) 그 자리 윤곽만
 * 계단처럼 딱딱해진다. 긴 고리의 자료 이미지는 그림 너비의 세 칸으로 되어 있다: ① 기본(이음새 주변에서는 먼저
 * 지나가는 선), ② 이음새 주변에서 나중에 지나가는 선의 순서, ③ 각 점에서 두 선이 차지하는 몫(빨강=먼저, 초록=나중).
 * 두 선을 따로 계산해 더하고(원본보다 진해지지는 않게), 각 선은 이음새에서도 자기 가장자리가 부드럽다.
 *
 * 자료 이미지는 반드시 원래 크기 그대로 읽는다. 자료를 화면 크기로 줄이면 순서 값이 다른 두 선의 경계가
 * 뭉개져서, 선이 겹치는 곳에 이가 빠진 것처럼 보인다. 그래서 원래 크기에서 한 장을 다 그린 다음,
 * 그 결과만 화면 크기로 줄여서 붙인다.
 */
const SOURCE_HEIGHT = 188 // 원본 띠의 높이(px)
const GROUND = { centre: 184.95, thickness: 4.29 } // 바닥 선
const REPEAT = 609 // 세 그림 한 묶음의 가로 길이
/**
 * draw · erase(화면 px, 없으면 기본값):
 *  - draw.start: 그림의 왼쪽 끝이 오른쪽 가장자리에서 이만큼 들어왔을 때 그리기 시작, draw.span: 그 뒤 이만큼 가는 동안 다 그린다.
 *  - erase.lead: 그림의 왼쪽 끝이 왼쪽 가장자리에 닿기 이만큼 전에 지우기 시작, erase.span: 그 뒤 이만큼 가는 동안 다 지운다.
 * 기본값은 "그림이 들어오는 만큼 그리고, 빠져나가는 만큼 지운다"(start 0, lead 22, span = 그림 너비 + 22).
 * 긴 고리의 값은 붓끝·지우개가 가장자리의 흐려지는 구간(10px)에 걸리지 않도록 그림 자료에서 계산한 값이고,
 * 그리는 빠르기와 지우는 빠르기를 같게 맞췄다(span이 같다). 지우기는 마지막 발끝이 흐려지는 구간에 닿기 전에
 * 끝나도록 조금 일찍 시작한다(발끝이 뿌연 얼룩으로 남았다 사라지지 않게).
 */
type Zone = { draw?: { start: number; span: number }; erase?: { lead: number; span: number } }
/** twoStrokes: 자료 이미지가 세 칸짜리(선이 자기 자신과 겹치는 그림). */
const FIGURES: ({ src: string; x: number; twoStrokes?: boolean } & Zone)[] = [
  { src: '/brand/motion/banner-fig-1.png', x: -3 },
  { src: '/brand/motion/banner-fig-2.png', x: 246, twoStrokes: true, draw: { start: 20, span: 110 }, erase: { lead: 104, span: 110 } },
  { src: '/brand/motion/banner-fig-3.png', x: 375 },
]
const EXTRA = 22 // 기본 구간에서, 그림이 다 들어온 뒤 조금 더 가서 마무리하는 거리(화면 px)
const INK = [55, 53, 47]
const DISPLAY_HEIGHT = 120 // 화면에서의 띠 높이(px)
const SPEED = 30 // 흘러가는 속도(화면 px/초)
const FEATHER = 0.06 // 붓끝의 부드러운 경계

const clamp01 = (x: number) => (x < 0 ? 0 : x > 1 ? 1 : x)

type Sprite = {
  width: number
  height: number
  alpha: Uint8ClampedArray
  order: Float32Array
  erase: Float32Array
  /** 이음새 주변에서 나중에 지나가는 선(세 칸짜리 자료일 때만). share: 각 점에서 먼저/나중 선이 차지하는 몫. */
  later: { order: Float32Array; erase: Float32Array; shareFirst: Float32Array; shareLater: Float32Array } | null
  full: HTMLCanvasElement
  work: HTMLCanvasElement
  workContext: CanvasRenderingContext2D
  buffer: ImageData
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

function makeCanvas(width: number, height: number) {
  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  return canvas
}

/** 그림 조각 하나를 준비한다. 자료 이미지를 원래 크기 그대로 읽는다(줄이지 않는다). */
function prepare(image: HTMLImageElement, twoStrokes: boolean): Sprite {
  const blocks = twoStrokes ? 3 : 1
  const width = image.naturalWidth / blocks
  const height = image.naturalHeight
  const scratch = makeCanvas(width * blocks, height)
  const context = scratch.getContext('2d', { willReadFrequently: true })
  if (!context) throw new Error('no 2d context')
  context.imageSmoothingEnabled = false
  context.drawImage(image, 0, 0)
  const pixels = context.getImageData(0, 0, width, height).data
  let later: Sprite['later'] = null
  if (twoStrokes) {
    const second = context.getImageData(width, 0, width, height).data
    const shares = context.getImageData(width * 2, 0, width, height).data
    later = {
      order: new Float32Array(width * height),
      erase: new Float32Array(width * height),
      shareFirst: new Float32Array(width * height),
      shareLater: new Float32Array(width * height),
    }
    for (let i = 0; i < width * height; i++) {
      later.order[i] = second[i * 4] / 255
      later.erase[i] = second[i * 4 + 1] / 255
      later.shareFirst[i] = shares[i * 4] / 255
      later.shareLater[i] = shares[i * 4 + 1] / 255
    }
  }
  const alpha = new Uint8ClampedArray(width * height)
  const order = new Float32Array(width * height)
  const erase = new Float32Array(width * height)
  const full = makeCanvas(width, height)
  const fullContext = full.getContext('2d')
  if (!fullContext) throw new Error('no 2d context')
  const fullData = fullContext.createImageData(width, height)
  for (let i = 0; i < width * height; i++) {
    order[i] = pixels[i * 4] / 255
    erase[i] = pixels[i * 4 + 1] / 255
    alpha[i] = pixels[i * 4 + 2]
    fullData.data[i * 4] = INK[0]
    fullData.data[i * 4 + 1] = INK[1]
    fullData.data[i * 4 + 2] = INK[2]
    fullData.data[i * 4 + 3] = alpha[i]
  }
  fullContext.putImageData(fullData, 0, 0)
  const work = makeCanvas(width, height)
  const workContext = work.getContext('2d')
  if (!workContext) throw new Error('no 2d context')
  const buffer = workContext.createImageData(width, height)
  for (let i = 0; i < width * height; i++) {
    buffer.data[i * 4] = INK[0]
    buffer.data[i * 4 + 1] = INK[1]
    buffer.data[i * 4 + 2] = INK[2]
  }
  return { width, height, alpha, order, erase, later, full, work, workContext, buffer }
}

export default function BannerMotion({ className = '' }: { className?: string }) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    let cancelled = false
    let frame = 0
    let resizeTimer = 0
    let observer: ResizeObserver | null = null

    Promise.all(FIGURES.map((figure) => loadImage(figure.src)))
      .then((images) => {
        if (cancelled) return
        const context = canvas.getContext('2d')
        if (!context) throw new Error('no 2d context')
        const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
        const scale = DISPLAY_HEIGHT / SOURCE_HEIGHT // 원본 px → 화면 px
        let ratio = 1 // 화면 px → 캔버스 px
        let cssWidth = 0
        let capacity = 0
        const sprites = images.map((image, index) => prepare(image, FIGURES[index].twoStrokes === true))
        let travelled = 0
        let last = 0
        let resizing = false

        const build = () => {
          const width = canvas.parentElement?.clientWidth ?? 0
          if (!width) return
          const nextRatio = Math.min(2.5, window.devicePixelRatio || 1)
          if (width === cssWidth && nextRatio === ratio) return
          cssWidth = width
          resizing = true
          window.clearTimeout(resizeTimer)
          resizeTimer = window.setTimeout(() => {
            if (cancelled) return
            resizing = false
            paint()
          }, 180)
          if (nextRatio !== ratio || width > capacity || capacity === 0) {
            ratio = nextRatio
            capacity = Math.ceil(Math.max(width, window.screen.width))
            canvas.width = Math.ceil(capacity * ratio)
            canvas.height = Math.round(DISPLAY_HEIGHT * ratio)
            canvas.style.width = `${capacity}px`
          }
          paint()
        }

        const paint = () => {
          if (!cssWidth) return
          const unit = scale * ratio // 원본 px → 캔버스 px
          const width = Math.round(cssWidth * ratio)
          context.clearRect(0, 0, canvas.width, canvas.height)
          context.imageSmoothingEnabled = true
          context.imageSmoothingQuality = 'high'

          // 바닥 선: 움직이지 않는다.
          context.fillStyle = `rgb(${INK[0]}, ${INK[1]}, ${INK[2]})`
          context.fillRect(0, (GROUND.centre - GROUND.thickness / 2) * unit, width, GROUND.thickness * unit)

          const repeat = REPEAT * unit
          const margin = 4 * ratio // 가장자리에서 이만큼 안쪽이 경계
          const soft = 10 * ratio // 경계의 부드러운 폭
          const offset = (travelled * ratio) % repeat
          for (let start = -repeat - offset; start < width; start += repeat) {
            for (let index = 0; index < FIGURES.length; index++) {
              const sprite = sprites[index]
              // 그림 조각은 원래 크기로 그린 뒤, 이 자리에 화면 크기(sizeX × sizeY)로 줄여 붙인다.
              const x = start + FIGURES[index].x * unit
              const sizeX = sprite.width * unit
              const sizeY = sprite.height * unit
              const right = x + sizeX
              if (right <= 0 || x >= width) continue
              // 창 크기가 바뀌는 동안에도 평소와 같은 방식으로 그린다. 다르게 그리면 가장자리 그림이
              // 다 그려진 모습으로 바뀌었다가 돌아와, 창을 키우고 줄일 때마다 띠가 깜빡인다.
              // 오른쪽 가장자리에서 그리고, 왼쪽 가장자리에서 지운다(구간은 그림마다 — 위 FIGURES 설명).
              const basic = sizeX + EXTRA * ratio
              const draw = FIGURES[index].draw
              const erase = FIGURES[index].erase
              const drawn = reduced ? 1 : clamp01((width - margin - x - (draw ? draw.start * ratio : 0)) / (draw ? draw.span * ratio : basic))
              const kept = reduced ? 1 : 1 - clamp01((margin - x + (erase ? erase.lead : EXTRA) * ratio) / (erase ? erase.span * ratio : basic))
              if (drawn <= 0 || kept <= 0) continue
              if (drawn >= 1 && kept >= 1) {
                context.drawImage(sprite.full, x, 0, sizeX, sizeY)
                continue
              }
              const head = drawn * (1 + FEATHER) // 여기까지 그려졌다
              const tail = (1 - kept) * (1 + FEATHER) - FEATHER // 여기까지 지워졌다
              const data = sprite.buffer.data
              const w = sprite.width
              const later = sprite.later
              for (let column = 0; column < w; column++) {
                // 가장자리 밖으로는 아무것도 보이지 않게 한다(부드럽게).
                const screen = x + (column + 0.5) * unit
                const inside = clamp01((width - margin - screen) / soft) * clamp01((screen - margin) / soft)
                for (let row = 0, i = column; row < sprite.height; row++, i += w) {
                  const a = sprite.alpha[i]
                  if (a === 0 || inside === 0) {
                    data[i * 4 + 3] = 0
                    continue
                  }
                  let shown = clamp01((head - sprite.order[i]) / FEATHER) * clamp01((sprite.erase[i] - tail) / FEATHER)
                  if (later && (later.shareLater[i] > 0 || later.shareFirst[i] < 1)) {
                    // 이음새 주변: 먼저 지나가는 선과 나중에 지나가는 선을 따로 계산해 더한다(원본보다 진해지지 않게).
                    const second = clamp01((head - later.order[i]) / FEATHER) * clamp01((later.erase[i] - tail) / FEATHER)
                    shown = Math.min(1, later.shareFirst[i] * shown + later.shareLater[i] * second)
                  }
                  data[i * 4 + 3] = a * inside * shown
                }
              }
              sprite.workContext.putImageData(sprite.buffer, 0, 0)
              context.drawImage(sprite.work, x, 0, sizeX, sizeY)
            }
          }
        }

        const tick = (now: number) => {
          // 탭이 가려졌다 돌아와도 건너뛰지 않도록 한 번에 최대 50ms만 진행한다.
          const step = last ? Math.min(50, now - last) : 0
          last = now
          if (!resizing) {
            travelled += (SPEED * step) / 1000
            paint()
          }
          frame = requestAnimationFrame(tick)
        }

        build()
        observer = new ResizeObserver(() => build())
        if (canvas.parentElement) observer.observe(canvas.parentElement)
        if (!reduced) frame = requestAnimationFrame(tick)
      })
      .catch(() => {
        if (!cancelled) setFailed(true)
      })

    return () => {
      cancelled = true
      cancelAnimationFrame(frame)
      window.clearTimeout(resizeTimer)
      observer?.disconnect()
    }
  }, [])

  return (
    <div className={`welcome-banner ${className}`}>
      {failed ? (
        <img src="/brand/banner.png" alt="" draggable={false} className="welcome-banner-fallback" />
      ) : (
        <canvas ref={canvasRef} className="welcome-banner-canvas" />
      )}
      <noscript>
        <img src="/brand/banner.png" alt="" className="welcome-banner-fallback" />
      </noscript>
    </div>
  )
}
