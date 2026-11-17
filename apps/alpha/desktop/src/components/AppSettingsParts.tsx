import { useCallback, useEffect, useState } from 'react'
import { isSelectionExpired, selectLocalPaths } from '../runtime'
import { FolderOpen } from 'lucide-react'
import { getArchiveStatus, openWebAddress } from '../archive'
import { folderMoveRatio, getFolderLocation, isFolderMoving, listenFolderMove, listenFolderMoveEnd, openFolderLocation, setFolderLocation, type FolderMoveProgress, type FolderMoveResult, type FolderMoveState } from '../app-folder'
import { W } from '../words'
import type { AppUpdateState } from './AppUpdate'

// 라이선스 전문: 공개 레포(BigBlue-Basoon/momso)의 LICENSE. 화면에서 이 레포로 가는 링크는 여기 하나뿐이다.
const LICENSE_URL = 'https://github.com/BigBlue-Basoon/momso/blob/main/LICENSE'

const dateFormat = new Intl.DateTimeFormat('ko-KR', { year: 'numeric', month: 'long', day: 'numeric' })
function formatDate(value: string | null | undefined) {
  if (!value) return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : dateFormat.format(date)
}


/** 옮기지 못한 까닭(archive_location_set 의 state)별 한 줄. */
const MOVE_ERROR: Partial<Record<FolderMoveState, string>> = {
  target_not_empty: W.appSettingsParts.moveNotEmpty,
  invalid: W.appSettingsParts.moveInvalid,
}

/**
 * 설정의 몸소 폴더 한 줄: 위치(경로), 폴더 열기, 위치 바꾸기. 바꾸면 옮기는 동안 막대가 보이고 끝나면 새 경로.
 * 막대는 옮긴 양/전체 양(archive-location-progress)이고, 전체를 모르면 오가는 막대(비율을 지어내지 않는다).
 * 옮긴 뒤 예전 폴더가 남아 있으면(previousKept) 한 줄로 알린다(지우는 단추는 없다).
 * 옮긴 곳이 OneDrive 같은 자동 올리기 폴더면 알린다. 위치 명령이 없으면 지금 경로만(읽을 수 있으면) 보이고 위치 바꾸기는 눌리지 않는다.
 */
export function AppFolderSetting({ fallbackOpen }: { fallbackOpen: (path: string) => void }) {
  const [path, setPath] = useState<string | null>(null)
  const [movable, setMovable] = useState(false)
  // 다른 화면에 갔다 와도 옮기는 중이면 막대가 이어서 보인다(isFolderMoving).
  const [moving, setMoving] = useState(isFolderMoving)
  const [progress, setProgress] = useState<FolderMoveProgress | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [oneDrive, setOneDrive] = useState(false)
  // 옮긴 뒤 그대로 남은 예전 폴더의 경로(경로를 모르면 빈 글자).
  const [kept, setKept] = useState<string | null>(null)
  // 위치를 읽는 중 / 읽음 / 읽지 못함(두 명령 모두 실패). 위치가 비고 단추만 잠겨 있지 않게 한 줄(2026-11-17).
  const [pathState, setPathState] = useState<'loading' | 'ok' | 'failed'>('loading')

  const loadPath = useCallback(() => {
    setPathState('loading')
    getFolderLocation()
      .then((location) => { setPath(location.path); setMovable(true); setPathState('ok') })
      .catch(() => {
        setMovable(false)
        getArchiveStatus().then((status) => { setPath(status.path); setPathState('ok') }).catch(() => { setPath(null); setPathState('failed') })
      })
  }, [])
  useEffect(() => { loadPath() }, [loadPath])

  // 옮긴 결과를 화면에 적는다(위치 바꾸기의 결과와 끝 이벤트가 같은 길로 온다).
  const applyResult = useCallback(async (result: FolderMoveResult, picked?: string) => {
    if (result.state === 'ok') {
      const location = await getFolderLocation().catch(() => null)
      setPath(location?.path ?? result.path ?? picked ?? null)
      setOneDrive(location?.onCloudSyncFolder === true)
      setKept(result.previousKept === true ? result.previousPath ?? '' : null)
      setError(null)
    } else if (result.state !== 'unchanged') {
      setError(MOVE_ERROR[result.state] ?? W.appSettingsParts.text1)
    }
    setMoving(false)
    setProgress(null)
  }, [])

  // 끝 이벤트(archive-location-done / -failed)와 옮기는 동안의 양을 이 화면이 열려 있는 동안 늘 듣는다.
  // 옮기기를 시작한 뒤 화면을 떠났다 돌아와도 막대와 끝난 결과가 맞게 보인다.
  useEffect(() => {
    let disposed = false
    const stops: Array<() => void> = []
    void listenFolderMoveEnd((result) => { void applyResult(result) }).then((stop) => { if (disposed) stop(); else stops.push(stop) }).catch(() => {})
    void listenFolderMove(setProgress).then((stop) => { if (disposed) stop(); else stops.push(stop) }).catch(() => {})
    return () => { disposed = true; stops.forEach((stop) => stop()) }
  }, [applyResult])

  async function changeLocation() {
    if (!movable || moving) return
    setError(null)
    setOneDrive(false)
    // 폴더 고르는 창은 뒤쪽이 연다(select_local_paths archive). 취소하면 빈 배열.
    const picked = (await selectLocalPaths('archive').catch(() => [] as string[]))[0]
    if (typeof picked !== 'string') return
    setKept(null)
    setProgress(null)
    setMoving(true)
    // 진행 양은 위의 듣기가 받는다(화면이 열릴 때부터 듣고 있다).
    try {
      await applyResult(await setFolderLocation(picked), picked)
    } catch (error) {
      setError(isSelectionExpired(error) ? W.selectAgain : W.appSettingsParts.text1)
      setMoving(false)
      setProgress(null)
    }
  }

  const ratio = folderMoveRatio(progress)

  return (
    <section className="settings-group app-folder" aria-labelledby="app-folder-title">
      <div className="settings-group-heading">
        <div>
          <h3 id="app-folder-title">{W.appFolder}</h3>
          <p>{W.appSettingsParts.text2(W.document, W.rawMaterial)}</p>
        </div>
        <div className="app-folder-actions">
          <button
            className="secondary-button"
            type="button"
            disabled={!path}
            onClick={() => { if (movable) void openFolderLocation().catch(() => {}); else if (path) fallbackOpen(path) }}
          >
            <FolderOpen size={15} aria-hidden />{W.appSettingsParts.text3}</button>
          <button className="secondary-button" type="button" disabled={!movable || moving} onClick={() => void changeLocation()}>{W.appSettingsParts.text4}</button>
        </div>
      </div>
      {path && <p className="app-folder-path" title={path}>{path}</p>}
      {!path && pathState === 'loading' && <p className="settings-checking" role="status">{W.appSettingsParts.pathLoading(W.appFolder)}</p>}
      {!path && pathState === 'failed' && (
        <div className="settings-problem" role="alert">
          <p>{W.appSettingsParts.pathFailed(W.appFolder)}</p>
          <button className="secondary-button" type="button" onClick={loadPath}>{W.firstRun.retry}</button>
        </div>
      )}
      {moving && (
        ratio === null ? (
          <div className="app-folder-progress is-indeterminate" role="status" aria-label={W.appSettingsParts.aria1}>
            <span />
          </div>
        ) : (
          <div className="app-folder-progress" role="progressbar" aria-label={W.appSettingsParts.aria1} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(ratio * 100)}>
            <span style={{ width: `${Math.max(2, ratio * 100)}%` }} />
          </div>
        )
      )}
      {error && <p className="app-folder-error" role="alert">{error}</p>}
      {kept !== null && !moving && <p className="app-folder-kept" role="status">{W.appSettingsParts.previousKept(kept || null)}</p>}
      {oneDrive && (
        <div className="app-folder-notice" role="alert">
          <p>{W.oneDrive.notice}</p>
          <div className="app-folder-notice-actions">
            <button className="secondary-button" type="button" autoFocus onClick={() => { setOneDrive(false); void changeLocation() }}>{W.oneDrive.pickOther}</button>
            <button className="secondary-button subtle" type="button" onClick={() => setOneDrive(false)}>{W.oneDrive.keep}</button>
          </div>
        </div>
      )}
    </section>
  )
}

/**
 * 설정의 "앱 정보": 지금 버전과 업데이트(최신인지, 새 버전과 바뀐 것·나온 날, 업데이트 확인·업데이트 단추, 받는 중 진행, 실패).
 * 버전 명령이 없으면 버전 줄을 그리지 않고, 업데이트 명령이 없으면 업데이트 쪽은 눌리지 않는다(글 없음).
 */
export function AppInfoSettings({ update }: { update: AppUpdateState }) {
  const { version, supported, info, phase, progress, error } = update
  const available = info?.state === 'available'
  return (
    <section className="settings-group app-info" aria-labelledby="app-info-title">
      {/* 버전 명령이 없으면 버전 줄을 그리지 않으므로 제목도 "업데이트"로 둔다. */}
      <h3 id="app-info-title">{version ? W.appSettingsParts.text5 : W.appSettingsParts.text6}</h3>
      {version && (
        <p className="app-info-version">
          <strong>{version.version}</strong>
          {formatDate(version.buildDate) && <span>{formatDate(version.buildDate)}</span>}
        </p>
      )}
      <div className="app-update" aria-disabled={!supported || undefined}>
        {supported && info?.state === 'up_to_date' && phase === 'idle' && <p className="app-update-state">{W.appSettingsParts.text7}</p>}
        {supported && info?.state === 'failed' && phase === 'idle' && <p className="app-update-state is-quiet">{W.appUpdate.notChecked}</p>}
        {supported && available && phase !== 'ready' && (
          <div className="app-update-new">
            <p className="app-update-state">{W.appSettingsParts.text8}<strong>{info?.latestVersion}</strong>{W.appSettingsParts.text9}{formatDate(info?.publishedAt) && <span className="app-update-date">{formatDate(info?.publishedAt)}</span>}
            </p>
            {info?.notes && <p className="app-update-notes">{info.notes}</p>}
          </div>
        )}
        {phase === 'downloading' && (
          <div className="app-update-progress" role="status" aria-label={W.appSettingsParts.aria2}>
            <span style={{ width: `${Math.max(4, progress ?? 4)}%` }} />
          </div>
        )}
        {phase === 'ready' && <p className="app-update-state">{W.appSettingsParts.text10}</p>}
        {error && <p className="app-update-error" role="alert">{error}</p>}
        <div className="app-update-actions">
          <button className="secondary-button" type="button" disabled={!supported || phase === 'checking' || phase === 'downloading'} onClick={update.check}>{W.appSettingsParts.text11}</button>
          <button className="confirm-button" type="button" disabled={!supported || !available || phase === 'downloading' || phase === 'ready'} onClick={update.install}>{W.appSettingsParts.text6}</button>
        </div>
      </div>
      {/* 저작권(2026-11-16 그의 말 "앱 정보에 … 저작권 그런거 문구도"). 글은 레포의 NOTICE·LICENSE 에서 가져와 짧게: 저작권 한 줄,
          라이선스 이름과 전문 링크, 다른 회사 이름·로고는 각 회사의 것. 장식 선·작은 라벨 없이 흐린 글로. */}
      <div className="app-legal">
        <p>{W.appSettingsParts.copyright}</p>
        <p>
          {W.appSettingsParts.license('PolyForm Shield 1.0.0')}{' '}
          <button className="app-legal-link" type="button" onClick={() => { void openWebAddress(LICENSE_URL).catch(() => {}) }}>{W.appSettingsParts.licenseLink}</button>
        </p>
        <p>{W.appSettingsParts.otherMarks}</p>
      </div>
    </section>
  )
}
