import { useEffect, useState } from 'react'
import { ArrowRight } from 'lucide-react'
import BrandMotion, { type BrandStage } from './BrandMotion'
import BannerMotion from './BannerMotion'
import { watchWindowShift } from '../window-shift'
import './FirstLaunch.css'
import { W } from '../words'

export function FirstLaunch({ onStart }: { onStart: () => void }) {
  const [stage, setStage] = useState<BrandStage>('intro')
  // 창을 최대화하거나 되돌리는 동안 내용이 두 겹으로 어긋나 보이지 않게 한다(window-shift.ts).
  useEffect(() => watchWindowShift(), [])
  return (
    <main className="first-launch">
      <div className="first-launch-main">
        <div className="first-launch-mark"><BrandMotion onStage={setStage} /></div>
        <div className={`first-launch-copy ${stage === 'settled' ? 'is-visible' : ''}`}>
          <h1>{W.firstLaunch.text1}<strong>{W.firstLaunch.text2}</strong></h1>
          {/* 로고가 자리 잡아 누를 수 있게 되면 초점을 둔다(키보드로 창 단추 넷을 지나지 않고 Enter 로 시작, 2026-11-17). */}
          <button
            type="button"
            onClick={onStart}
            disabled={stage !== 'settled'}
            ref={(element) => { if (element && stage === 'settled' && (!document.activeElement || document.activeElement === document.body)) element.focus() }}
          >{W.firstLaunch.text3}<ArrowRight size={18} aria-hidden /></button>
        </div>
      </div>
      <div data-brand-bottom className={`first-launch-banner ${stage === 'settled' ? 'is-visible' : ''}`} aria-hidden="true">
        <BannerMotion />
      </div>
    </main>
  )
}
