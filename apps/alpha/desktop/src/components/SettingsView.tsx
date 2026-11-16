import { useEffect, useRef, useState, type ReactNode } from 'react'
import {
  Archive,
  Check,
  Cloud,
  Database,
  DatabaseBackup,
  FolderOpen,
  HardDrive,
  LockKeyhole,
  MessagesSquare,
  Mic2,
  Settings2,
  Info,
  Sparkles,
  Wand2,
  UserRound,
  X,
} from 'lucide-react'
import type { AppPreferences, ResponseDensity, SourceRetention } from '../preferences'
import { getRetentionLabel } from '../preferences'
import type { RuntimeSession } from '../runtime'
import { AiConnectionSettings } from './AiConnectionSettings'
import { UserOwnedBackupSettings } from './UserOwnedBackupSettings'
import { UserProfileSettings } from './UserProfileSettings'
import { AppFolderSetting, AppInfoSettings } from './AppSettingsParts'
import { SkillsSettings } from './SkillsSettings'
import { ArchivedChatsSettings } from './ArchivedChatsSettings'
import { BackupSettings } from './BackupSettings'
import { ProviderSignIn, useProviderAuth } from './ProviderSignIn'
import type { SkillEntry } from '../skills'
import type { AppUpdateState } from './AppUpdate'
import { W } from '../words'

type SettingsSection = 'profile' | 'general' | 'records' | 'ai' | 'skills' | 'archived' | 'backup' | 'storage' | 'about'

type SettingsViewProps = {
  /** 맨 위 줄의 오른쪽 끝(닫기 단추 오른쪽)에 놓는 단추. */
  headerActions?: ReactNode
  preferences: AppPreferences
  session: RuntimeSession
  onPreferenceChange: <K extends keyof AppPreferences>(key: K, value: AppPreferences[K]) => void
  onClose: () => void
  onOpenDataRoot: () => void
  onCreateBackup: () => Promise<string>
  onOpenAiImport: () => void
  /** 앱 버전과 업데이트(App 의 useAppUpdate). */
  update: AppUpdateState
  /** 스킬 목록. null 이면 스킬 명령이 없어 "스킬" 쪽을 그리지 않는다. */
  skills?: SkillEntry[] | null
  onSkillsChanged?: () => void
  /** 보관된 채팅방 쪽을 그릴지(보관 명령이 있을 때만). 되돌리거나 지우면 onChatsChanged 로 왼쪽 목록을 다시 읽는다. */
  archivedChats?: boolean
  onChatsChanged?: () => void
}

// 작업공간 쪽을 다시 보이려면 true.
const SHOW_GENERAL_SECTION = false as boolean
// 지금 설계에 없거나 효과가 없어진 항목(2026-11-16 정리, 기록 37절). 다시 보이려면 true. 코드는 남긴다.
const SHOW_PARTICIPANT_LABEL = false as boolean
const SHOW_ASSISTANT_HISTORY = false as boolean
const SHOW_PRIVACY_BOUNDARY = false as boolean
// 예전 백업 화면(GitHub·Cloudflare R2·Neon). 2026-11-16 결정으로 새 화면(BackupSettings)으로 바꿨다.
const SHOW_OLD_BACKUP = false as boolean
// 본부 결정(2026-11-16): 응답 밀도는 뺀다("AI가 이렇게 답해 주면 좋겠습니다" 칸이 그 일을 한다).
const SHOW_RESPONSE_DENSITY = false as boolean
// 본부 결정: 이전 채팅방 가져오기는 화면에서 뺀다(코드는 남김). 가져온 것이 지금 화면 어디에도 안 보인다.
const SHOW_AI_IMPORT = false as boolean
// 본부 결정: 기록 쪽 머리 글(예전 "기록"에 대한 말)은 뺀다.
const SHOW_RECORDS_INTRO = false as boolean
// 예전 저장공간의 경로 칸(로컬 데이터, 구조 데이터, 백업 경로)과 예전 앱 정보 칸. 몸소 폴더 한 줄과 "앱 정보" 쪽으로 바꿨다.
const SHOW_LOCAL_PATHS = false as boolean

const SECTIONS: Array<{
  id: SettingsSection
  label: string
  icon: typeof Settings2
}> = [
  { id: 'profile', label: W.settingsView.text1, icon: UserRound },
  // '일반'(작업공간 이름)은 지금 설계에 없어 화면에서 뺐다(2026-11-15). 코드는 아래에 남긴다.
  ...(SHOW_GENERAL_SECTION ? [{ id: 'general' as const, label: W.settingsView.text2, icon: Settings2 }] : []),
  { id: 'records', label: W.settingsView.text3, icon: Mic2 },
  { id: 'ai', label: W.settingsView.text4, icon: Sparkles },
  // 스킬: 스킬 명령이 있을 때만 보인다.
  { id: 'skills', label: W.skills.word, icon: Wand2 },
  // 보관된 채팅방: 보관 명령이 있을 때만 보인다.
  { id: 'archived', label: W.chatKeep.settingsTitle, icon: Archive },
  { id: 'backup', label: W.settingsView.text5, icon: Cloud },
  { id: 'storage', label: W.settingsView.text6, icon: HardDrive },
  { id: 'about', label: W.settingsView.text7, icon: Info },
]

function ToggleSetting({
  checked,
  description,
  label,
  onChange,
}: {
  checked: boolean
  description: string
  label: string
  onChange: (checked: boolean) => void
}) {
  return (
    <div className="setting-row">
      <div>
        <strong>{label}</strong>
        <p>{description}</p>
      </div>
      <button
        className={checked ? 'toggle-switch active' : 'toggle-switch'}
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
      >
        <span aria-hidden />
      </button>
    </div>
  )
}

function PathRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="storage-path-row">
      <span>{label}</span>
      <code title={value}>{value}</code>
    </div>
  )
}

export function SettingsView({
  headerActions,
  preferences,
  session,
  onPreferenceChange,
  onClose,
  onOpenDataRoot,
  onCreateBackup,
  onOpenAiImport,
  update,
  skills = null,
  onSkillsChanged,
  archivedChats = false,
  onChatsChanged,
}: SettingsViewProps) {
  // 설정의 AI 로그인: 명령이 있으면 회사 셋의 로그인 칸(undefined = 알아보는 중, null = 명령 없음).
  const providerAuth = useProviderAuth()
  const [section, setSection] = useState<SettingsSection>('profile')
  const [backupState, setBackupState] = useState<'idle' | 'working' | 'done' | 'error'>('idle')
  const [backupMessage, setBackupMessage] = useState('')
  const contentRef = useRef<HTMLDivElement>(null)
  const runtime = session.info
  const dataRoot = session.mode === 'ready' && runtime ? runtime.dataRoot : W.settingsView.text8(W.appFolder)
  const databasePath = session.mode === 'ready' && runtime ? runtime.databasePath : W.settingsView.text9
  const backupsPath = session.mode === 'ready' && runtime ? runtime.backupsPath : W.settingsView.text10

  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 })
  }, [section])

  async function createBackup() {
    setBackupState('working')
    setBackupMessage('')
    try {
      const path = await onCreateBackup()
      setBackupState('done')
      setBackupMessage(path)
    } catch (error) {
      setBackupState('error')
      setBackupMessage(error instanceof Error ? error.message : String(error))
    }
  }

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
          {section === 'general' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text2}</h2>
                <p>{W.settingsView.text12}</p>
              </header>

              <section className="settings-group" aria-labelledby="workspace-settings-title">
                <h3 id="workspace-settings-title">{W.settingsView.text13}</h3>
                <label className="setting-field">
                  <span>{W.settingsView.text14}</span>
                  <input
                    value={preferences.workspaceName}
                    maxLength={30}
                    onChange={(event) => onPreferenceChange('workspaceName', event.target.value)}
                  />
                  <small>{W.settingsView.text15}</small>
                </label>
              </section>
            </div>
          )}

          {section === 'profile' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text1}</h2>
                <p>{W.settingsView.text16}</p>
              </header>
              <UserProfileSettings />
            </div>
          )}

          {section === 'records' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text3}</h2>
                {SHOW_RECORDS_INTRO && <p>{W.settingsView.text17}</p>}
              </header>

              <section className="settings-group" aria-labelledby="record-defaults-title">
                <h3 id="record-defaults-title">{W.settingsView.text18}</h3>
                {SHOW_PARTICIPANT_LABEL && <label className="setting-field">
                  <span>{W.settingsView.text19}</span>
                  <input
                    value={preferences.defaultParticipantLabel}
                    maxLength={30}
                    onChange={(event) => onPreferenceChange('defaultParticipantLabel', event.target.value)}
                  />
                </label>}
                <label className="setting-field">
                  <span>{W.settingsView.text20}</span>
                  <select
                    value={preferences.sourceRetention}
                    onChange={(event) => onPreferenceChange('sourceRetention', event.target.value as SourceRetention)}
                  >
                    <option value="keep">{getRetentionLabel('keep')}</option>
                    <option value="after-review">{getRetentionLabel('after-review')}</option>
                    <option value="ask">{getRetentionLabel('ask')}</option>
                  </select>
                  <small>{W.settingsView.text21}</small>
                </label>
              </section>

              <section className="settings-group" aria-labelledby="review-flow-title">
                <h3 id="review-flow-title">{W.settingsView.text22}</h3>
                <ToggleSetting
                  label={W.settingsView.label1}
                  description={W.settingsView.description1}
                  checked={preferences.confirmRecordingConsent}
                  onChange={(checked) => onPreferenceChange('confirmRecordingConsent', checked)}
                />
              </section>
            </div>
          )}

          {section === 'ai' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text4}</h2>
                <p>{W.settingsView.text23}</p>
              </header>

              {/* 로그인 명령(provider_auth_status)이 있으면 회사 셋의 로그인 칸 하나로, 없으면 예전 연결 칸(겹치지 않게 둘 중 하나만). */}
              {providerAuth.list ? (
                <ProviderSignIn list={providerAuth.list} onChanged={providerAuth.refresh} />
              ) : providerAuth.list === null && (
                <section className="settings-group" aria-labelledby="ai-provider-title">
                  <div className="settings-group-heading">
                    <div>
                      <h3 id="ai-provider-title">{W.settingsView.text24}</h3>
                      <p>{W.settingsView.text25}</p>
                    </div>
                  </div>
                  <AiConnectionSettings preferences={preferences} onPreferenceChange={onPreferenceChange} />
                </section>
              )}

              {(SHOW_ASSISTANT_HISTORY || SHOW_RESPONSE_DENSITY) && <section className="settings-group" aria-labelledby="ai-context-title">
                <h3 id="ai-context-title">{W.settingsView.text26}</h3>
                {SHOW_ASSISTANT_HISTORY && <ToggleSetting
                  label={W.settingsView.label2}
                  description={W.settingsView.description2}
                  checked={preferences.persistAssistantHistory}
                  onChange={(checked) => onPreferenceChange('persistAssistantHistory', checked)}
                />}
                {SHOW_RESPONSE_DENSITY && <label className="setting-field">
                  <span>{W.settingsView.text27}</span>
                  <select
                    value={preferences.responseDensity}
                    onChange={(event) => onPreferenceChange('responseDensity', event.target.value as ResponseDensity)}
                  >
                    <option value="concise">{W.settingsView.text28}</option>
                    <option value="balanced">{W.settingsView.text29}</option>
                    <option value="detailed">{W.settingsView.text30}</option>
                  </select>
                </label>}
              </section>}

              {SHOW_PRIVACY_BOUNDARY && <section className="settings-group" aria-labelledby="privacy-boundary-title">
                <h3 id="privacy-boundary-title">{W.settingsView.text31}</h3>
                <div className="privacy-boundary">
                  <span className="privacy-boundary-icon" aria-hidden><LockKeyhole size={18} /></span>
                  <div>
                    <strong>{W.settingsView.text32}</strong>
                    <p>{W.settingsView.text33}</p>
                  </div>
                  <span className="local-policy-state">{W.settingsView.text34}</span>
                </div>
              </section>}

              {SHOW_AI_IMPORT && <section className="settings-group" aria-labelledby="ai-import-title">
                <div className="settings-group-heading settings-import-heading">
                  <div>
                    <h3 id="ai-import-title">{W.settingsView.text35}</h3>
                    <p>{W.settingsView.text36(W.provider.codex, W.provider.claude)}</p>
                  </div>
                  <button className="secondary-button" type="button" onClick={onOpenAiImport}>
                    <MessagesSquare size={15} aria-hidden />{W.settingsView.text37}</button>
                </div>
              </section>}
            </div>
          )}

          {section === 'backup' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text5}</h2>
                <p>{SHOW_OLD_BACKUP ? W.settingsView.text38 : W.backup.pageIntro}</p>
              </header>
              {/* 2026-11-16 결정: GitHub(문서와 고친 이력) + 오브젝트 스토리지(원자료). DB(Neon)를 쓰는 예전 화면은 껐다(코드는 남김). */}
              {SHOW_OLD_BACKUP ? <UserOwnedBackupSettings /> : <BackupSettings />}
            </div>
          )}

          {section === 'storage' && (
            <div className="settings-page">
              <header className="settings-page-heading">
                <h2>{W.settingsView.text6}</h2>
                <p>{W.settingsView.text39}</p>
              </header>

              {/* 몸소 폴더 한 줄(위치, 폴더 열기, 위치 바꾸기). 예전 "로컬 데이터" 경로 칸은 화면에서 뺐다. */}
              <AppFolderSetting fallbackOpen={() => onOpenDataRoot()} />

              {SHOW_LOCAL_PATHS && <section className="settings-group" aria-labelledby="local-storage-title">
                <div className="settings-group-heading">
                  <h3 id="local-storage-title">{W.settingsView.text40}</h3>
                  <button className="secondary-button" type="button" onClick={onOpenDataRoot}>
                    <FolderOpen size={15} aria-hidden />{W.settingsView.text41}</button>
                </div>
                <div className="storage-paths">
                  <PathRow label={W.appFolder} value={dataRoot} />
                  <PathRow label={W.settingsView.label3} value={databasePath} />
                  <PathRow label={W.settingsView.label4} value={backupsPath} />
                </div>
              </section>}

              <section className="settings-group" aria-labelledby="backup-title">
                <div className="settings-group-heading">
                  <div>
                    <h3 id="backup-title">{W.settingsView.text42}</h3>
                    <p>{W.settingsView.text43}</p>
                  </div>
                  <button
                    className="confirm-button"
                    type="button"
                    disabled={backupState === 'working'}
                    onClick={() => void createBackup()}
                  >
                    <DatabaseBackup size={15} aria-hidden />
                    {backupState === 'working' ? W.settingsView.text44 : W.settingsView.text45}
                  </button>
                </div>
                {backupMessage && (
                  <div className={`backup-result ${backupState}`} role="status">
                    {backupState === 'done' ? <Check size={14} aria-hidden /> : <Database size={14} aria-hidden />}
                    <span>{backupMessage}</span>
                  </div>
                )}
              </section>

              {SHOW_LOCAL_PATHS && <section className="settings-group app-information" aria-labelledby="app-information-title">
                <h3 id="app-information-title">{W.settingsView.text7}</h3>
                <dl>
                  <div><dt>{W.settingsView.text46}</dt><dd>{runtime?.appVersion ?? '0.1.0'}</dd></div>
                  <div><dt>{W.settingsView.text47}</dt><dd>{session.mode === 'error' ? W.settingsView.text48 : W.settingsView.text49}</dd></div>
                  <div><dt>{W.settingsView.text50}</dt><dd>{runtime?.schemaVersion ?? 1}</dd></div>
                </dl>
              </section>}
            </div>
          )}

          {section === 'skills' && skills && <SkillsSettings skills={skills} onChanged={() => onSkillsChanged?.()} />}

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
