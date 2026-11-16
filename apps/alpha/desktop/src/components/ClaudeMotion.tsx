import { useEffect, useRef, useState } from 'react'

type Point = { x: number; y: number }
type Segment = { command: 'M' | 'L' | 'C' | 'Z'; points: Point[] }

const rayAngles = [-180, -148, -114, -80, -48, -9, 12, 40, 60, 89, 121, 149]

function parseOutline(outline: string): Segment[] {
  const tokens = outline.match(/[MLHVCZ]|-?(?:\d+\.?\d*|\.\d+)(?:e[-+]?\d+)?/gi) ?? []
  const segments: Segment[] = []
  let x = 0
  let y = 0
  for (let i = 0; i < tokens.length;) {
    const command = tokens[i++]
    if (command === 'Z') { segments.push({ command, points: [] }); continue }
    if (command === 'H') { x = Number(tokens[i++]); segments.push({ command: 'L', points: [{ x, y }] }); continue }
    if (command === 'V') { y = Number(tokens[i++]); segments.push({ command: 'L', points: [{ x, y }] }); continue }
    if (command !== 'M' && command !== 'L' && command !== 'C') throw new Error('Unsupported logo path')
    const points: Point[] = []
    for (let n = 0; n < (command === 'C' ? 3 : 1); n++) {
      x = Number(tokens[i++]); y = Number(tokens[i++]); points.push({ x, y })
    }
    segments.push({ command, points })
  }
  return segments
}

function animatedOutline(segments: Segment[], time: number, strength = 0.055): string {
  return segments.map(({ command, points }) => {
    const values = points.map(({ x, y }) => {
      const dx = x - 124
      const dy = y - 124
      const distance = Math.hypot(dx, dy)
      const angle = Math.atan2(dy, dx)
      let sum = 0
      let weights = 0
      rayAngles.forEach((degrees, index) => {
        const delta = angle - degrees * Math.PI / 180
        const weight = Math.exp(8 * (Math.cos(delta) - 1))
        sum += weight * Math.sin(time * (1.05 + (index % 4) * 0.13) + index * 1.83)
        weights += weight
      })
      const influence = Math.max(0, Math.min(1, (distance - 24) / 75))
      const scale = 1 + strength * influence * sum / weights
      return `${(124 + dx * scale).toFixed(2)} ${(124 + dy * scale).toFixed(2)}`
    })
    return command + values.join(' ')
  }).join('')
}

/**
 * Claude 로고의 빛살 움직임. strength 는 빛살이 늘고 주는 폭(기본 5.5%, 첫 화면), speed 는 빠르기 배수.
 * 작게 그리는 곳(아이콘 띠의 사용량, 로고 21px)에서는 기본 폭으로는 1px 도 움직이지 않아 폭과 빠르기를 키운다.
 */
export function ClaudeMotion({ active, strength = 0.055, speed = 1 }: { active: boolean; strength?: number; speed?: number }) {
  const path = useRef<SVGPathElement>(null)
  const [segments, setSegments] = useState<Segment[] | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    void fetch('/brand/claude-official.svg', { signal: controller.signal })
      .then((response) => { if (!response.ok) throw new Error('Logo unavailable'); return response.text() })
      .then((text) => {
        const outline = new DOMParser().parseFromString(text, 'image/svg+xml').querySelector('path')?.getAttribute('d')
        if (outline) setSegments(parseOutline(outline))
      }).catch(() => {})
    return () => controller.abort()
  }, [])

  useEffect(() => {
    if (!active || !segments || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
    let frame = 0
    const started = performance.now()
    const animate = (now: number) => {
      path.current?.setAttribute('d', animatedOutline(segments, ((now - started) / 1000) * speed, strength))
      frame = requestAnimationFrame(animate)
    }
    frame = requestAnimationFrame(animate)
    return () => cancelAnimationFrame(frame)
  }, [active, segments, strength, speed])

  if (!active || !segments) return null
  return <svg className="claude-motion" viewBox="0 0 248 248" aria-hidden="true"><path ref={path} d={animatedOutline(segments, 0, strength)} fill="#D97757" /></svg>
}
