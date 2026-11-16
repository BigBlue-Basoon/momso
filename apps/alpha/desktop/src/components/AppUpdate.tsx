import { W } from '../words'
import { useCallback, useEffect, useState } from 'react'
import { ArrowDownToLine, RefreshCw } from 'lucide-react'
import { isMissingCommand } from '../ai-models'
import { checkUpdate, getAppVersion, installUpdate, listenUpdates, type AppVersion, type UpdateInfo } from '../app-update'
import { ShortcutTip } from './ShortcutTip'

export type UpdatePhase = 'idle' | 'checking' | 'downloading' | 'ready' | 'failed'

export type AppUpdateState = {
  version: AppVersion | null
  /** 업데이트 명령이 백엔드에 있는지. 없으면 업데이트 쪽은 눌리지 않는다. */
  supported: boolean
  info: UpdateInfo | null
  phase: UpdatePhase
  progress: number | null
  error: string | null
  check: () => void
  install: () => void
}

/** 앱 버전과 업데이트 상태. 설정의 "앱 정보"와 아이콘 띠의 업데이트 단추가 같이 쓴다. */
export function useAppUpdate(): AppUpdateState {
  const [version, setVersion] = useState<AppVersion | null>(null)
  const [supported, setSupported] = useState(false)
  const [info, setInfo] = useState<UpdateInfo | null>(null)
  const [phase, setPhase] = useState<UpdatePhase>('idle')
  const [progress, setProgress] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)

  const check = useCallback(() => {
    setPhase('checking')
    setError(null)
    checkUpdate()
      .then((next) => {
        // unavailable: 업데이트 설정이 아직 없음 → 업데이트 쪽은 눌리지 않게만(글 없음).
        setSupported(next.state !== 'unavailable')
        setInfo(next)
        // failed: 아직 릴리스가 없을 때도, 연결이 안 될 때도 온다(archive-backend-port-notes.md B14). 까닭을 단정하지 않고
        // 오류로 띄우지도 않는다. 설정의 앱 정보에 흐린 한 줄만(AppInfoSettings). 사람이 "업데이트 확인"을 다시 누를 수 있다.
        setPhase('idle')
      })
      .catch((reason) => {
        if (isMissingCommand(reason)) { setSupported(false); setPhase('idle'); return }
        // 명령은 있는데 부르지 못했으면 failed 와 같게(오류 꼴 없이).
        setSupported(true)
        setInfo({ state: 'failed', currentVersion: null, latestVersion: null, notes: null, publishedAt: null })
        setPhase('idle')
      })
  }, [])

  const install = useCallback(() => {
    setPhase('downloading')
    setProgress(null)
    setError(null)
    installUpdate().then((result) => {
      if (result?.state === 'downloaded') { setPhase('ready'); setProgress(100) }
      else if (result?.state === 'failed') { setPhase('failed'); setError(W.appUpdate.text2) }
      else if (result?.state === 'unavailable') { setPhase('idle'); setSupported(false) }
    }).catch(() => {
      setPhase('failed')
      setError(W.appUpdate.text2)
    })
  }, [])

  useEffect(() => {
    getAppVersion().then(setVersion).catch(() => setVersion(null))
    check()
    let stop: (() => void) | undefined
    let disposed = false
    void listenUpdates({
      onAvailable: (next) => setInfo((current) => ({ ...(current ?? { currentVersion: null, notes: null, publishedAt: null, latestVersion: null }), ...next, state: 'available' } as UpdateInfo)),
      onProgress: (percent) => { setPhase('downloading'); setProgress(percent) },
      onDone: () => { setPhase('ready'); setProgress(100) },
      onFailed: (reason) => { setPhase('failed'); setError(reason || W.appUpdate.text2) },
    }).then((unlisten) => { if (disposed) unlisten(); else stop = unlisten }).catch(() => {})
    return () => { disposed = true; stop?.() }
  }, [check])

  return { version, supported, info, phase, progress, error, check, install }
}

const RING = 2 * Math.PI * 15

/**
 * 아이콘 띠의 업데이트 단추(사용량 위). 받을 수 있는 업데이트가 있을 때만 보인다. 누르면 받기 시작하고(고리로 진행),
 * 다 받으면 다시 켜기 단추가 된다. 두 앱의 왼쪽 아래 업데이트 알림은 그때 떠 있지 않아 보지 못했다.
 */
export function UpdateButton({ update, onRestart }: { update: AppUpdateState; onRestart: () => void }) {
  const available = update.info?.state === 'available'
  if (!update.supported || (!available && update.phase !== 'downloading' && update.phase !== 'ready')) return null
  const ready = update.phase === 'ready'
  const downloading = update.phase === 'downloading'
  const label = ready ? W.appUpdate.text3 : downloading ? W.appUpdate.text4 : W.appUpdate.text5(update.info?.latestVersion ? ` ${update.info.latestVersion}` : '')
  return (
    <ShortcutTip label={label} side="right">
      <button
        className="rail-button update-button"
        type="button"
        aria-label={label}
        aria-busy={downloading || undefined}
        onClick={() => { if (ready) onRestart(); else if (!downloading) update.install() }}
      >
        {downloading && (
          <svg className="update-ring" viewBox="0 0 38 38" aria-hidden="true">
            <circle cx="19" cy="19" r="15" className="update-ring-track" />
            <circle cx="19" cy="19" r="15" className="update-ring-fill" strokeDasharray={`${(RING * (update.progress ?? 8)) / 100} ${RING}`} />
          </svg>
        )}
        {ready ? <RefreshCw size={18} strokeWidth={1.9} aria-hidden /> : <ArrowDownToLine size={18} strokeWidth={1.9} aria-hidden />}
      </button>
    </ShortcutTip>
  )
}
