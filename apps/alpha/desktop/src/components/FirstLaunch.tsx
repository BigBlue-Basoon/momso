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
          <button type="button" onClick={onStart} disabled={stage !== 'settled'}>{W.firstLaunch.text3}<ArrowRight size={18} /></button>
        </div>
      </div>
      <div data-brand-bottom className={`first-launch-banner ${stage === 'settled' ? 'is-visible' : ''}`} aria-hidden="true">
        <BannerMotion />
      </div>
    </main>
  )
}
