import { FileText, House, Images, Settings } from 'lucide-react'
import type { ReactNode } from 'react'
import { ShortcutTip } from './ShortcutTip'
import { UsageMeter } from './UsageMeter'
import type { ProviderAuth } from '../provider-auth'
import { UpdateButton, type AppUpdateState } from './AppUpdate'
import { W } from '../words'

export type AppScreen = 'home' | 'documents' | 'media' | 'settings'

type ActivityRailProps = {
  screen: AppScreen
  /** 미디어를 주는 백엔드가 있는지. 없으면 미디어 단추는 눌리지 않는다(글 없음). */
  mediaAvailable: boolean
  onNavigate: (screen: AppScreen) => void
  update: AppUpdateState
  onRestart: () => void
  /** AI 연결 상태(provider_auth_status). 사용량 단추가 확인되지 않은 키를 연결로 치지 않게 넘긴다. */
  providerAuth?: ProviderAuth[] | null
}

/**
 * 맨 왼쪽의 아이콘 띠. 위쪽에 홈(채팅방 화면), 문서(읽는 글 모아 보기), 미디어(만든 사진과 동영상),
 * 아래쪽에 AI 세 곳의 사용량과 설정. 이름은 올렸을 때 풍선으로 뜬다.
 */
export function ActivityRail({ screen, mediaAvailable, onNavigate, update, onRestart, providerAuth }: ActivityRailProps) {
  return (
    <aside className="activity-rail" aria-label={W.activityRail.aria1}>
      <nav className="rail-screens" aria-label={W.activityRail.aria2}>
        <RailButton label={W.tip.home} active={screen === 'home'} onClick={() => onNavigate('home')}>
          <House size={19} strokeWidth={1.75} aria-hidden />
        </RailButton>
        <RailButton label={W.tip.documents} active={screen === 'documents'} onClick={() => onNavigate('documents')}>
          <FileText size={19} strokeWidth={1.75} aria-hidden />
        </RailButton>
        <RailButton label={W.tip.media} active={screen === 'media'} disabled={!mediaAvailable} onClick={() => onNavigate('media')}>
          <Images size={19} strokeWidth={1.75} aria-hidden />
        </RailButton>
      </nav>

      {/* 받을 수 있는 업데이트가 있을 때만 사용량 위에 뜬다. */}
      <div className="rail-update-slot">
        <UpdateButton update={update} onRestart={onRestart} />
      </div>

      <UsageMeter auth={providerAuth} />

      <RailButton label={W.tip.settings} active={screen === 'settings'} className="rail-settings" onClick={() => onNavigate('settings')}>
        <Settings size={19} strokeWidth={1.75} aria-hidden />
      </RailButton>
    </aside>
  )
}

function RailButton({
  label,
  active,
  disabled = false,
  className = '',
  onClick,
  children,
}: {
  label: string
  active: boolean
  disabled?: boolean
  className?: string
  onClick: () => void
  children: ReactNode
}) {
  return (
    <ShortcutTip label={label} side="right">
      <button
        className={`rail-button rail-screen-button ${active ? 'active' : ''} ${className}`.trim()}
        type="button"
        aria-label={label}
        aria-current={active ? 'page' : undefined}
        aria-disabled={disabled || undefined}
        onClick={() => { if (!disabled) onClick() }}
      >
        {children}
      </button>
    </ShortcutTip>
  )
}
