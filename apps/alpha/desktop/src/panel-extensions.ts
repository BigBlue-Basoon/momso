// 오른쪽 패널 브라우저의 확장 프로그램(2026-11-21 줄1 스냅샷 1cdc137·b18832c "add browser extension import and store support" = 줄4 S12b+S28).
//   계약: 공유 Git hq/s28-done 469e15a8c docs/development/desktop-panel-extensions-s12.md. 패널을 먼저 연 뒤 부른다. 설치·켜짐은 모든 채팅방이 함께 쓴다.
//   panel_extensions_list() → [{ id, name, enabled, iconDataUrl, origin }](2026-11-22 스냅샷 1c369e5 "expose extension icons and origin" = 줄4 S41)
//     iconDataUrl: 확장 폴더의 manifest 그림을 data:image/(png|jpeg|gif|bmp|x-icon);base64,… 로, 없으면 null. 설치·가져오기 결과도 같은 모양.
//     origin: builtin(확인된 기본 탑재, Microsoft 가 넣어 둔 것) · user(몸소로 설치) · unknown(출처를 확인할 수 없는 옛 설치).
//     김성균 개발자의 결정대로 builtin 은 화면 어디에도 보이지 않게 여기서 뺀다(메뉴·관리 화면·고정 단추·개수 모두). unknown 은 보인다.
//   panel_extensions_install() → 고른 폴더를 설치하고 목록(폴더 고르기를 그만두면 cancelled)
//   panel_extensions_install_store({ addressOrId }) → 목록. 오류 invalid_store_address·store_unavailable·store_package_invalid·store_signature_invalid·store_id_mismatch
//   panel_extensions_set_enabled({ extensionId, enabled }) · panel_extensions_remove({ extensionId }) → null
//   panel_extensions_import_candidates() → [{ browser: chrome|edge, profile, extensionId, version, name }](비밀번호·쿠키는 읽지 않음)
//   panel_extensions_import({ browser, profile, extensionId, version }) → 목록(몸소 폴더로 복사, ID 가 달라질 수 있음)
//   panel_extensions_pin_get() → 고정한 ID 배열(고정한 차례) · panel_extensions_pin_set({ extensionId, pinned }) → null(화면 표시용, 켜고 끄지 않음)
//   panel_extension_popup_open({ extensionId, input: { kind, chatId, x, y, width, height } }) → { id }: S13 떠 있는 창과 같은 규칙의 창에 그 확장의 기본 팝업.
//     닫기는 panel_overlay_close({ id }). 이 명령은 hq/s28-done 469e15a8c 에 있고 스냅샷 b18832c 에는 아직 없다(없으면 화면은 "창을 열지 못했습니다").
//   이벤트 panel-extensions-changed: 설치·켜짐·삭제가 성공하면. 화면은 목록을 다시 읽는다. 고정은 이벤트가 없어 화면끼리 알린다(아래).
//   공통 오류 panel_unavailable·unsupported·unavailable·timeout(끝났는지 모름: 목록을 다시 읽음)·not_found·install_failed·operation_failed·cancelled,
//     가져오기·목록·고정 invalid_source·source_unavailable·catalog_invalid·installed_catalog_unavailable.
// 떠 있는 창(퍼즐 메뉴)과 메인 화면 사이(같은 앱 권한): 관리 화면 열기·고정 바뀜을 Tauri 이벤트로 알린다.
import { emit } from '@tauri-apps/api/event'
import { invoke, listen } from './host'

export type PanelExtension = { id: string; name: string; enabled: boolean; iconDataUrl: string | null; origin: 'user' | 'unknown' }
export type ExtensionCandidate = { browser: string; profile: string; extensionId: string; version: string; name: string }
export type ExtensionsSection = 'all' | 'import'

const str = (value: unknown) => (typeof value === 'string' ? value : '')

const ICON_DATA = /^data:image\/(png|jpeg|gif|bmp|x-icon);base64,/

function readList(value: unknown): PanelExtension[] {
  return Array.isArray(value) ? value.flatMap((item) => {
    const p = (item ?? {}) as Record<string, unknown>
    if (!str(p.id) || p.origin === 'builtin') return []
    const icon = str(p.iconDataUrl)
    return [{ id: str(p.id), name: str(p.name) || str(p.id), enabled: p.enabled === true, iconDataUrl: ICON_DATA.test(icon) ? icon : null, origin: p.origin === 'user' ? 'user' as const : 'unknown' as const }]
  }) : []
}

export async function listExtensions() {
  return readList(await invoke<unknown>('panel_extensions_list'))
}

/** 폴더에서 설치. 폴더 고르기를 그만두면 null. */
export async function installExtensionFolder() {
  try {
    const value = await invoke<unknown>('panel_extensions_install')
    return value === 'cancelled' ? null : readList(value)
  } catch (error) {
    if (extensionErrorCode(error) === 'cancelled') return null
    throw error
  }
}

export async function installExtensionFromStore(addressOrId: string) {
  return readList(await invoke<unknown>('panel_extensions_install_store', { addressOrId }))
}

export function setExtensionEnabled(extensionId: string, enabled: boolean) {
  return invoke<null>('panel_extensions_set_enabled', { extensionId, enabled })
}

export function removeExtension(extensionId: string) {
  return invoke<null>('panel_extensions_remove', { extensionId })
}

export async function importCandidates(): Promise<ExtensionCandidate[]> {
  const value = await invoke<unknown>('panel_extensions_import_candidates')
  return Array.isArray(value) ? value.flatMap((item) => {
    const p = (item ?? {}) as Record<string, unknown>
    return str(p.extensionId) && str(p.browser) ? [{ browser: str(p.browser), profile: str(p.profile), extensionId: str(p.extensionId), version: str(p.version), name: str(p.name) || str(p.extensionId) }] : []
  }) : []
}

export async function importExtension(candidate: ExtensionCandidate) {
  const { browser, profile, extensionId, version } = candidate
  return readList(await invoke<unknown>('panel_extensions_import', { browser, profile, extensionId, version }))
}

export async function pinnedExtensions() {
  const value = await invoke<unknown>('panel_extensions_pin_get')
  return Array.isArray(value) ? value.filter((id): id is string => typeof id === 'string' && Boolean(id)) : []
}

export function setExtensionPinned(extensionId: string, pinned: boolean) {
  return invoke<null>('panel_extensions_pin_set', { extensionId, pinned })
}

export async function openExtensionPopup(extensionId: string, chatId: string, rect: { x: number; y: number; width: number; height: number }) {
  const value = await invoke<{ id?: unknown }>('panel_extension_popup_open', { extensionId, input: { kind: 'extension-popup', chatId, ...rect } })
  if (typeof value?.id !== 'string') throw new Error('unavailable')
  return value.id
}

/** 뒤쪽 오류 값(글자로 오거나 Error 의 글). */
export function extensionErrorCode(error: unknown) {
  return typeof error === 'string' ? error : str((error as { message?: unknown } | null)?.message)
}

const PINS_EVENT = 'momso:extensions-pins-changed'
const OPEN_EVENT = 'momso:extensions-open'

/** 목록·고정이 바뀜(뒤쪽 panel-extensions-changed, 또는 다른 창이 고정을 바꿈). */
export async function listenExtensionsChanged(onChange: () => void) {
  const stops = await Promise.all([listen('panel-extensions-changed', () => onChange()), listen(PINS_EVENT, () => onChange())])
  return () => stops.forEach((stop) => stop())
}

export async function notifyPinsChanged() {
  await emit(PINS_EVENT, {})
}

/** 떠 있는 메뉴에서: 그 채팅방의 패널에 관리 화면을 연다. */
export async function requestExtensionsPage(chatId: string, section: ExtensionsSection) {
  await emit(OPEN_EVENT, { chatId, section })
}

export async function listenExtensionsPage(onOpen: (chatId: string, section: ExtensionsSection) => void) {
  return listen<{ chatId?: unknown; section?: unknown }>(OPEN_EVENT, (event) => {
    const chatId = str(event.payload?.chatId)
    if (chatId) onOpen(chatId, event.payload?.section === 'import' ? 'import' : 'all')
  })
}
