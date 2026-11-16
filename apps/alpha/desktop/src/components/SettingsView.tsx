import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Archive, BookOpen, Cloud, HardDrive, Info, Sparkles, UserRound, Wand2, X } from 'lucide-react'
import { UserProfileSettings } from './UserProfileSettings'
import { AppFolderSetting, AppInfoSettings } from './AppSettingsParts'
import { SkillsSettings } from './SkillsSettings'
import { ArchivedChatsSettings } from './ArchivedChatsSettings'
import { BackupSettings } from './BackupSettings'
import { ProviderSignIn, useProviderAuth } from './ProviderSignIn'
import { ChatImportSettings } from './ChatImportSettings'
import { TermDictionarySettings } from './TermDictionarySettings'
import type { SkillEntry } from '../skills'
import type { AppUpdateState } from './AppUpdate'
import { W } from '../words'

// 2026-11-16 김성균 개발자의 지시("방향을 바꾸기 전 옛화면에 남아있는거 그런거는 이제 필요 없는거잖아")로
// 꺼 두었던 옛 칸(작업공간, 참여자 표기, 채팅방 이력 보관, 응답 밀도, 개인정보 경계, 이전 채팅방 가져오기, 예전 백업 화면,
// 예전 경로·앱 정보 칸)과 예전 AI 연결 칸, 저장공간의 수동 백업, 옛 "새 기록" 창에 딸린 "기록" 쪽(원자료 보관, 녹음 동의 확인)을 없앴다.
export type SettingsSection = 'profile' | 'ai' | 'skills' | 'dictionary' | 'archived' | 'backup' | 'storage' | 'about'

type SettingsViewProps = {
  /** 처음 열 쪽(채팅의 알림 줄에서 "AI 연결 열기"를 누르면 'ai'). */
  initialSection?: SettingsSection
  /** 맨 위 줄의 오른쪽 끝(닫기 단추 오른쪽)에 놓는 단추. */
  headerActions?: ReactNode
  onClose: () => void
  onOpenDataRoot: () => void
  /** 앱 버전과 업데이트(App 의 useAppUpdate). */
  update: AppUpdateState
  /** 스킬 목록. null 이면 스킬 명령이 없어 "스킬" 쪽을 그리지 않는다. */
  skills?: SkillEntry[] | null
  onSkillsChanged?: () => void
  /** 보관된 채팅방 쪽을 그릴지(보관 명령이 있을 때만). 되돌리거나 지우면 onChatsChanged 로 왼쪽 목록을 다시 읽는다. */
  archivedChats?: boolean
  onChatsChanged?: () => void
}

const SECTIONS: Array<{ id: SettingsSection; label: string; icon: typeof Info }> = [
  { id: 'profile', label: W.settingsView.text1, icon: UserRound },
  { id: 'ai', label: W.settingsView.text4, icon: Sparkles },
  // 스킬: 스킬 명령이 있을 때만 보인다.
  { id: 'skills', label: W.skills.word, icon: Wand2 },
  // 용어 사전(2026-11-16, 보기만): 용어가 수백 개라 다른 쪽에 넣지 않고 따로 둔다. AI 가 알아듣는 데 쓰는 것이라 스킬 다음.
  { id: 'dictionary', label: W.termDictionary.title, icon: BookOpen },
  // 보관된 채팅방: 보관 명령이 있을 때만 보인다.
  { id: 'archived', label: W.chatKeep.settingsTitle, icon: Archive },
  { id: 'backup', label: W.settingsView.text5, icon: Cloud },
  { id: 'storage', label: W.settingsView.text6, icon: HardDrive },
  { id: 'about', label: W.settingsView.text7, icon: Info },
]

export function SettingsView({
  headerActions,
  onClose,
  onOpenDataRoot,
  update,
  skills = null,
  onSkillsChanged,
  archivedChats = false,
  onChatsChanged,
  initialSection,
}: SettingsViewProps) {
  // 설정의 AI 로그인: 명령이 있으면 회사 셋의 로그인 칸(undefined = 알아보는 중, null = 명령 없음).
  const providerAuth = useProviderAuth()
  const [section, setSection] = useState<SettingsSection>(initialSection ?? 'profile')
  const contentRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 })
  }, [section])

  return (
    <section className="settings-workspace" aria-labelledby="settings-title">
      <header className="settings-header">
        <div>
          <h1 id="settings-title">{W.settingsView.text11}</h1>
        </div>
        <div className="settings-header-actions">
          <button className="icon-button" type="button" onClick={onClose} aria-label={W.settingsView.aria1} title={W.settingsView.aria1}>
            <X size={18} aria-hidden />
          </button>
          {headerActions}
        </div>
      </header>

      <div className="settings-layout">
        <nav className="settings-navigation" aria-label={W.settingsView.aria2}>
          {SECTIONS.filter((item) => (item.id !== 'skills' || skills) && (item.id !== 'archived' || archivedChats)).map((item) => {
            const Icon = item.icon
            return (
              <button
                key={item.id}
                type="button"
                className={section === item.id ? 'active' : ''}
                aria-pressed={section === item.id}
                onClick={() => setSection(item.id)}
              >
                <Icon size={16} strokeWidth={1.8} aria-hidden />
                <span>{item.label}</span>
              </button>
            )
          })}
        </nav>

        <div className="settings-content" ref={contentRef}>
          {section === 'profile' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text1}</h2>
                <p>{W.settingsView.text16}</p>
              </header>
              <UserProfileSettings />
            </div>
          )}

          {section === 'ai' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text4}</h2>
                {providerAuth.list && <p>{W.signIn.intro}</p>}
              </header>
              {/* 로그인 명령(provider_auth_status)이 있으면 회사 셋의 로그인 칸. 예전 연결 칸(구독 계정/API 키 고르기)은 없앴다. */}
              {providerAuth.list && <ProviderSignIn list={providerAuth.list} onChanged={providerAuth.refresh} />}
              {/* 다른 AI 에서 내보낸 대화 가져오기(2026-11-16, 가장 작게 한 벌). 같은 회사들의 일이라 AI 연결 아래에 둔다. */}
              <ChatImportSettings onChanged={() => onChatsChanged?.()} />
            </div>
          )}

          {section === 'backup' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text5}</h2>
                <p>{W.backup.pageIntro}</p>
              </header>
              <BackupSettings />
            </div>
          )}

          {section === 'storage' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text6}</h2>
              </header>
              {/* 몸소 폴더 한 줄(위치, 폴더 열기, 위치 바꾸기). */}
              <AppFolderSetting fallbackOpen={() => onOpenDataRoot()} />
            </div>
          )}

          {section === 'skills' && skills && <SkillsSettings skills={skills} onChanged={() => onSkillsChanged?.()} />}

          {section === 'dictionary' && <TermDictionarySettings />}

          {section === 'archived' && archivedChats && <ArchivedChatsSettings onChanged={() => onChatsChanged?.()} />}

          {section === 'about' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text7}</h2>
                <p>{W.settingsView.text51}</p>
              </header>
              <AppInfoSettings update={update} />
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
