// 휴대폰에서 시작하기 페이지의 뒤쪽 잇기(줄2 S26, 공유 Git hq/s26-done 7fa9efbc1).
//   계약(momso-anywhere-onboarding-s26.md "화면 담당 계약"): 시작 단추를 누른 그 자리에서 startFromGesture({ google, clientId, release, onProgress }) → { url } 로 이동.
//     onProgress({ step, state: working|done|failed }), 실패는 SetupError 의 step(결제 준비·다시 시도 등을 가른다).
//   주소 찾기(momso-anywhere-pwa-entry-20261121.md "주소를 잊었을 때"): findFromGesture({ google, clientId, release }) → 입구 주소 목록(정렬된 https 출처).
//   이 함수들은 줄2 의 scripts/momso_anywhere_start.mjs(같은 폴더의 momso_anywhere_google_browser.mjs·momso_anywhere_setup.mjs 를 부름)에 있다.
//   공개 시작 페이지는 그 파일들과 같은 폴더에 둔다(로그인 페이지 momso_anywhere_login_site 와 같은 방식: ./config.json).
//   config.json: { clientId, release }. Google 접근 토큰은 시작 모듈 안 메모리에만 있다(이 파일은 받지 않는다).
// S64(momso-anywhere-start-s64.md "화면 연결 계약"): projectsFromGesture → [{ projectId, displayName }], billingAccountsFromGesture → 열린 결제 계정 [{ name, displayName }].
//   startFromGesture 에 고른 projectId(새로 만들기면 뺌)와 billingAccountName(고른 name). 기존 프로젝트에 이미 연결된 결제 계정은 모듈이 바꾸지 않는다.
//   목록은 화면 메모리에만(주소·기록·저장소에 없음). 두 목록 함수가 없는 예전 모듈이면 예전처럼 바로 시작한다(projects·billingAccounts 가 null).
// S65(momso-anywhere-start-s65.md "전체 화면 인증", 9·13줄): Google 로그인은 팝업 대신 창 전체가 Google 로 갔다 돌아온다.
//   시작·찾기·목록·지우기 함수가 Google 로 보내면 { redirecting: true } 를 돌려준다(실패가 아니다: 화면은 기다리는 모습 그대로 두고 창이 떠난다).
//   목록을 부를 때 지금 고른 값({ projectId, billingAccountName })을 넘겨야 돌아온 뒤에도 남는다.
//   화면을 열 때마다 한 번 resumeAfterRedirect({ clientId, release, onProgress }) → null(이어 갈 것 없음) | { kind: 'projects', projects, selection }
//   | { kind: 'billing', billingAccounts, selection } | { kind: 'start'|'find'|'destroy', result, selection } | { kind: 'redirecting' }(토큰이 만료돼 다시 Google 로).
//   selection = { projectId, billingAccountName }(고르지 않은 값은 null). 지우기 결과는 { remaining: [이름…] }, 단계 상태에 pending(나중에 다시 해야 함)이 더해졌다.
//   다른 오류 뒤 다시 시도는 retryFromGesture({ clientId })(같은 일을 같은 고르기로 다시 Google 로).
//   예전 팝업 모듈을 위해 Google 로그인 스크립트도 미리 읽어 두지만, 읽지 못해도 페이지는 연다(S65 모듈은 쓰지 않는다).
// S65b(같은 문서 "인증 한 번으로 이어 쓰기", 줄2 363b6c07f): 돌아온 토큰을 모듈 메모리에 두고 목록·설치·찾기·지우기에 다시 쓴다(Google 에 다시 가지 않음).
//   오류 뒤 다시 시도는 retryLastAction({ clientId, release, onProgress }) → resumeAfterRedirect 와 같은 모양의 결과(또는 { kind: 'redirecting' }).
//   retryLastAction 이 없는 모듈이면 예전처럼 retryFromGesture(언제나 Google 로).

export type StartProgress = { step: string; state: 'working' | 'done' | 'failed' | 'pending' | string }
export type StartProject = { projectId: string; displayName: string }
export type StartBillingAccount = { name: string; displayName: string }
export type StartChoice = { projectId?: string; billingAccountName?: string }
export type StartSelection = { projectId: string | null; billingAccountName: string | null }
/** Google 로 보내는 중(창이 곧 떠난다). */
export const REDIRECTING = 'redirecting' as const
export type StartResume =
  | { kind: 'projects'; projects: StartProject[]; selection: StartSelection }
  | { kind: 'billing'; accounts: StartBillingAccount[]; selection: StartSelection }
  | { kind: 'start'; url: string | null; selection: StartSelection }
  | { kind: 'find'; urls: string[] }
  | { kind: 'destroy'; remaining: string[]; selection: StartSelection }
  | { kind: 'redirecting' }

type Base = { google: unknown; clientId: string; release: unknown }
type StartModule = {
  startFromGesture: (input: Base & StartChoice & { onProgress: (progress: StartProgress) => void }) => Promise<unknown>
  findFromGesture: (input: Base) => Promise<unknown>
  projectsFromGesture?: (input: Base & StartSelection) => Promise<unknown>
  billingAccountsFromGesture?: (input: Base & StartSelection) => Promise<unknown>
  resumeAfterRedirect?: (input: { clientId: string; release: unknown; onProgress: (progress: StartProgress) => void }) => Promise<unknown>
  retryFromGesture?: (input: { clientId: string }) => unknown
  retryLastAction?: (input: { clientId: string; release: unknown; onProgress: (progress: StartProgress) => void }) => Promise<unknown>
}
export type StartKit = {
  start: (choice: StartChoice, onProgress: (progress: StartProgress) => void) => Promise<{ url: string | null } | typeof REDIRECTING>
  find: () => Promise<string[] | typeof REDIRECTING>
  projects: ((selection: StartSelection) => Promise<StartProject[] | typeof REDIRECTING>) | null
  billingAccounts: ((selection: StartSelection) => Promise<StartBillingAccount[] | typeof REDIRECTING>) | null
  /** 화면을 열 때 한 번. 이어 갈 것이 없거나 예전 모듈이면 null. */
  resume: (onProgress: (progress: StartProgress) => void) => Promise<StartResume | null>
  /** 멈춘 일을 같은 고르기로 다시. 결과는 resume 과 같은 모양(다시 할 기록이 없으면 null). 예전 모듈이면 null. */
  retry: ((onProgress: (progress: StartProgress) => void) => Promise<StartResume | null>) | null
  /** retry 가 이 페이지 안에서 다시 하는가(S65b retryLastAction). 아니면 Google 에서 돌아온 뒤에만 쓴다. */
  retryInPage: boolean
}

const GOOGLE_SCRIPT = 'https://accounts.google.com/gsi/client'
type GoogleWindow = Window & { google?: { accounts?: { oauth2?: unknown } } }

function loadGoogle() {
  const win = window as GoogleWindow
  if (win.google?.accounts?.oauth2) return Promise.resolve(win.google)
  return new Promise<unknown>((resolve, reject) => {
    if (!document.querySelector(`script[src="${GOOGLE_SCRIPT}"]`)) {
      const script = document.createElement('script')
      script.src = GOOGLE_SCRIPT
      script.async = true
      document.head.append(script)
    }
    let tries = 0
    const timer = window.setInterval(() => {
      tries += 1
      if (win.google?.accounts?.oauth2) { window.clearInterval(timer); resolve(win.google) }
      else if (tries > 100) { window.clearInterval(timer); reject(new Error('google_unavailable')) }
    }, 100)
  })
}

const text = (value: unknown) => (typeof value === 'string' ? value : '')
const isRedirect = (value: unknown) => Boolean(value && typeof value === 'object' && (value as { redirecting?: unknown }).redirecting === true)
function readProjects(value: unknown): StartProject[] {
  return Array.isArray(value) ? value.flatMap((item) => { const p = (item ?? {}) as Record<string, unknown>; return text(p.projectId) ? [{ projectId: text(p.projectId), displayName: text(p.displayName) }] : [] }) : []
}
function readAccounts(value: unknown): StartBillingAccount[] {
  return Array.isArray(value) ? value.flatMap((item) => { const p = (item ?? {}) as Record<string, unknown>; return text(p.name) ? [{ name: text(p.name), displayName: text(p.displayName) }] : [] }) : []
}
function readUrls(value: unknown) {
  return Array.isArray(value) ? value.filter((url): url is string => typeof url === 'string' && /^https:\/\//.test(url)) : []
}
function readSelection(value: unknown): StartSelection {
  const p = (value ?? {}) as Record<string, unknown>
  return { projectId: text(p.projectId) || null, billingAccountName: text(p.billingAccountName) || null }
}
function readUrl(value: unknown) {
  const url = text((value as { url?: unknown } | null)?.url)
  return url || null
}

/** 모듈이 돌려준 resume·retry 결과 → 화면 모양. 모르는 값이면 null. */
function readResume(value: Record<string, unknown> | null): StartResume | null {
  if (!value) return null
  const selection = readSelection(value.selection)
  switch (value.kind) {
    case 'projects': return { kind: 'projects', projects: readProjects(value.projects), selection }
    case 'billing': return { kind: 'billing', accounts: readAccounts(value.billingAccounts), selection }
    case 'start': return { kind: 'start', url: readUrl(value.result), selection }
    case 'find': return { kind: 'find', urls: readUrls(value.result) }
    case 'destroy': {
      const remaining = (value.result as { remaining?: unknown } | null)?.remaining
      return { kind: 'destroy', remaining: Array.isArray(remaining) ? remaining.filter((item): item is string => typeof item === 'string') : [], selection }
    }
    case 'redirecting': return { kind: 'redirecting' }
    default: return null
  }
}

/** 화면을 열 때 한 번: 설정·시작 모듈(·예전 모듈용 Google 로그인 스크립트). 설정이나 모듈을 못 읽으면 실패. */
export async function loadStart(): Promise<StartKit> {
  const config = await (await fetch('./config.json', { cache: 'no-store' })).json() as { clientId?: unknown; release?: unknown }
  if (typeof config.clientId !== 'string' || !config.clientId) throw new Error('config_unavailable')
  const clientId = config.clientId
  const release = config.release ?? null
  const [google, module] = await Promise.all([
    loadGoogle().catch(() => null),
    import(/* @vite-ignore */ new URL('./momso_anywhere_start.mjs', window.location.href).href) as Promise<StartModule>,
  ])
  if (typeof module.startFromGesture !== 'function' || typeof module.findFromGesture !== 'function') throw new Error('start_unavailable')
  const base = { google, clientId, release }
  const listProjects = module.projectsFromGesture
  const listAccounts = module.billingAccountsFromGesture
  const resumeModule = module.resumeAfterRedirect
  const retryModule = module.retryFromGesture
  const retryLast = module.retryLastAction
  return {
    start: async (choice, onProgress) => {
      const value = await module.startFromGesture({
        ...base,
        ...(choice.projectId ? { projectId: choice.projectId } : {}),
        ...(choice.billingAccountName ? { billingAccountName: choice.billingAccountName } : {}),
        onProgress,
      })
      return isRedirect(value) ? REDIRECTING : { url: readUrl(value) }
    },
    find: async () => { const value = await module.findFromGesture(base); return isRedirect(value) ? REDIRECTING : readUrls(value) },
    projects: typeof listProjects === 'function' ? async (selection) => { const value = await listProjects({ ...base, ...selection }); return isRedirect(value) ? REDIRECTING : readProjects(value) } : null,
    billingAccounts: typeof listAccounts === 'function' ? async (selection) => { const value = await listAccounts({ ...base, ...selection }); return isRedirect(value) ? REDIRECTING : readAccounts(value) } : null,
    resume: async (onProgress) => {
      if (typeof resumeModule !== 'function') return null
      return readResume(await resumeModule({ clientId, release, onProgress }) as Record<string, unknown> | null)
    },
    retry: typeof retryLast === 'function'
      ? async (onProgress) => readResume(await retryLast({ clientId, release, onProgress }) as Record<string, unknown> | null)
      : typeof retryModule === 'function'
        ? async () => {
            try { const value = retryModule({ clientId }); if (value instanceof Promise) value.catch(() => {}); return { kind: 'redirecting' } } catch { return null }
          }
        : null,
    retryInPage: typeof retryLast === 'function',
  }
}

/** 실패의 step 값(SetupError). 없으면 null. */
export function setupErrorStep(error: unknown) {
  const step = (error as { step?: unknown } | null)?.step
  return typeof step === 'string' ? step : null
}
