// 시작하기 화면 시험용 가짜(줄2 scripts/momso_anywhere_start.mjs 와 같은 내보내기). Google·클라우드에 닿지 않는다.
// 주소의 물음표 값으로 고른다: ?fail=<SetupError step>&at=<단계>(그 단계에서 멈춤), ?found=0|1|2(찾은 주소 수), ?findFail=1, ?slow=<ms>(단계마다, 기본 350).
// S64: ?projects=0|1|3(기존 프로젝트 수, 기본 3), ?accounts=0|1|2(열린 결제 계정 수, 기본 2), ?listFail=<SetupError step>(목록 읽기 실패), ?legacy=1(목록 함수가 없는 예전 모듈).
//   새 프로젝트인데 결제 계정을 주지 않으면 billing 단계에서 billing_selection_required. 받은 고르기는 window.__startChoice 에 남긴다(토큰은 없음).
// S65: ?redirect=1 이면 창 전체가 Google 로 갔다 오는 흉내. 시작·찾기·목록은 고른 값을 이 탭(sessionStorage)에 적고 같은 주소에 &oauth=1 을 붙여 떠난 뒤 { redirecting: true }.
//   돌아오면 resumeAfterRedirect 가 oauth=1 을 지우고 그 일을 잇는다(목록·설치·찾기·지우기). 설치는 project·billing 이 이미 끝난 체크포인트에서 이어진다.
//   지우기 시험: sessionStorage 'fake.redirect' 에 { action: 'destroy', projectId, installationId } 를 넣고 ?redirect=1&oauth=1 로 연다(subnet·network 가 pending 으로 남음).
//   ?resumeFail=<step> 이면 돌아온 일이 그 까닭으로 실패. retryFromGesture 는 마지막 일을 같은 고르기로 다시 보낸다.
// S65b: 돌아온 뒤에는 이 페이지 메모리에 '토큰'이 있는 것으로 보고(authorized) 목록·설치·찾기를 Google 에 다시 가지 않고 이 페이지에서 한다.
//   retryLastAction 은 마지막 일을 이 페이지에서 같은 고르기로 다시 하고 resumeAfterRedirect 와 같은 모양으로 준다(토큰이 없으면 Google 로).
//   window.__fakeNavigations 는 이 페이지가 떠나려 한 횟수(다음·다시 시도가 Google 에 가지 않는지 보려고).
const STEPS = ['project', 'billing', 'services', 'privateNetwork', 'server', 'gateway', 'gatewayReady']
class SetupError extends Error { constructor(step) { super(`momso anywhere setup failed at ${step}`); this.step = step } }
const q = () => new URLSearchParams(location.search)
const wait = (ms) => new Promise((r) => setTimeout(r, ms))
let attempts = 0
let authorized = false
window.__fakeNavigations = 0
const PROJECTS = [
  { projectId: 'yoga-studio-412305', displayName: '요가원 운영' },
  { projectId: 'my-first-project-88213', displayName: 'My First Project' },
  { projectId: 'momso-test-20261122', displayName: 'momso-test-20261122' },
]
const ACCOUNTS = [
  { name: 'billingAccounts/0A1B2C-3D4E5F-6A7B8C', displayName: '내 결제 계정' },
  { name: 'billingAccounts/112233-445566-778899', displayName: '요가원 법인카드' },
]
export async function startFromGesture({ clientId, projectId, billingAccountName, onProgress }) {
  if (!clientId) throw new SetupError('invalid_client')
  if (redirect && !authorized) return go('start', { projectId, billingAccountName })
  attempts += 1
  window.__startChoice = { projectId: projectId ?? null, billingAccountName: billingAccountName ?? null }
  sessionStorage.setItem('fake.retry', JSON.stringify({ action: 'start', projectId: projectId ?? null, billingAccountName: billingAccountName ?? null, installationId: null }))
  const fail = q().get('fail'); const at = q().get('at') || 'billing'; const slow = Number(q().get('slow') || 350)
  for (const step of STEPS) {
    onProgress({ step, state: 'working' })
    await wait(slow)
    if (step === 'billing' && !projectId && !billingAccountName) { onProgress({ step, state: 'failed' }); throw new SetupError('billing_selection_required') }
    // 다시 시도하면(두 번째부터) 멈추지 않는다.
    if (fail && step === at && attempts === 1) { onProgress({ step, state: 'failed' }); throw new SetupError(fail) }
    onProgress({ step, state: 'done' })
  }
  return { url: 'https://momso-any-aaaaaaaaaaaa-du.a.run.app' }
}
export async function findFromGesture() {
  if (redirect && !authorized) return go('find')
  await wait(600)
  if (q().get('findFail')) throw new SetupError('google_unavailable')
  const n = Number(q().get('found') ?? 1)
  return ['https://momso-any-aaaaaaaaaaaa-du.a.run.app', 'https://momso-any-bbbbbbbbbbbb-du.a.run.app'].slice(0, n)
}
async function projectsFromGesture(input = {}) {
  if (redirect && !authorized) return go('projects', input)
  await wait(500)
  if (q().get('listFail')) throw new SetupError(q().get('listFail'))
  return PROJECTS.slice(0, Number(q().get('projects') ?? 3))
}
async function billingAccountsFromGesture(input = {}) {
  if (redirect && !authorized) return go('billing', input)
  await wait(300)
  return ACCOUNTS.slice(0, Number(q().get('accounts') ?? 2))
}
const redirect = new URLSearchParams(location.search).has('redirect')
function go(action, extra = {}) {
  const request = { action, projectId: extra.projectId ?? null, billingAccountName: extra.billingAccountName ?? null, installationId: extra.installationId ?? null }
  sessionStorage.setItem('fake.redirect', JSON.stringify(request))
  sessionStorage.setItem('fake.retry', JSON.stringify(request))
  const url = new URL(location.href); url.searchParams.set('oauth', '1')
  window.__fakeNavigations += 1
  setTimeout(() => location.assign(url.href), 400)
  return { redirecting: true }
}
export async function resumeAfterRedirect({ onProgress }) {
  const url = new URL(location.href)
  if (url.searchParams.get('oauth') !== '1') return null
  url.searchParams.delete('oauth'); history.replaceState(null, '', url.href)
  authorized = true
  const request = JSON.parse(sessionStorage.getItem('fake.redirect') || 'null')
  sessionStorage.removeItem('fake.redirect')
  if (!request) throw new SetupError('invalid_authorization')
  const selection = { projectId: request.projectId, billingAccountName: request.billingAccountName }
  const slow = Number(q().get('slow') || 350)
  await wait(500)
  if (q().get('resumeFail')) throw new SetupError(q().get('resumeFail'))
  if (request.action === 'projects') return { kind: 'projects', projects: PROJECTS.slice(0, Number(q().get('projects') ?? 3)), selection }
  if (request.action === 'billing') return { kind: 'billing', billingAccounts: ACCOUNTS.slice(0, Number(q().get('accounts') ?? 2)), selection }
  if (request.action === 'find') return { kind: 'find', result: ['https://momso-any-aaaaaaaaaaaa-du.a.run.app'], selection }
  if (request.action === 'start') {
    window.__startChoice = selection
    for (const step of STEPS) {
      onProgress({ step, state: 'working' })
      await wait(step === 'project' || step === 'billing' ? 60 : slow)
      onProgress({ step, state: 'done' })
    }
    sessionStorage.removeItem('fake.retry')
    return { kind: 'start', result: { url: 'https://momso-any-aaaaaaaaaaaa-du.a.run.app' }, selection }
  }
  if (request.action === 'destroy') {
    for (const step of ['gateway', 'vm', 'router', 'firewall']) { onProgress({ step, state: 'working' }); await wait(slow); onProgress({ step, state: 'done' }) }
    for (const step of ['subnet', 'network']) { onProgress({ step, state: 'working' }); await wait(slow); onProgress({ step, state: 'pending' }) }
    return { kind: 'destroy', result: { remaining: ['subnet', 'network'] }, selection }
  }
  throw new SetupError('invalid_authorization')
}
export async function retryLastAction({ clientId, onProgress }) {
  const request = JSON.parse(sessionStorage.getItem('fake.retry') || 'null')
  if (!request) throw new SetupError('checkpoint_unavailable')
  if (!authorized) { go(request.action, request); return { kind: 'redirecting' } }
  const selection = { projectId: request.projectId, billingAccountName: request.billingAccountName }
  if (request.action === 'start') return { kind: 'start', result: await startFromGesture({ clientId, projectId: request.projectId ?? undefined, billingAccountName: request.billingAccountName ?? undefined, onProgress }), selection }
  if (request.action === 'projects') return { kind: 'projects', projects: await projectsFromGesture(request), selection }
  if (request.action === 'billing') return { kind: 'billing', billingAccounts: await billingAccountsFromGesture(request), selection }
  if (request.action === 'find') return { kind: 'find', result: await findFromGesture(), selection }
  throw new SetupError('invalid_authorization')
}
export function retryFromGesture() {
  const request = JSON.parse(sessionStorage.getItem('fake.retry') || 'null')
  if (!request) throw new SetupError('checkpoint_unavailable')
  return go(request.action, request)
}
const legacy = new URLSearchParams(location.search).has('legacy')
const projects = legacy ? undefined : projectsFromGesture
const billing = legacy ? undefined : billingAccountsFromGesture
export { projects as projectsFromGesture, billing as billingAccountsFromGesture }
