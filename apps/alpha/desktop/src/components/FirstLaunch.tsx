import { useEffect, useState } from 'react'
import { ArrowRight } from 'lucide-react'
import BrandMotion, { type BrandStage } from './BrandMotion'
import BannerMotion from './BannerMotion'
import { watchWindowShift } from '../window-shift'
import './FirstLaunch.css'
import { W } from '../words'

// 글과 시작 단추가 나타나기까지(로고 움직임은 그 뒤에도 이어진다). 예전에는 움직임이 다 끝날 때(최소 약 5.1초)까지 막았다(2026-11-18 G71).
const READY_MS = 900

export function FirstLaunch({ onStart }: { onStart: () => void }) {
  const [stage, setStage] = useState<BrandStage>('intro')
  const [early, setEarly] = useState(false)
  // 창을 최대화하거나 되돌리는 동안 내용이 두 겹으로 어긋나 보이지 않게 한다(window-shift.ts).
  useEffect(() => watchWindowShift(), [])
  useEffect(() => {
    const timer = window.setTimeout(() => setEarly(true), READY_MS)
    return () => window.clearTimeout(timer)
  }, [])
  const ready = early || stage === 'settled'
  return (
    <main className="first-launch">
      <div className="first-launch-main">
        {/* 글과 단추가 0.9초에 나오므로 로고는 가운데에 크게 놓지 않고 제자리에서 그린다(크게 놓이면 그 위를 덮고 누름도 가로챈다, 2026-11-20). */}
        <div className="first-launch-mark"><BrandMotion onStage={setStage} inPlace /></div>
        <div className={`first-launch-copy ${ready ? 'is-visible' : ''}`}>
          <h1>{W.firstLaunch.text1}<strong>{W.firstLaunch.text2}</strong></h1>
          {/* 로고가 자리 잡아 누를 수 있게 되면 초점을 둔다(키보드로 창 단추 넷을 지나지 않고 Enter 로 시작, 2026-11-17). */}
          <button
            type="button"
            onClick={onStart}
            disabled={!ready}
            ref={(element) => { if (element && ready && (!document.activeElement || document.activeElement === document.body)) element.focus() }}
          >{W.firstLaunch.text3}<ArrowRight size={18} aria-hidden /></button>
        </div>
      </div>
      <div data-brand-bottom className={`first-launch-banner ${stage === 'settled' ? 'is-visible' : ''}`} aria-hidden="true">
        <BannerMotion />
      </div>
    </main>
  )
}
