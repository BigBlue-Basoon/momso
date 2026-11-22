// 화면 낱말 파일(SCREEN_RULES 3절). 화면에 보이는 글은 모두 여기에 있다. 한글판(ko)과 영어판(en)을 나란히 둔다.
// 지금 앱은 한글판(W = WORDS.ko)을 쓴다. 영어판은 아직 채우지 않는다(나중에 같은 열쇠에 영어를 넣으면 된다).
//
// - 한글로 옮기는 말은 김성균 개발자가 정한 것만 적는다(SCREEN_RULES 3절 표). 표에 없는 회사 표기는 그대로 둔다.
// - 백엔드가 주는 영어 값(id)은 열쇠로 쓰고, 화면은 이 파일에서 그 열쇠의 글을 찾아 보인다(permission, effort).
// - 아래 "화면마다" 칸은 그 글이 쓰이는 소스 파일별로 묶었다. 값이 함수인 것은 ${a} 자리에 이름·숫자 같은 값이 들어간다.
// - 한 곳만 고치면 그 글이 쓰이는 화면 전체가 바뀐다. 화면에 보이는 글을 코드에 새로 박지 않는다.
// - 김성균 개발자가 보는 목록: momso-public-prep/ui-words-ko.md (이 파일에서 뽑았다).

/**
 * 앞 낱말의 끝에 받침이 있으면 첫째, 없으면 둘째 조사(을/를). 값이 바뀌어도 문장이 맞게.
 * 끝이 숫자면 한국어로 읽은 소리로 본다(영·일·삼·육·칠·팔은 받침). '으로/로'는 ㄹ 받침이면 '로'(2026-11-18: "메모 56를", "홍보 글으로"를 고침).
 */
const DIGIT_FINAL = [21, 8, 0, 16, 0, 0, 1, 8, 8, 0] // 영(ㅇ) 일(ㄹ) 이 삼(ㅁ) 사 오 육(ㄱ) 칠(ㄹ) 팔(ㄹ) 구
function josa(word: unknown, withFinal: string, without: string) {
  const text = String(word).trim()
  const last = text.charAt(text.length - 1)
  let final = 0
  if (last >= '0' && last <= '9') final = DIGIT_FINAL[Number(last)]
  else {
    const code = last.charCodeAt(0)
    if (code < 0xac00 || code > 0xd7a3) return without
    final = (code - 0xac00) % 28
  }
  if (!final) return without
  if (withFinal === '으로' && final === 8) return without
  return withFinal
}

const ko = {
  // ---- 늘 같은 것을 한 낱말로 부르는 말 ----
  chatRoom: '채팅방',
  document: '문서',
  rawMaterial: '원자료',
  workLog: '작업 내역',
  appFolder: '몸소 폴더',
  /** 채팅방을 모으는 것. 2026-11-16 김성균 개발자가 정함("저거 그냥 음 프로젝트라고 하자"). */
  group: '프로젝트',
  search: '검색',
  /** AI 회사 이름(열쇠는 백엔드의 provider 값). */
  provider: { codex: '코덱스', claude: '클로드', gemini: '제미나이' } as Record<'codex' | 'claude' | 'gemini', string>,
  /** 모델 계열 이름(열쇠는 소문자 영어 계열 이름). 모델 이름을 짜 맞출 때 이 낱말로 바꾼다. */
  family: { fable: '페이블', opus: '오푸스', sonnet: '소넷', haiku: '하이쿠', terra: '테라', sol: '솔', luna: '루나', astra: '아스트라' } as Record<string, string>,
  /** 단추 풍선. 이름표보다 하는 일이 드러나는 한국어 프로그램의 표준 문구. */
  tip: { home: '대화하기', documents: '문서', media: '사진과 동영상', settings: '설정', back: '뒤로 가기', forward: '앞으로 가기' },

  // ---- 권한 단계(열쇠: '회사:백엔드 id'). 회사 앱의 공식 한국어(김성균 개발자가 찍어 준 사진). 없는 단계는 회사 원문 그대로. ----
  permission: {
    // 코덱스(Codex 앱 권한 메뉴). "Full access" 는 Codex 앱 단추에는 "전체 액세스"지만 몸소는 메뉴와 단추 모두 "전체 권한"(본부가 고름).
    'codex:Ask for approval': { label: '승인 요청', description: '외부 파일 수정 및 인터넷 사용 시 항상 확인합니다' },
    'codex:Approve for me': { label: '나 대신 승인', description: '잠재적으로 위험한 것으로 감지된 작업에만 승인을 요청합니다' },
    'codex:Full access': { label: '전체 권한', description: '인터넷과 컴퓨터의 모든 파일에 제한 없이 액세스합니다' },
    // 클로드(Claude 앱 "모드" 창). 백엔드 id 와 짝: default=Manual, acceptEdits, plan, auto, bypassPermissions.
    'claude:auto': { label: '자동', description: '클로드가 권한 결정을 처리합니다' },
    'claude:default': { label: '수동', description: '변경하기 전에 항상 확인' },
    'claude:acceptEdits': { label: '편집 자동 수락', description: '모든 파일 편집을 자동으로 수락' },
    'claude:plan': { label: '계획', description: '변경하기 전에 계획 세우기' },
    'claude:bypassPermissions': { label: '권한 무시', description: '모든 권한 허용' },
  } as Record<string, { label: string; description?: string | null }>,

  // ---- 추론 수준(열쇠: '회사:백엔드 id', 메뉴 맨 위 설명은 '회사:note'). ----
  effort: {
    // 클로드(Claude 앱 노력 창의 공식 한국어).
    'claude:low': { label: '낮음' },
    'claude:medium': { label: '중간' },
    'claude:high': { label: '높음' },
    'claude:xhigh': { label: '엑스트라' },
    'claude:max': { label: '최대' },
    'claude:note': { label: '더 높은 노력은 더 철저한 응답을 의미하지만, 시간이 더 오래 걸리고 한도를 더 빨리 사용합니다.' },
    // 맨 위 특별한 수준: 2026-11-16 김성균 개발자가 정함("울트라 혹은 울트라코드 이거는 llm회사에 따라 다르게").
    // 그 회사가 부르는 이름을 한글로: 코덱스는 추론 수준 값 ultra, 클로드는 켜고 끄는 항목 Ultracode(ai_models 의 toggles).
    'codex:ultra': { label: '울트라' },
    'claude:ultracode': { label: '울트라코드' },
  } as Record<string, { label: string; description?: string | null }>,
  /** 추론 수준 값(백엔드 id)의 한글 — 회사 표가 없을 때. 본부 임시안(Claude 앱 공식 한국어에 맞춤, 그의 확인 전). ultra 는 회사마다 이름이 달라 위 회사 표에 둔다. */
  effortName: { none: '없음', minimal: '최소', low: '낮음', medium: '중간', high: '높음', xhigh: '엑스트라', max: '최대' } as Record<string, string>,

  // ================= 화면마다(소스 파일별) =================
  // ---- 문서 본문 안 화자 표시 ----
  zetMarkdown: {
    /** 이름 앞 ? 표시에 올렸을 때(AI 가 스스로 가린 화자). 새로 쓴 글, 그의 확인 전. */
    guessedSpeaker: '추정한 화자',
  },
  // ---- 설정 "연결과 백업"(2026-11-16 결정: GitHub = 문서와 고친 이력, 오브젝트 스토리지 = 원자료, DB 없음). 새로 쓴 글은 본부·그의 확인 전. ----
  // 2026-11-16 그의 결정("어 그래 깃허브에 채팅방올려 안될 이유 없잖아."): 채팅방도 GitHub 비공개 저장소에 올라간다. 두 글에 채팅방을 넣음.
  backup: {
    pageIntro: '문서, 채팅방, 수정 이력은 GitHub에, 원자료는 선택한 오브젝트 스토리지에 백업합니다.',
    githubTitle: 'GitHub',
    githubWhat: '문서, 채팅방, 수정 이력',
    /** GitHub 연결 때의 알림: 방향 문서 L243 의 글 그대로. */
    /** 2026-11-16 새로 씀(그의 확인 전): Git 이 없어 GitHub 백업이 막혔을 때 그 자리의 한 줄과 설치(git_install). */
    gitNeeded: 'GitHub에 업로드하려면 Git이라는 프로그램이 필요합니다.',
    gitInstallTitle: 'Git을 설치할까요?',
    gitInstallText: 'GitHub에 업로드하려면 필요한 프로그램입니다. 공식 배포판을 다운로드해 확인한 뒤 몸소 안에 설치합니다. 몇 분 걸릴 수 있습니다.',
    gitInstallFailed: '설치하지 못했습니다. 다시 시도해 주세요.',
    /** 2026-11-17 사용자 이름을 읽는 동안·읽지 못했을 때(본부 결정, 그가 보고 고친다). */
    usernameLoading: '사용자 이름 불러오는 중',
    usernameFailed: '사용자 이름을 읽지 못했습니다',
    // 2026-11-17 처음 켠 사람 눈으로 돌다 찾음: 읽는 2초 동안과 읽지 못했을 때 글이 없었다. 새로 씀, 그의 확인 전.
    githubLoading: 'GitHub 연결 불러오는 중',
    githubFailed: 'GitHub 연결을 읽지 못했습니다',
    storeFailed: '오브젝트 스토리지 연결을 읽지 못했습니다',
    gitInstallReason: {
      networkFailed: '다운로드하지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요.',
      // 2026-11-17 받은 Git 파일의 지문이 다를 때(git-install-changed stage checksumMismatch). 새로 씀, 그의 확인 전.
      checksumMismatch: '받은 파일이 원래 파일과 달라 설치하지 않았습니다. 다시 시도해 주세요.',
    } as Record<string, string>,
    // 2026-11-16 본부: 그가 채팅방도 올리기로 정했으므로 그의 글 꼴을 지켜 "채팅방"만 더함(본부가 그에게 알림).
    githubNotice: '문서와 채팅방이 내 GitHub 비공개 저장소에 업로드됩니다.',
    repoLabel: '저장소',
    connect: '연결',
    push: '업로드',
    pull: '다운로드',
    disconnect: '연결 해제',
    account: 'GitHub 계정',
    lastPushed: '마지막으로 올린 때',
    ahead: (a: unknown) => `업로드할 것 ${a}`,
    behind: (a: unknown) => `다운로드할 것 ${a}`,
    /** 탐색기에서 몸소 폴더에 따로 넣은 일반 파일 수(2026-11-19 첫째 줄 M17 archive_additional_files_count). GitHub 업로드할 것 곁에. 새로 씀, 그의 확인 전. */
    additionalFiles: (a: unknown) => `따로 넣은 파일 ${a}개`,
    githubDisconnectTitle: 'GitHub 연결을 해제할까요?',
    githubDisconnectText: 'GitHub의 저장소와 문서는 지워지지 않습니다.',
    storeTitle: '오브젝트 스토리지',
    /** 제목 "오브젝트 스토리지" 아래 한 줄(2026-11-19 G81 8: 낱말은 그 회사 화면에서 찾는 말이라 두고 처음 나오는 자리에 풀이). 새로 씀, 그의 확인 전. */
    storeWhat: '원자료를 보관하는 인터넷 저장 공간입니다',
    /** 버킷 칸 아래 풀이(G81 8). 새로 씀, 그의 확인 전. */
    bucketHelp: '오브젝트 스토리지 안에서 원자료를 담는 곳의 이름입니다',
    provider: '회사',
    region: '지역',
    bucket: '버킷',
    endpoint: '주소',
    accessKey: '액세스 키',
    secretKey: '비밀 키',
    lastBackup: '마지막 백업',
    pending: (a: unknown, b: unknown) => `아직 업로드하지 않은 원자료 ${a}개 · ${b}`,
    backupNow: '지금 백업',
    restore: '백업에서 복원',
    storeDisconnectTitle: '오브젝트 스토리지 연결을 해제할까요?',
    storeDisconnectText: '스토리지에 올린 원자료는 지워지지 않습니다.',
    restoreTitle: '백업에서 복원할까요?',
    restoreText: '앱의 목록을 마지막 백업 때로 되돌리고, 이 기기에 없는 원자료를 받아옵니다. 되돌리기 전에 지금 상태를 따로 남겨 둡니다.',
    cancel: '취소',
    done: '끝났습니다',
    failed: '하지 못했습니다',
    /** GitHub 자동 백업(2026-11-18). 새로 씀, 그의 확인 전. */
    autoBackup: '자동 백업',
    /** 원자료 자동 업로드(2026-11-18 넷째 줄 D38). 형편 줄은 GitHub 자동 백업과 같은 글을 쓴다. 새로 씀, 그의 확인 전. */
    autoUpload: '자동 업로드',
    storeAutoReason: {
      interrupted: '업로드가 중간에 멈췄습니다',
      local_read_failed: '이 컴퓨터의 원자료를 읽지 못했습니다',
      local_state_unavailable: '업로드 기록을 읽지 못했습니다',
      remote_unavailable: '저장소에 연결하지 못했습니다. 저장소 연결과 키를 확인해 주세요',
      first_receive_in_progress: 'GitHub에서 처음 받아 오는 중이라 끝난 뒤에 업로드합니다',
    } as Record<string, string>,
    /** 원자료 자동 업로드에 글을 정하지 않은 까닭이 오면(2026-11-19: 영어 값이나 "하지 못했습니다"만 보이지 않게). 새로 씀, 그의 확인 전. */
    storeAutoUnknown: '원자료를 업로드하는 중 문제가 생겼습니다',
    autoPending: '바뀐 내용을 곧 업로드합니다',
    autoRunning: '자동으로 업로드하는 중',
    autoRetry: (a: unknown) => `업로드하지 못해 ${a}분 뒤에 다시 시도합니다`,
    autoRetrySoon: '업로드하지 못해 곧 다시 시도합니다',
    /** GitHub 칸: 글을 정해 두지 않은 까닭이 오면 그 값을 그대로 붙인다(무엇이 안 됐는지 알 수 있게). 2026-11-18 새로 씀, 그의 확인 전. */
    /** 글을 정하지 않은 까닭이 오면(2026-11-19 본부: 영어 값을 보이지 않는다). 새로 씀, 그의 확인 전. */
    unknownReason: 'GitHub 연결에 문제가 있습니다. 다시 시도해 주세요',
    /** 뒤쪽 상태 값 → 쉬운 말(실패 알림). */
    /** 원자료 저장소 회사 이름: 영어 그대로(2026-11-16 김성균 개발자가 정함, "저거는 그냥 AWS GCP 이렇게 적고"). */
    storeName: { aws: 'AWS', gcp: 'GCP', cloudflare: 'Cloudflare', ncp: 'NCP' } as Record<'aws' | 'gcp' | 'cloudflare' | 'ncp', string>,
    reason: {
      git_missing: 'Git이 필요합니다', gh_missing: 'GitHub 연결 도구가 필요합니다', signed_out: 'GitHub에 로그인해 주세요',
      invalid_repository: '저장소 이름을 확인해 주세요', repository_unavailable: '저장소를 찾지 못했습니다', repository_not_private: '비공개 저장소만 연결할 수 있습니다',
      remote_ahead: 'GitHub에 더 새 내용이 있습니다. 먼저 받아 주세요',
      // 2026-11-17: 풀지 못한 것이 남아도 이 PC 의 일은 계속된다(본부 지시: "멈췄다"가 아니라 몇 개가 남았는지). 수를 모를 때의 글. 새로 씀, 그의 확인 전.
      diverged: 'GitHub와 합치지 못한 파일이 남았습니다. 이 PC에서는 계속 쓸 수 있습니다',
      unsaved_changes: '저장하지 않은 문서가 있습니다. 먼저 저장해 주세요', nothing_saved: '업로드할 저장 내용이 없습니다', busy: '다른 작업이 끝난 뒤에 다시 해 주세요',
      invalid_credentials: '키를 확인해 주세요', bucket_not_found: '버킷을 찾지 못했습니다', access_denied: '이 키로는 버킷에 쓸 수 없습니다', write_denied: '이 키로는 버킷에 쓸 수 없습니다',
      invalid_endpoint: '주소를 확인해 주세요', invalid_region: '지역을 확인해 주세요', invalid_bucket: '버킷 이름을 확인해 주세요',
      recovery_key_unconfirmed: '먼저 복구 키를 만들고 보관을 확인해 주세요', recovery_key_missing: '먼저 복구 키를 만들고 보관을 확인해 주세요',
      file_too_large: '너무 큰 원자료가 있어 멈췄습니다',
      // 백업에서 되살리기 실패(app_state_restore, 2026-11-17 새로 씀, 그의 확인 전).
      restore_failed: '백업에서 복원하지 못했습니다. 지금 상태는 그대로입니다',
      restore_rollback_failed: '백업에서 복원하지 못했고 원래 상태로도 돌려놓지 못했습니다. 복원 전에 따로 남겨 둔 사본이 있습니다',
      // GitHub 에 못 올린 까닭(2026-11-17 뒤쪽 desktop-github-b29-errors.md). 까닭마다 할 일을 가른 글. 새로 씀, 그의 확인 전.
      authentication_failed: 'GitHub 로그인이 풀렸습니다. 다시 로그인해 주세요',
      repository_unavailable_or_denied: '저장소를 찾을 수 없거나 접근할 수 없습니다. 저장소 주소와 권한을 확인하고 다시 연결해 주세요',
      permission_denied: '이 저장소에 올릴 권한이 없습니다. 저장소의 쓰기 권한을 확인해 주세요',
      connection_failed: 'GitHub에 연결하지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요',
      size_limit: 'GitHub의 크기 한도에 걸려 올리지 못했습니다. 큰 파일이 있는지 확인해 주세요',
      first_receive_interrupted: '처음 받아 오기가 중간에 끊겼습니다. 새로 받은 것과 원래 있던 것을 지우지 말고 확인해 주세요',
      authentication_unavailable: 'GitHub 연결과 계정 상태를 확인하고 다시 시도해 주세요',
      account_unavailable: 'GitHub 연결과 계정 상태를 확인하고 다시 시도해 주세요',
      remote_unavailable: 'GitHub 연결과 계정 상태를 확인하고 다시 시도해 주세요',
      // GitHub 상태·연결에서 뒤쪽이 주는데 글이 없던 까닭(archive_remote.rs, 2026-11-18). 새로 씀, 그의 확인 전.
      local_state_unavailable: '몸소 폴더의 상태를 읽지 못했습니다. 다시 시도해 주세요',
      git_unavailable: 'Git을 실행하지 못했습니다. 다시 시도해 주세요',
      unsafe_git_configuration: '이 PC의 Git 설정이 GitHub 주소를 바꾸고 있어 멈췄습니다. Git 설정을 확인해 주세요',
      unsafe_history: '기록에 확인할 수 없는 부분이 있어 멈췄습니다',
      /** 2026-11-18 넷째 줄 D39: 아직 올리지 않은 저장 기록에 백업에서 빼는 파일이 있어 멈춤. 본부 결정: 선생님께 파일을 고치게 하지 않는다
          (몸소가 스스로 정리하도록 넷째 줄 D41). 화면은 [다시 시도]를 함께 둔다. 새로 씀, 그의 확인 전. */
      ignored_history: '백업에서 빼기로 한 파일 때문에 업로드를 멈췄습니다',
      /** 몸소가 그 파일을 기록에서 정리하는 중(D41 의 상태 값이 오면 잇는다). */
      ignored_history_cleaning: '백업에서 빼기로 한 파일을 기록에서 정리하고 다시 업로드합니다',
      // 2026-11-19 본부(여섯째 줄이 실제 앱에서 찾음): GitHub 줄에 영어 값(not_connected 등)이 그대로 보였다. 뒤쪽 archive_remote.rs 가 주는데
      // 글이 없던 값과 넷째 줄 desktop-document-folders.md 의 새 멈춤 까닭. 새로 씀, 그의 확인 전.
      not_connected: 'GitHub가 아직 연결되지 않았습니다',
      auto_backup_disabled: '자동 백업이 꺼져 있습니다',
      redaction_pending: '채팅방 사본에서 키를 가리는 중이라 업로드를 미뤘습니다',
      unsaved_skill_changes: '바뀐 스킬을 저장하지 못해 업로드를 멈췄습니다',
      local_save_failed: '이 PC에서 문서를 저장하지 못해 업로드를 멈췄습니다',
      first_receive_conflict: 'GitHub에서 처음 받아 온 내용과 이 PC의 내용이 달라 확인이 필요합니다',
      ignored_history_published: '백업에서 빼기로 한 파일이 이미 GitHub에 올라가 있어 업로드를 멈췄습니다',
      ignored_history_unverified: 'GitHub에 이미 올라간 기록인지 확인하지 못해 업로드를 멈췄습니다. 다시 시도해 주세요',
      ignored_history_locked: '다른 Git 작업이 끝나지 않아 업로드를 멈췄습니다. 잠시 뒤 다시 시도해 주세요',
      redaction_required: '기록에 가려야 할 값이 있어 업로드하지 않았습니다',
      first_receive_in_progress: 'GitHub에서 처음 받아 오는 중입니다. 끝난 뒤에 다시 해 주세요',
      merge_report_unavailable: '합친 결과를 확인하지 못했습니다. 다시 시도해 주세요',
      reindex_failed: '받은 문서를 목록에 반영하지 못했습니다. 다시 시도해 주세요',
      credentials_unavailable: '저장된 키를 읽지 못해 멈췄습니다. 다시 시도해 주세요',
      fast_forward_failed: 'GitHub의 새 내용을 반영하지 못했습니다. 다시 시도해 주세요',
      invalid_branch: '이 저장소의 기본 브랜치 이름은 쓸 수 없습니다',
      unavailable: 'GitHub 상태를 확인하지 못했습니다. 다시 시도해 주세요',
      // 원자료 버킷 자동 만들기(2026-11-16 새로 쓴 글)
      username_required: '사용자 이름이 없어 버킷 이름을 정하지 못했습니다. 버킷 이름을 적어 주세요',
      invalid_username_for_bucket: '사용자 이름으로는 버킷 이름을 만들 수 없습니다. 버킷 이름을 적어 주세요',
      automatic_provider_unsupported: '이 회사에서는 버킷을 만들 수 없습니다. 버킷 이름을 적어 주세요',
      bucket_name_taken: '같은 이름의 버킷이 이미 다른 곳에 있습니다. 버킷 이름을 적어 주세요',
      bucket_list_denied: '이 키로는 버킷 목록을 볼 수 없습니다. 키 권한을 확인해 주세요',
      bucket_list_failed: '버킷 목록을 읽지 못했습니다',
      bucket_lookup_failed: '버킷을 확인하지 못했습니다',
      bucket_create_denied: '이 키로는 버킷을 만들 수 없습니다. 키 권한을 확인해 주세요',
      bucket_create_invalid: '버킷을 만들 수 없는 이름입니다. 버킷 이름을 적어 주세요',
      bucket_create_failed: '버킷을 만들지 못했습니다',
      // 2026-11-17 뒤쪽이 더한 두 값(R2 버킷 만들기). 새로 씀, 그의 확인 전.
      bucket_create_unknown: '버킷이 만들어졌는지 확인하지 못했습니다. 잠시 뒤 다시 연결해 주세요',
      created_not_connected: '버킷은 만들어졌습니다. 다시 연결하면 이어집니다',
      bucket_privacy_unverified: '버킷이 비공개인지 확인하지 못해 연결하지 않았습니다',
      gcp_project_required: '버킷 이름을 적어 주세요', gcp_oauth_required: '버킷 이름을 적어 주세요', gcp_oauth_invalid: '버킷 이름을 적어 주세요',
    } as Record<string, string>,
    /** 버킷 칸을 비워 두었을 때 그 아래 한 줄. */
    autoBucket: (a: unknown) => `비워 두면 ${a} 버킷을 만들어 연결합니다.`,
    /** GitHub 저장소 만들기(archive_remote_create). 2026-11-16 새로 쓴 글. */
    /** 채팅방 사본 쓰기가 실패했을 때(chat-archive-changed 의 reason). GitHub 까닭 줄 자리. 2026-11-17 새로 씀, 그의 확인 전. */
    chatCopy: {
      archiveMissing: '몸소 폴더를 찾지 못해 채팅방 사본을 쓰지 못했습니다. 설정의 저장공간에서 몸소 폴더를 확인해 주세요',
      notGitRepository: '몸소 폴더의 백업 기록을 찾지 못해 채팅방 사본을 쓰지 못했습니다',
      permissionDenied: '몸소 폴더에 쓸 권한이 없어 채팅방 사본을 쓰지 못했습니다',
      pathTooLong: '몸소 폴더 위치가 너무 길어 채팅방 사본을 쓰지 못했습니다. 더 짧은 위치로 옮겨 주세요',
      diskError: '디스크 문제로 채팅방 사본을 쓰지 못했습니다. 저장 공간을 확인해 주세요',
      redactionFailed: '비밀값을 가리지 못해 채팅방 사본을 쓰지 않았습니다',
      credentialStoreUnavailable: '저장된 키를 읽지 못해 비밀값을 가릴 수 없어 채팅방 사본을 쓰지 않았습니다',
      other: '채팅방 사본을 쓰지 못했습니다',
    } as Record<string, string>,
    chatCopyRetry: '업로드할 때 다시 시도합니다',
    /** 옛 키 다시 가리기(2026-11-17). 새로 씀, 그의 확인 전. */
    redactionWait: '바꾼 키를 채팅방 사본에서 가리는 중입니다. 끝나면 업로드합니다',
    keyLeftWarning: '예전에 업로드한 채팅 기록에 키가 남아 있을 수 있습니다. 그 키를 발급한 곳에서 새 키를 받아 넣어 주세요',
    chatCopyPending: (a: unknown) => `채팅방 사본 ${a}개를 아직 쓰지 못했습니다. 업로드할 때 다시 시도합니다`,
    /** 두 PC 에서 고쳤을 때(2026-11-17). 새로 씀, 그의 확인 전. */
    conflictsLeft: (a: unknown) => `GitHub와 합치지 못한 파일이 ${a}개 남았습니다. 이 PC에서는 계속 쓸 수 있습니다`,
    preservedOne: (a: unknown) => `“${a}” 문서는 다른 PC에서도 고쳐져 이 PC의 것을 이전 버전으로 남겼습니다`,
    preservedMany: (a: unknown, b: unknown) => `문서 ${a}개는 다른 PC에서도 고쳐져 이 PC의 것을 이전 버전으로 남겼습니다: ${b}`,
    createRepo: '저장소 만들기',
    createWhat: (a: unknown) => `GitHub에 ${a} 비공개 저장소를 만들어 연결합니다.`,
    createNeedsName: '사용자 이름이 있어야 저장소를 만들 수 있습니다.',
    createTitle: (a: unknown) => `GitHub에 ${a} 저장소를 만들까요?`,
    createText: '나만 볼 수 있는 비공개 저장소로 만들고 바로 연결합니다.',
    create: {
      created: '저장소를 만들고 연결했습니다',
      created_not_connected: '저장소는 만들었지만 연결하지 못했습니다. 아래 칸에 저장소를 적어 연결해 주세요',
      created_unverified: '저장소는 만들었지만 비공개인지 확인하지 못해 연결하지 않았습니다',
      create_failed: '저장소를 만들지 못했습니다',
      username_missing: '사용자 이름이 있어야 저장소를 만들 수 있습니다',
      invalid_username: '사용자 이름으로는 저장소를 만들 수 없습니다',
      already_connected: '이미 저장소가 연결되어 있습니다',
      gh_missing: 'GitHub 연결 도구가 필요합니다', signed_out: 'GitHub에 로그인해 주세요', git_missing: 'Git이 필요합니다', unavailable: '하지 못했습니다',
      exists_public: '같은 이름의 공개 저장소가 있어 만들지 않았습니다. 비공개 저장소만 연결할 수 있습니다',
      exists_empty: '같은 이름의 빈 저장소가 이미 있습니다. 아래에서 연결을 눌러 주세요',
      exists_matching: '같은 이름의 저장소가 이미 있고 이 기기의 이력과 이어집니다. 아래에서 연결을 눌러 주세요',
      exists_private_unknown: '같은 이름의 비공개 저장소가 이미 있습니다. 내 저장소가 맞으면 아래에서 연결을 눌러 주세요',
    } as Record<string, string>,
  },
  // ---- 설정의 AI 로그인(코덱스·클로드·제미나이). 2026-11-16 새로 쓴 글(본부·그의 확인 전, 낱말 목록에 올림). ----
  signIn: {
    intro: '쓰려는 AI에 계정으로 로그인하거나 API 키를 넣습니다.',
    state: { account: '계정으로 연결됨', apiKey: 'API 키로 연결됨', signedOut: '연결 안 됨', notInstalled: '설치 필요', verifying: 'API 키 확인하는 중' } as Record<string, string>,
    /** 그 AI 의 도구가 이 컴퓨터에 없을 때(방향 문서 23~24줄: 몸소가 깔아 준다). 깔기 전에 한 번 묻는다. */
    install: '설치하기',
    installing: '설치하고 있습니다',
    installRetry: '다시 시도',
    installTitle: (a: unknown) => `${a}에 필요한 도구를 설치할까요?`,
    installText: '몸소가 필요한 도구를 이 컴퓨터에 다운로드해 설치합니다. 몇 분 걸릴 수 있습니다.',
    installFailed: '설치하지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요.',
    /** 설치 실패 까닭(provider-install-progress 의 reason). 없거나 모르는 값이면 installFailed. unavailable 은 글 없이 단추만 막는다. */
    installReason: {
      runtimeMissing: '설치에 필요한 파일이 몸소에 없습니다. 몸소를 다시 설치해 주세요.',
      timeout: '설치가 너무 오래 걸려 멈췄습니다. 다시 시도해 주세요.',
      permissionDenied: '이 컴퓨터에서 설치할 권한이 없습니다.',
      startFailed: '설치를 시작하지 못했습니다. 다시 시도해 주세요.',
      toolNotFound: '설치한 도구를 찾지 못했습니다. 다시 시도해 주세요.',
      installFailed: '설치하지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요.',
      // 2026-11-17 뒤쪽이 더한 까닭. 새로 씀, 그의 확인 전.
      checksumMismatch: '받은 파일이 원래 파일과 달라 설치하지 않았습니다. 다시 시도해 주세요.',
      toolNotRunnable: '설치한 도구가 실행되지 않습니다. 다시 시도해 주세요.',
      diskFull: '저장 공간이 모자라 설치하지 못했습니다. 공간을 비운 뒤 다시 시도해 주세요.',
    } as Record<string, string>,
    /** 2026-11-16 김성균 개발자가 정함: 그 회사의 도구에서 로그인한다는 글("로그인 단추의 글도 그렇게 해주세요"). 제미나이는 계정 로그인이 없다(API 키만). */
    login: { claude: '클로드 코드에서 로그인', codex: '코덱스에서 로그인' } as Partial<Record<'codex' | 'claude' | 'gemini', string>>,
    loggingIn: '로그인 중',
    browserHint: '브라우저에서 로그인을 마치면 여기에 나타납니다.',
    cancel: '취소',
    logout: '로그아웃',
    apiKey: 'API 키',
    keyPlaceholder: 'API 키 넣기',
    keySave: '저장',
    keySaved: 'API 키 저장됨',
    keyClear: '키 삭제',
    logoutTitle: (a: unknown) => `${a}에서 로그아웃할까요?`,
    logoutText: '다시 쓰려면 로그인해야 합니다.',
    clearTitle: (a: unknown) => `${a} API 키를 삭제할까요?`,
    clearText: '삭제한 키는 다시 볼 수 없습니다. 다시 쓰려면 키를 새로 넣어야 합니다.',
    invalidKey: '키 모양이 맞지 않습니다. 다시 확인해 주세요.',
    loginFailed: '로그인을 시작하지 못했습니다',
    /** 로그인하는 중이던 것이 끝나지 않고 멈췄을 때(provider-auth-changed 가 account·cancelled 가 아닌 값, 5분 지남도 여기). 2026-11-17 새로 씀, 그의 확인 전. */
    loginUnfinished: '로그인이 끝나지 않았습니다. 다시 시도해 주세요',
    /** 로그인이 시간 안에 끝나지 않았을 때(reason timeout, 5분). 2026-11-17 새로 씀, 그의 확인 전. */
    loginTimeout: '로그인이 시간 안에 끝나지 않았습니다. 다시 시도해 주세요',
    logoutFailed: '로그아웃하지 못했습니다',
    keySaveFailed: '키를 저장하지 못했습니다',
    keyClearFailed: '키를 삭제하지 못했습니다',
    // 2026-11-17 저장된 키의 확인 결과(keyVerification). 새로 씀, 그의 확인 전.
    keyRejected: '저장된 API 키가 맞지 않아 연결되지 않았습니다. 새 키를 넣어 주세요.',
    keyUnchecked: 'API 키를 확인하지 못했습니다. 인터넷 연결을 확인하고 다시 시도해 주세요.',
    keyRetry: '다시 시도',
    keyCheckFailed: 'API 키를 확인하지 못했습니다',
    /** API 키로 쓸 때 따로 드는 비용 안내(그 회사의 글을 Codex 가 확인해 오면 넣는다. 비어 있으면 그리지 않는다). */
    apiCost: { codex: '', claude: '', gemini: '' } as Record<string, string>,
  },
  // ---- 스킬(그가 정한 이름 "스킬"). 아래 글은 2026-11-16 새로 쓴 것(본부·그의 확인 전, 낱말 목록에 올림). ----
  skills: {
    word: '스킬',
    /** 입력 칸에서 / 를 쳤을 때 뜨는 목록의 화면 읽기 이름. */
    pickerLabel: '스킬 선택',
    /** 입력 칸 안 스킬 조각의 지우기 단추. */
    removeChip: (a: unknown) => `${a} 제거`,
    /** 보낸 말 위에 붙는 표시의 화면 읽기 이름. */
    usedLabel: (a: unknown) => `스킬: ${a}`,
    /** 설정 쪽 */
    settingsIntro: '자주 시키는 일을 한 번 적어 두고, 입력창에서 / 를 쳐서 불러 씁니다.',
    empty: '스킬은 자주 시키는 일을 한 번 적어 두는 것입니다. 입력창에서 / 를 치면 선택해 쓸 수 있습니다.',
    add: '새 스킬',
    builtin: '기본 제공',
    mine: '내가 만든 것',
    conflict: '다른 곳에서 수정한 내용과 다릅니다',
    enable: (a: unknown) => `${a} 켜기`,
    fieldTitle: '이름',
    fieldName: '호출 이름',
    fieldNameHelp: '영어 소문자, 숫자, 붙임표(-)만 쓸 수 있습니다. 만든 뒤에는 바꿀 수 없습니다.',
    fieldDescription: '한 줄 설명',
    fieldBody: '내용',
    bodyHint: 'AI가 이 스킬을 쓸 때 따를 내용을 적어 주세요.',
    save: '저장',
    cancel: '취소',
    remove: '삭제',
    back: '스킬 목록',
    removeTitle: (a: unknown) => `“${a}” 스킬을 삭제할까요?`,
    /** 2026-11-17 본부 결정으로 고침(뒤쪽 대조 desktop-promise-audit 7): 앱에 되돌리는 단추가 없다는 뜻. 작업 내역의 글과 같은 말. */
    removeText: '삭제한 스킬은 몸소에서는 되돌릴 수 없습니다.',
    saved: '저장했습니다',
    saveFailed: '저장하지 못했습니다',
    invalidName: '호출 이름은 영어 소문자, 숫자, 붙임표(-)로 적어 주세요.',
    nameTaken: '같은 호출 이름의 스킬이 이미 있습니다.',
    needTitle: '이름을 적어 주세요.',
    conflictTitle: '이 스킬이 그사이 바뀌었습니다',
    conflictText: '수정하는 동안 이 스킬이 다른 곳에서 바뀌었습니다. 바뀐 내용을 불러오면 내가 수정한 내용이 사라지고, 내 글로 덮어쓰면 그사이 바뀐 내용이 사라집니다.',
    reload: '바뀐 내용 불러오기',
    overwrite: '내가 수정한 것으로 덮어쓰기',
    loadFailed: '스킬을 읽지 못했습니다',
    /** 2026-11-17 목록을 읽는 동안(본부 결정). */
    loading: '스킬 목록 불러오는 중',
    toggleFailed: '스킬을 켜거나 끄지 못했습니다',
    // 2026-11-16 쓸 수 있는지(skill_list 의 available·unavailableReason). 새로 씀, 그의 확인 전.
    unavailableTag: '사용할 수 없음',
    unavailableReason: {
      skill_conflict: '다른 곳에서 수정한 내용과 달라 지금은 쓸 수 없습니다.',
      policy_modified: '처음 내용에서 수정되어 이 스킬로 브라우저를 열 수 없습니다.',
      policy_unavailable: '이 스킬의 브라우저 사용 규칙을 찾지 못해 쓸 수 없습니다.',
      browser_unavailable: '이 컴퓨터에서 몸소 브라우저를 열 수 없어 쓸 수 없습니다.',
    },
    removeFailed: '스킬을 삭제하지 못했습니다',
    /** 보낼 때 고른 스킬이 그사이 꺼졌거나 없어졌을 때(chat_send → skill_unavailable). 새로 쓴 글, 그의 확인 전. */
    unavailable: '이 스킬은 지금 쓸 수 없어 보내지 않았습니다',
  },
  // ---- 입력 칸의 클립(이 채팅방에 파일 첨부). 2026-11-16 김성균 개발자가 정함: "첨부"("야 첨부파일이라 해야지 붙인 파일 이러면 누가 아냐"). ----
  attach: {
    done: (name: string) => `${name}${josa(name, '을', '를')} 첨부했습니다`,
    doneMany: (n: number) => `파일 ${n}개를 첨부했습니다`,
    failed: '파일을 첨부하지 못했습니다',
    /** 폴더를 끌어 놓았을 때(source_folder_not_allowed). 2026-11-17 새로 씀, 그의 확인 전. */
    folderNotAllowed: '폴더는 첨부할 수 없습니다. 파일을 놓아 주세요',
    /** 내 말풍선 위 첨부 목록의 화면 읽기 이름. 2026-11-18 새로 씀, 그의 확인 전. */
    sentLabel: (a: unknown) => `첨부 ${a}개`,
    /** momso anywhere 에서 파일을 올리는 동안(S39). 2026-11-22 새로 씀, 그의 확인 전. */
    uploading: (percent: number) => `파일을 올리는 중 ${percent}%`,
  },
  // ---- 채팅방 고정·보관(왼쪽 목록)과 설정의 보관된 채팅방. 2026-11-16 새로 쓴 글(그가 아직 정하지 않음, 낱말 목록에 올림). ----
  chatKeep: {
    pin: '채팅방 고정',
    unpin: '고정 해제',
    archive: '채팅방 보관',
    pinnedHeading: '고정된 채팅방',
    pinFailed: '고정하지 못했습니다',
    unpinFailed: '고정을 풀지 못했습니다',
    archiveFailed: '보관하지 못했습니다',
    settingsTitle: '보관된 채팅방',
    settingsIntro: '보관한 채팅방은 왼쪽 목록에 보이지 않습니다. 되돌리면 다시 보입니다.',
    empty: '보관된 채팅방이 없습니다.',
    loadFailed: '보관된 채팅방을 읽지 못했습니다',
    /** 2026-11-17 목록을 읽는 동안(본부 결정). */
    loading: '보관된 채팅방 불러오는 중',
    restore: '되돌리기',
    restoreFailed: '되돌리지 못했습니다',
    remove: '영구 삭제',
    removeTitle: (a: unknown) => `“${a}” 채팅방을 영구 삭제할까요?`,
    /** 2026-11-17 본부 결정으로 고침(뒤쪽 대조 3): 삭제는 GitHub 백업으로 다른 컴퓨터에도 가고, 예전에 올린 기록에는 남을 수 있다. */
    removeText: '삭제하면 되돌릴 수 없습니다. GitHub에 백업하고 있으면 다른 컴퓨터에서도 지워지고, 예전에 업로드한 기록에는 남아 있을 수 있습니다.',
    removeFailed: '영구 삭제하지 못했습니다',
    cancel: '취소',
  },
  // ---- 빠른 모드 단추의 이름. 2026-11-16 김성균 개발자가 정함: 회사와 관계없이 "고속 모드"("'Fast mode' 저거는 고속 모드"). ----
  fastName: { claude: '고속 모드', codex: '고속 모드', gemini: '고속 모드' } as Partial<Record<'codex' | 'claude' | 'gemini', string>>,
  /** 고속 모드로 보냈는데 지금 쓸 수 없다고 왔을 때(chat-error 의 fast_unavailable). 보낸 말은 채팅방에 남는다. 2026-11-16 새로 쓴 글, 그가 아직 정하지 않음. */
  fastUnavailable: (name: string) => `${name}를 지금 쓸 수 없어 답을 받지 못했습니다. ${name} 없이 다시 보내 주세요.`,  // ---- OneDrive 가 자동으로 올리는 폴더를 몸소 폴더로 골랐을 때(본부가 쓴 글, 그의 확인 전) ----
  oneDrive: {
    notice: '이 폴더는 OneDrive가 자동으로 업로드하는 곳입니다. 몸소 폴더를 여기에 두면 문서의 이력이 엉킬 수 있습니다. 다른 폴더를 권합니다.',
    pickOther: '다른 폴더 선택',
    keep: '그래도 여기에 두기',
  },
  // ---- 미디어 갤러리(미디어 화면과 오른쪽 패널) · 그 채팅방에서 다룬 것. 2026-11-16 새로 쓴 글(본부·그의 확인 전) ----
  gallery: {
    /** 무엇을 했는지(본부가 전한 말). */
    how: { created: '생성', edited: '수정', read: '읽음', attached: '첨부' } as Record<string, string>,
    /** 크게 보기에서 채팅방 이름 단추의 풍선. */
    openChat: '채팅방으로 가기',
    audio: '소리',
    /** 오른쪽 패널의 첫 탭 이름(그 채팅방에서 다룬 문서와 미디어). */
    chatTab: '이 채팅방',
    /** 그 채팅방에서 다룬 것이 없을 때 패널의 한 줄(본부가 든 예를 따름). */
    chatEmpty: '이 채팅방에서 다룬 문서와 첨부파일이 여기에 모입니다.',
    /** 오른쪽 패널의 첨부파일 탭(글 파일처럼 미디어가 아닌 첨부, T42). 2026-11-16 새로 쓴 글. */
    files: '첨부파일',
    /** 2026-11-16 새로 씀(그의 확인 전): 첨부파일 글을 패널에서 열었을 때(attachment_text_read 의 state). */
    fileLoading: '파일을 여는 중',
    fileNotFound: '파일을 찾을 수 없습니다.',
    fileNotText: '글로 볼 수 없는 파일입니다.',
    fileNotUtf8: '글자를 읽을 수 없는 파일입니다.',
    fileFailed: '파일을 열지 못했습니다',
    /** 패널을 닫아 둔 동안 새로 생긴 것이 있을 때 패널 단추의 화면 읽기 이름. */
    newInPanel: (a: unknown) => `${a}, 새로 생긴 것 있음`,
  },
  // ---- restore-progress.ts: 받아 오기와 되살리기의 진행(2026-11-17 새로 씀, 그의 확인 전). 있던 "○○ 불러오는 중" 꼴. ----
  restoreProgress: {
    chatRestoring: '채팅방 되살리는 중',
    chatRestoringCount: (a: unknown, b: unknown) => `채팅방 되살리는 중 ${a}/${b}`,
    /** 되살리는 동안 채팅방을 바꾸려 할 때(보내기·새 채팅방·이름 바꾸기·고정·보관·묶음). */
    chatBusy: '채팅방을 되살리는 동안에는 채팅방을 바꿀 수 없습니다',
    chatFailed: '채팅방을 되살리지 못했습니다. 이 PC에 있던 채팅방은 그대로입니다',
    /** 새 PC 에서 GitHub 를 처음 받아 올 때(archive-first-receive-progress 의 state). */
    receive: {
      recovering: '끊긴 받아 오기를 정리하는 중',
      receiving: 'GitHub에서 받아 오는 중',
      indexing: '문서 정리하는 중',
      restoring: '채팅방 되살리는 중',
    } as Record<string, string>,
    receiveEmpty: 'GitHub에 받아 올 내용이 없습니다',
    /** 받아 오는 동안 못 하는 일(계약: 끝나야 받은 문서와 채팅방을 쓸 수 있다). */
    receiveWait: '다 받을 때까지 받아 온 채팅방은 쓸 수 없습니다',
    /** 같은 이름의 파일이 있어 처음 받아 오기를 멈췄을 때(first_receive_conflict + conflictPaths). 2026-11-17 새로 씀, 그의 확인 전. */
    receiveConflict: (a: unknown, b: unknown) => `몸소 폴더에 같은 이름의 파일 ${a}개가 있어 처음 받아 오기를 멈췄습니다. 이 파일을 옮기거나 이름을 바꾼 뒤 다운로드를 다시 눌러 주세요: ${b}`,
    /** 받아 오는 동안 문서를 고치거나 되돌리려 할 때(채팅방을 막을 때와 같은 알림 줄). */
    docBusy: 'GitHub에서 받아 오는 동안에는 문서를 고칠 수 없습니다',
  },
  // ---- components/DeletedSourcesSettings.tsx: 설정 "삭제한 원자료"(2026-11-17 본부 결정, "보관된 채팅방"과 같은 꼴). 새로 씀, 그의 확인 전. ----
  deletedSources: {
    settingsTitle: '삭제한 원자료',
    settingsIntro: '삭제한 원자료는 첨부파일, 사진과 동영상에 보이지 않습니다. 되돌리면 다시 보입니다.',
    empty: '삭제한 원자료가 없습니다.',
    loading: '삭제한 원자료 불러오는 중',
    loadFailed: '삭제한 원자료를 읽지 못했습니다',
    usedCount: (a: unknown) => `사용처 ${a}개`,
    purgeFailed: '완전히 삭제하지 못했습니다',
    // (지움) purgedName '완전히 삭제한 원자료': 뒤쪽이 다 지운 것을 목록에서 빼게 되어 걷어 냄(2026-11-17).
    /** 완전히 삭제를 눌렀지만 아직 다 지우지 못한 줄의 둘째 줄(local_retry_pending·remote_retry_pending). 2026-11-17 새로 씀, 그의 확인 전. */
    retryPending: '아직 다 지우지 못했습니다. 완전히 삭제를 다시 눌러 주세요',
  },
  // ---- components/PanelBrowser.tsx: 오른쪽 패널 브라우저(2026-11-17 본부 결정). 새로 씀, 그의 확인 전. ----
  panelBrowser: {
    /** 탭 이름: 제목도 호스트도 아직 없을 때. */
    loading: '웹 페이지 불러오는 중',
    reload: '새로 고침',
    stop: '불러오기 멈추기',
    address: '주소',
    insecure: '안전하지 않은 연결',
    openExternal: '기본 브라우저로 열기',
    openFailed: '웹 페이지를 열지 못했습니다',
    /** 링크의 주소를 뒤쪽이 거절했을 때(invalid_url). */
    cannotOpen: '이 주소는 몸소 안에서 열 수 없습니다',
    // (지움) download '몸소 안에서는 파일을 내려받을 수 없습니다…': 뒤쪽이 다운로드를 받게 되어 아래 글로 바꿈(2026-11-18).
    /** 다운로드 알림 줄(panel-browser-download 의 state). 앱의 낱말 "다운로드". 2026-11-18 새로 씀, 그의 확인 전. */
    downloading: (a: unknown) => `${a} 다운로드하는 중`,
    downloadingFile: '파일 다운로드하는 중',
    // 파일 이름은 확장자(영문)로 끝나 조사가 어긋나기 쉬워 쌍점 뒤에 둔다.
    downloaded: (a: unknown) => `다운로드 폴더에 저장했습니다: ${a}`,
    downloadedFile: '파일을 다운로드 폴더에 저장했습니다',
    attachDownload: '채팅방에 첨부',
    /** 2026-11-18 panel_download_reveal. 새로 씀, 그의 확인 전. */
    revealDownload: '폴더에서 보기',
    revealFailed: '폴더를 열지 못했습니다',
    /** 팝업이 열려 있는 동안 패널 위쪽 한 줄(panel-browser-popup). 호스트는 쌍점 뒤에(조사 어긋남 피함). 새로 씀, 그의 확인 전. */
    popupFrom: (a: unknown, b: unknown) => `${a}에서 연 창: ${b}`,
    popupOpen: (a: unknown) => `열린 창: ${a}`,
    downloadBlocked: (a: unknown) => `실행될 수 있는 파일이라 다운로드하지 않았습니다: ${a}`,
    downloadBlockedFile: '실행될 수 있는 파일이라 다운로드하지 않았습니다',
    downloadFailed: (a: unknown) => `다운로드하지 못했습니다: ${a}`,
    downloadFailedFile: '파일을 다운로드하지 못했습니다',
    google: '구글 계정 로그인은 오른쪽 패널에서 할 수 없습니다. 몸소 브라우저 창에서 열어 주세요',
    googleNoSite: '구글 계정 로그인은 오른쪽 패널에서 할 수 없습니다',
    openHuman: '몸소 브라우저 창에서 열기',
    browserMissing: 'Edge나 Chrome이 없어 몸소 브라우저 창을 열지 못했습니다',
    humanFailed: '몸소 브라우저 창을 열지 못했습니다',
    /** AI 가 패널을 쓸 때 맨 위 한 줄(panel-agent-control-changed). 2026-11-18 새로 씀, 그의 확인 전. */
    agentUsing: 'AI가 이 패널을 쓰고 있습니다',
    /** 설정·미디어를 보는 중에 AI 가 패널을 열어 달라고 할 때 알림 줄(누를 때까지 남음). 2026-11-18 본부 임시안, 그의 확인 전. */
    agentWants: 'AI가 브라우저를 쓰려고 합니다',
    /** 설정 화면 등에서 로그인(R2 등)이 오른쪽 패널을 열어 달라고 할 때(2026-11-18). 새로 씀, 그의 확인 전. */
    loginWants: '로그인 페이지를 오른쪽 패널에 엽니다',
    agentOpenPanel: '패널 열기',
    /** AI 가 연 빈 패널의 탭 이름(브라우저의 빈 탭과 같은 말). */
    newTab: '새 탭',
    agentTakeOver: '직접 쓰기',
    agentHuman: '직접 쓰는 중입니다. AI는 기다립니다',
    agentResume: 'AI에게 돌려주기',
    agentBusy: '다른 채팅방의 AI가 이 패널을 쓰고 있습니다',
    agentControlFailed: '패널을 넘기지 못했습니다',
    /** 2026-11-18 G57b: 다른 채팅방의 AI 가 쓸 때, 로그인·인증을 사람이 해야 할 때. 새로 씀, 그의 확인 전. */
    agentOther: (a: unknown) => `“${a}” 채팅방의 AI가 이 패널을 쓰고 있습니다`,
    agentLogin: '로그인은 직접 해 주세요. 끝나면 AI에게 돌려주세요',
    agentCode: '인증 코드는 직접 입력해 주세요. 끝나면 AI에게 돌려주세요',
    agentCheck: '확인 절차는 직접 해 주세요. 끝나면 AI에게 돌려주세요',
    agentLoginOutside: '구글 로그인은 몸소 브라우저 창에서 해 주세요. 끝나면 AI에게 돌려주세요',
    /** 바깥 로그인 뒤 이어 하기(2026-11-18 여섯째 줄 G75): 몸소 브라우저 창이 열림 / AI 가 그 창에서 이어 함. 새로 씀, 그의 확인 전. */
    agentExternalOpened: '몸소 브라우저 창에서 로그인한 뒤 AI에게 돌려주세요',
    agentExternalWorking: 'AI가 몸소 브라우저 창에서 이어서 하고 있습니다',
    agentOpenOutside: '열기',
    agentLoginNotYet: '아직 로그인이 끝나지 않았습니다',
    /** [AI에게 돌려주기] 뒤 이어 하기가 안 될 때(2026-11-18 여섯째 줄 G68 panel_agent_resume 오류). 새로 씀, 그의 확인 전. */
    resumeFailed: {
      in_progress: '이미 AI에게 이어서 하라고 보냈습니다',
      login_wait_ended: '로그인 기다림이 끝났습니다. AI에게 다시 부탁해 주세요.',
      resume_unavailable: 'AI가 이어서 하지 못했습니다. AI에게 다시 부탁해 주세요.',
      external_login_unavailable: '먼저 [열기]로 몸소 브라우저 창을 열어 로그인해 주세요.',
    } as Record<string, string>,
  },
  // ---- 작업 내역 지우기(2026-11-17 본부 결정, 오른쪽 패널 작업 내역 탭 머리의 ⋯ 메뉴). 새로 씀, 그의 확인 전. ----
  receiptDelete: {
    title: '이 작업 내역을 삭제할까요?',
    text: '문서와 채팅방에서 이 작업 내역이 보이지 않게 됩니다. 몸소에서는 되돌릴 수 없습니다.',
    done: '작업 내역을 삭제했습니다',
    failed: '작업 내역을 삭제하지 못했습니다',
  },
  // ---- components/SourceDelete.tsx: 원자료 지우기(2026-11-17 새로 씀, 그의 확인 전). ----
  sourceDelete: {
    /** 크게 보기의 아랫줄과 첨부파일 줄 메뉴의 단추. */
    trash: '삭제',
    purge: '완전히 삭제',
    menu: (a: unknown) => `${a} 메뉴`,
    title: (a: unknown) => `“${a}” 원자료를 삭제할까요?`,
    checking: '사용처 확인하는 중',
    checkFailed: '사용처를 확인하지 못해 삭제하지 않았습니다',
    usedBy: (a: unknown) => `이 원자료의 사용처: ${a}.`,
    unused: '이 원자료를 쓰는 문서나 채팅방이 없습니다.',
    documents: (a: unknown) => `문서 ${a}개`,
    receipts: (a: unknown) => `작업 내역 ${a}개`,
    chats: (a: unknown) => `채팅방 ${a}개`,
    records: (a: unknown) => `예전 기록 ${a}개`,
    more: (a: unknown) => `외 ${a}개`,
    unnamedDocument: '이름 없는 문서',
    /** 2026-11-17 본부 결정으로 고침: 삭제한 원자료는 스스로 지워지지 않는다(기한 없음). 기한 대신 되돌리는 곳을 말한다. 쪽 이름은 deletedSources.settingsTitle 과 같게, 꼴은 "설정의 저장공간에서"처럼. */
    trashText: '삭제해도 설정의 삭제한 원자료에서 되돌릴 수 있습니다.',
    changed: '사용처가 바뀌어 다시 확인했습니다.',
    purgeTitle: (a: unknown) => `“${a}” 원자료를 완전히 삭제할까요?`,
    purgeText: '이 컴퓨터에서 되살릴 수 없게 지웁니다. 연결한 저장소나 다른 곳의 사본은 남아 있을 수 있습니다.',
    trashed: '원자료를 삭제했습니다',
    undo: '되돌리기',
    restored: '원자료를 되살렸습니다',
    restoreFailed: '원자료를 되살리지 못했습니다',
    purged: '원자료를 완전히 삭제했습니다',
    purgedLeft: '이 컴퓨터에서 완전히 삭제했습니다. 연결한 저장소나 다른 곳의 사본은 남아 있을 수 있습니다.',
    busy: 'AI가 답하는 중에는 삭제할 수 없습니다',
    failed: '원자료를 삭제하지 못했습니다',
    openFailed: '원자료를 열지 못했습니다',
  },
  // ---- app-update.ts ----
  appUpdate: {
    /** 2026-11-16 새로 씀(그의 확인 전): update_check 가 failed 일 때 설정 앱 정보의 흐린 한 줄. 릴리스가 없을 때도, 연결이 안 될 때도 오므로 까닭을 적지 않는다. */
    notChecked: '지금은 새 버전을 확인하지 못했습니다.',
    text2: '업데이트를 받지 못했습니다',
    text3: '다시 시작해 업데이트 완료',
    text4: '업데이트 받는 중',
    text5: (a: unknown) => `업데이트${a}`,
    /** 2026-11-22 새로 씀(그의 확인 전, S69): 다시 시작해 업데이트하려는데 진행 중인 채팅이 있어 뒤쪽이 미뤘을 때(busy·active_turn). 알림 줄. */
    restartDeferred: 'AI가 답하는 중이라 업데이트를 미뤘습니다. 답이 끝나면 다시 시작해 주세요.',
    /** 2026-11-22 새로 씀(그의 확인 전, S69): 다른 업데이트 작업 중이라 미뤘을 때(busy, 까닭 없음). 알림 줄. */
    restartBusy: '다른 업데이트 작업이 끝난 뒤 다시 시작해 주세요.',
    /** 2026-11-22 새로 씀(그의 확인 전, S69): 미뤘던 업데이트를, 이 창의 AI 답이 끝난 뒤 다시 묻는 알림 줄(단추 "다시 시작"). */
    restartAfterTurn: '답이 끝났습니다. 다시 시작해 업데이트를 마칠 수 있습니다.',
  },
  // ---- App.tsx ----
  app: {
    /** 몸소 브라우저에서 선생님이 직접 해야 할 때(browser-human-required 의 reason). 채팅방 끝 한 줄. 2026-11-16 새로 씀, 그의 확인 전. */
    browserHuman: {
      login: '브라우저에서 직접 로그인해 주세요.',
      password: '브라우저에서 비밀번호를 직접 넣어 주세요.',
      one_time_code: '브라우저에서 인증 번호를 직접 넣어 주세요.',
      captcha: '브라우저에서 사람인지 확인하는 문제를 직접 풀어 주세요.',
      verification: '브라우저에서 본인 확인을 직접 해 주세요.',
    } as Record<string, string>,
    browserHumanAfter: '끝나면 이 채팅방에 알려 주세요.',
    /**
     * 휴대폰 몸소(momso server, S23 browser-human-required): 캡차·패스키처럼 server 의 브라우저에서 사람이 직접 해야 하는 것은 휴대폰에서 이어서 할 수 없다.
     * 인증 번호는 S32 부터 채팅 안 카드로 받는다(serverInput). 2026-11-21 새로 씀, 그의 확인 전.
     */
    serverBrowserHuman: {
      verification: '사이트가 본인 확인을 더 요구합니다. 휴대폰에서는 아직 이어서 할 수 없습니다.',
    } as Record<string, string>,
    /** "다시 보내기"를 눌렀는데 그 말을 다시 보낼 수 없을 때(chat_send → retry_unavailable). 2026-11-16 새로 씀, 그의 확인 전. */
    retryUnavailable: '이 말은 다시 보낼 수 없어 입력창에 다시 넣었습니다.',
    /** 앱을 켤 때(runtime_info) 실패했고 뒤쪽 글이 한글이 아닐 때(2026-11-17 본부 결정). 알림에 "다시 시도"와 함께. */
    runtimeFailed: '몸소를 준비하지 못했습니다',
    /** 앱 시작 단계(runtime-startup-progress 의 state). 왼쪽 목록의 "불러오는 중" 뒤에 붙는다. 2026-11-17 새로 씀, 그의 확인 전. */
    startup: {
      checking: '자료를 확인하는 중',
      backup: '바꾸기 전 자료를 백업하는 중',
      migrating: '자료를 새 형식으로 바꾸는 중',
    } as Record<string, string>,
    /** 채팅방 GitHub 사본에서 가린 수(chat_archive_redaction_count)가 0 보다 클 때 그 채팅방 끝의 한 줄. 2026-11-16 새로 씀, 그의 확인 전. */
    chatRedacted: (n: number) => `이 채팅방에서 비밀값으로 보이는 값 ${n}개는 GitHub에 올리지 않았습니다.`,
    text1: '채팅방 목록을 읽지 못했습니다',
    text2: '채팅방을 읽지 못했습니다',
    text6: '권한 단계를 바꾸지 못했습니다',
    text7: 'AI 의 답을 받지 못했습니다',
    text8: '답을 보내지 못했습니다',
    text9: '메시지를 저장하지 못했습니다',
    text10: '메시지를 보내지 못했습니다',
    /** 2026-11-16 새로 씀(그의 확인 전): 그 회사의 로그인·키가 받아들여지지 않았을 때(status auth_failed). */
    /**
     * 2026-11-16 새로 씀(그의 확인 전): 답이 오지 않았을 때 채팅방 끝의 한 줄(chat-error 의 error.kind, G18). 〈값1〉은 회사 이름.
     * 로그인이 풀린 것(authentication)은 아래 authFailed 를 그대로 쓴다. 뒤쪽이 가르지 못하면(unknown) text7.
     */
    failure: {
      usageLimit: (a: unknown) => `${a}에서 쓸 수 있는 양을 다 썼습니다.`,
      rateLimited: (a: unknown) => `${a}에 잠깐 너무 많이 보냈습니다. 조금 뒤에 다시 보내 주세요.`,
      billing: (a: unknown) => `${a} API 키의 결제가 필요하거나 남은 금액이 없습니다. 설정의 AI 연결에서 확인해 주세요.`,
      modelUnavailable: (a: unknown) => `선택한 모델을 지금 ${a} 요금제에서 쓸 수 없습니다. 다른 모델로 바꿔 주세요.`,
      connectionClosed: (a: unknown) => `${a}와의 연결이 끊겼습니다. 인터넷 연결을 확인하고 다시 보내 주세요.`,
      timeout: (a: unknown) => `${a}의 답이 너무 오래 걸려 멈췄습니다. 다시 보내 주세요.`,
      toolMissing: (a: unknown) => `${a} 프로그램이 없거나 오래되었습니다. 설정의 AI 연결에서 설치해 주세요.`,
      serviceUnavailable: (a: unknown) => `${a} 쪽이 잠깐 안 됩니다. 조금 뒤에 다시 보내 주세요.`,
      contextTooLong: (a: unknown) => `채팅방이 너무 길어 ${a}가 더 받지 못합니다. 새 채팅방에서 이어 가 주세요.`,
      /** 휴대폰 몸소(momso server) 가 준 까닭 값(S17 6ec22f472)마다. 화면 글의 제품 이름은 데스크톱처럼 "몸소". 2026-11-21 새로 씀, 그의 확인 전. */
      serverCliUnavailable: (a: unknown) => `몸소에서 ${a}를 쓸 수 없습니다. ${a}가 설치되어 있고 로그인되어 있는지 확인해 주세요.`,
      serverProviderFailed: (a: unknown) => `${a}가 답하지 못했습니다. 몸소의 ${a} 로그인이 풀렸거나 쓸 수 있는 양을 다 썼을 수 있습니다.`,
      serverCliStopped: (a: unknown) => `${a}가 답하는 도중 멈췄습니다. 다시 보내 주세요.`,
      serverEmpty: (a: unknown) => `${a}가 빈 답을 보냈습니다. 다시 보내 주세요.`,
      serverRestarted: () => '답을 쓰는 동안 몸소가 다시 시작되어 답을 받지 못했습니다. 다시 보내 주세요.',
      serverStorage: () => '몸소에 저장하지 못했습니다. 잠시 뒤 다시 보내 주세요.',
    } as Record<string, (a: unknown) => string>,
    /** 회사가 다시 쓸 수 있는 때를 주었을 때 위 줄 뒤에 붙는다. */
    retryAt: (a: unknown) => `${a}에 다시 쓸 수 있습니다.`,
    /** 2026-11-18 새로 씀(그의 확인 전): 다시 쓸 수 있는 때가 한 시간 안일 때, API 키가 받아들여지지 않았을 때(actions checkApiKey). */
    retryIn: (a: unknown) => `${a}분 뒤에 다시 쓸 수 있습니다.`,
    authKeyFailed: (a: unknown) => `${a} API 키가 받아들여지지 않았습니다. 설정의 AI 연결에서 키를 확인해 주세요.`,
    /** 2026-11-18 새로 씀(그의 확인 전): 회사가 스스로 다시 시도하는 중(chat-provider-retrying)일 때 답 아래 한 줄. */
    retrying: {
      rateLimited: (a: unknown) => `${a}에 요청이 많아 다시 시도하는 중입니다`,
      serviceUnavailable: (a: unknown) => `${a} 쪽이 잠깐 안 돼 다시 시도하는 중입니다`,
      connectionClosed: (a: unknown) => `${a}와의 연결이 끊겨 다시 시도하는 중입니다`,
      other: (a: unknown) => `${a}${josa(a, '이', '가')} 다시 시도하는 중입니다`,
    } as Record<string, (a: unknown) => string>,
    retryingAttempt: (n: number) => `(${n}번째)`,
    retryingWait: (n: number) => `${n}초 뒤`,
    retryingAi: 'AI',
    /** 그 줄의 단추(지금 할 수 있는 일). */
    failureAction: {
      retry: '다시 보내기',
      settings: 'AI 연결 열기',
      switchModel: '모델 바꾸기',
      switchProvider: '다른 AI로 바꾸기',
      shorten: '새 채팅방',
    } as Record<string, string>,
    /** 화면 한 부분이 그리다 죽었을 때 그 자리(오류 경계, 2026-11-18). 새로 씀, 그의 확인 전. */
    partFailed: '이 부분을 표시하지 못했습니다',
    authFailed: (a: unknown) => `${a} 연결이 끊어졌습니다. 설정의 AI 연결에서 다시 연결해 주세요.`,
    text11: '이름을 바꾸지 못했습니다',
    text12: '이 환경에서는 폴더 선택을 사용할 수 없습니다',
    text13: '다시 시작하지 못했습니다',
    text14: (a: unknown) => `새 ${a} 만들기`,
    text15: (a: unknown) => `${a} 화면으로 가기`,
    text16: '설정 열기',
    text17: (a: unknown) => `새 ${a}`,
    text18: (a: unknown) => `${a}${josa(a, '을', '를')} 만들지 못했습니다`,
    text19: '차례를 바꾸지 못했습니다',
    text20: (a: unknown) => `${a}${josa(a, '을', '를')} 삭제하지 못했습니다`,
    text21: (a: unknown) => `${a}${josa(a, '을', '를')} 바꾸지 못했습니다`,
    text22: '채팅방이 길어져 앞의 내용을 줄여서 이어 갑니다.',
    title1: '다시 시작할까요?',
    text23: '저장하지 않은 수정 내용이 있거나 AI가 답하는 중입니다. 지금 다시 시작하면 그 내용이 사라집니다.',
    text24: '취소',
    text25: '다시 시작',
  },
  // ---- approvals.ts ----
  approvals: {
    text1: '이번만 허락',
    // 2026-11-16 김성균 개발자가 정함("이 채팅방에서 항상 허락 이렇게").
    // 2026-11-17 본부 결정으로 고침(뒤쪽 대조 9): 코덱스가 주는 "이번 세션 동안 허락"을 전할 뿐 몸소가 저장하지 않는다. "항상"이 아니다.
    text2: '이 채팅방에서 계속 허락',
    text3: '거절',
    text4: '취소',
    text5: '이번만 허락했습니다',
    text6: '이 채팅방에서 계속 허락했습니다. 앱을 다시 시작하거나 AI를 바꾸면 다시 묻습니다',
    text7: '거절했습니다',
    text8: '취소했습니다',
    /** 답하기 전에 뒤쪽이 물음을 닫았을 때(agent-approval-closed: 5분 지남이나 대화 멈춤, 뒤쪽 값으로는 둘을 가르지 못함). 2026-11-17 새로 씀, 그의 확인 전. */
    closed: '답하기 전에 요청이 닫혔습니다',
    /** 5분 동안 답하지 않아 닫혔을 때(reason timeout). 2026-11-17 새로 씀, 그의 확인 전. */
    closedTimeout: '답이 없어 요청이 닫혔습니다. 다시 하려면 AI에게 다시 말해 주세요',
    /** AI 쪽이 요청을 거두었을 때(reason provider_cancelled, 선생님이 멈춤을 누른 때는 빼고). 키 넣는 카드도 같은 글. 2026-11-17 새로 씀, 그의 확인 전. */
    closedByAi: 'AI가 요청을 거두었습니다',
  },
  // ---- archive.ts ----
  archive: {
    text1: '열 수 없는 주소입니다',
  },
  // ---- components/ActivityRail.tsx ----
  activityRail: {
    aria1: '몸소 화면',
    aria2: '화면',
  },
  // ---- components/ApprovalCard.tsx ----
  approvalCard: {
    text1: (a: unknown) => `${a}${josa(a, '이', '가')} 컴퓨터에서 이 명령을 실행하려고 합니다`,
    text2: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 수정하려고 합니다`,
    text3: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 수정하려고 합니다`,
    text4: (a: unknown) => `${a}${josa(a, '이', '가')} 허락을 구합니다`,
    aria1: '승인 요청',
    /** 코덱스가 보호 장치(샌드박스) 밖에서 명령을 다시 실행하려고 할 때(2026-11-18 실제 앱 점검). 새로 씀, 그의 확인 전. */
    outsideSandbox: (a: unknown) => `${a}${josa(a, '이', '가')} 보호 장치 밖에서 명령을 다시 실행하려고 합니다`,
    outsideSandboxWarn: '보호 장치 밖에서는 몸소 폴더 밖의 파일도 바꿀 수 있습니다. 명령을 확인한 뒤에만 허락해 주세요.',
    showCommand: '명령 보기',
    /** 2026-11-19 G81: 알 수 없는 도구의 원문, 원자료 파일의 정확한 경로를 펼쳐 보는 단추 / 원자료(objects/…) 파일의 이름 대신. 새로 씀, 그의 확인 전. */
    showDetail: '자세히 보기',
    sourceFile: '원자료 파일',
    /**
     * 2026-11-16 새로 쓴 글(그가 아직 정하지 않음): 파일을 어떻게 바꾸려는지(change)에 따른 머리 글.
     * edit 는 위 text2/text3 을 그대로 쓴다. 종류를 모르면(null) "바꾸려고"로 짐작하지 않고 적는다.
     */
    change: {
      create: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 새로 만들려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 새로 만들려고 합니다` },
      overwrite: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 덮어쓰려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 덮어쓰려고 합니다` },
      delete: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 삭제하려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 삭제하려고 합니다` },
      unknown: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 바꾸려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 바꾸려고 합니다` },
    },
    /** 대상이 여럿일 때 머리 글과 대상마다 붙는 종류 낱말. */
    many: (a: unknown, n: number) => `${a}${josa(a, '이', '가')} 파일 ${n}개를 바꾸려고 합니다`,
    /** 코덱스가 파일 목록 없이 허락할 폴더(grantRoot)만 보냈을 때, 그것도 없을 때. */
    inFolder: (a: unknown) => `${a}${josa(a, '이', '가')} 이 폴더 안의 파일을 바꾸려고 합니다`,
    someFile: (a: unknown) => `${a}${josa(a, '이', '가')} 파일을 바꾸려고 합니다`,
    targetChange: { create: '생성', edit: '수정', overwrite: '덮어쓰기', delete: '삭제' } as Record<string, string>,
    /** 몸소 브라우저의 물음(kind browser_action_scope / browser_action). 2026-11-16 새로 씀, 그의 확인 전. */
    browser: {
      scopeHead: (a: unknown) => `${a}${josa(a, '이', '가')} 브라우저에서 읽기만 하다가 바꾸는 일을 하려고 합니다`,
      actionHead: (a: unknown) => `${a}${josa(a, '이', '가')} 브라우저에서 이 일을 하려고 합니다`,
      scopeNote: '허락해도 누르거나 입력할 때마다 다시 묻습니다.',
      /**
       * 사이트 승인(reason site_access, 2026-11-21 뒤쪽 3c76dcf): 허락이 이 채팅방·호스트에 앱이 켜져 있는 동안 기억된다.
       * 새 글이 아니다: 허락 단추는 approvals.text2, 답한 줄은 approvals.text6 의 앞 문장만(뒷 문장 "AI를 바꾸면 다시 묻습니다"는 이 승인에 맞지 않음).
       */
      siteAllowedHere: '이 채팅방에서 계속 허락했습니다',
      /** 사이트 승인의 물음 글. 새 글이 아니다: 데스크톱 뒤쪽(browser_runtime.rs site_access)이 보내는 글 그대로. 휴대폰 몸소(S23 browser_site)는 화면이 채운다. */
      siteQuestion: '이 사이트에서 누르고 입력해도 될까요',
      site: '사이트',
      result: '누르면',
      effect: {
        publish: '사이트에 올라갑니다',
        delete: '사이트에서 삭제됩니다',
        notify: '다른 사람에게 알림이나 메시지가 갈 수 있습니다',
        refund: '환불될 수 있습니다',
        payment: '돈이 나갈 수 있습니다',
        irreversible: '되돌릴 수 없습니다',
        unknown: '무엇이 바뀌는지 확인되지 않았습니다',
      },
    },
    /** 스킬로 하는 연결에서 AI 가 만들거나 연결하려 할 때(kind 'setup'). 2026-11-16 새로 쓴 글. 요가 선생님이 읽는 글이라 "버킷" 같은 말을 쓰지 않는다. */
    setup: {
      createRepository: (a: unknown, b: unknown) => `${a}${josa(a, '이', '가')} ${b}에 새 저장소를 만들고 몸소를 연결하려고 합니다`,
      createStorage: (a: unknown, b: unknown) => `${a}${josa(a, '이', '가')} ${b}에 원자료를 보관할 곳을 새로 만들고 몸소를 연결하려고 합니다`,
      service: '회사',
      account: '계정',
      name: '새로 만들 이름',
      /** 2026-11-16 계약에 맞춰 새로 씀(그의 확인 전): 원자료를 둘 지역(setup.region), 보이는 사람(setup.visibility). */
      region: '지역',
      /**
       * 지역 값을 사람이 읽는 말로(2026-11-16 본부). 계약(통합 브랜치 da496aa3 setup_actions.rs): Cloudflare·GCP 는 늘 "auto", AWS 는 지역 코드
       * (us-east-1 은 받지 않음). AWS 이름은 AWS 한국어 콘솔의 지역 이름 꼴. 표에 없는 값은 받은 그대로 보인다.
       */
      regionName: {
        auto: '자동(그 회사가 정합니다)',
        'us-east-2': '미국 동부(오하이오)', 'us-west-1': '미국 서부(캘리포니아 북부)', 'us-west-2': '미국 서부(오레곤)',
        'ca-central-1': '캐나다(중부)', 'sa-east-1': '남아메리카(상파울루)', 'af-south-1': '아프리카(케이프타운)',
        'ap-northeast-1': '아시아 태평양(도쿄)', 'ap-northeast-2': '아시아 태평양(서울)', 'ap-northeast-3': '아시아 태평양(오사카)',
        'ap-east-1': '아시아 태평양(홍콩)', 'ap-south-1': '아시아 태평양(뭄바이)', 'ap-south-2': '아시아 태평양(하이데라바드)',
        'ap-southeast-1': '아시아 태평양(싱가포르)', 'ap-southeast-2': '아시아 태평양(시드니)', 'ap-southeast-3': '아시아 태평양(자카르타)', 'ap-southeast-4': '아시아 태평양(멜버른)',
        'eu-central-1': '유럽(프랑크푸르트)', 'eu-central-2': '유럽(취리히)', 'eu-west-1': '유럽(아일랜드)', 'eu-west-2': '유럽(런던)', 'eu-west-3': '유럽(파리)',
        'eu-south-1': '유럽(밀라노)', 'eu-south-2': '유럽(스페인)', 'eu-north-1': '유럽(스톡홀름)',
        'me-south-1': '중동(바레인)', 'me-central-1': '중동(UAE)', 'il-central-1': '이스라엘(텔아비브)',
      } as Record<string, string>,
      visibility: '공개 범위',
      visibilityText: { private: '나만 볼 수 있습니다.' } as Record<string, string>,
      cost: '비용',
      costText: { possible: '쓰는 양에 따라 그 회사에서 돈이 나갈 수 있습니다.' } as Record<string, string>,
    },
  },
  // ---- components/SecretRequestCard.tsx: 스킬로 하는 연결에서 키를 사람이 직접 넣는 칸. 2026-11-16 새로 쓴 글. ----
  /**
   * 휴대폰 몸소의 입력 카드(줄3 S32 agent-input-request): 키 넣기 카드(secretRequest)와 같은 꼴. 칸 이름은 열쇠 창의 말(savedLogins.field*) 그대로.
   * 2026-11-21 새로 씀, 그의 확인 전.
   */
  serverInput: {
    one_time_code: {
      aria: '인증 번호 넣기',
      title: '사이트가 보낸 인증 번호를 넣어 주세요',
      code: '인증 번호',
      note: '넣은 인증 번호는 AI에게 보내지 않고 그 사이트에만 넣습니다.',
      done: '인증 번호를 넣었습니다',
      cancelled: '인증 번호 입력을 취소했습니다',
      closed: '인증 번호를 넣기 전에 요청이 닫혔습니다',
      closedTimeout: '인증 번호를 넣지 않아 요청이 닫혔습니다. 다시 하려면 AI에게 다시 말해 주세요',
      invalid: '인증 번호를 다시 확인해 주세요. 숫자와 영문자만 넣을 수 있습니다.',
      failed: '인증 번호를 보내지 못했습니다',
      submit: '보내기',
    },
    saved_login: {
      aria: '이 사이트의 로그인 저장',
      title: '이 사이트의 로그인을 저장해 주세요',
      note: '비밀번호는 AI에게 보내지 않고 암호화해 저장하며 이 사이트에만 넣습니다.',
      done: '저장했습니다',
      cancelled: '로그인 저장을 취소했습니다',
      closed: '로그인을 저장하기 전에 요청이 닫혔습니다',
      closedTimeout: '로그인을 저장하지 않아 요청이 닫혔습니다. 다시 하려면 AI에게 다시 말해 주세요',
      invalid: '아이디와 비밀번호를 넣어 주세요.',
      failed: '저장하지 못했습니다',
      submit: '저장',
    },
  } as Record<string, { aria: string; title: string; code?: string; note: string; done: string; cancelled: string; closed: string; closedTimeout: string; invalid: string; failed: string; submit: string }>,
  secretRequest: {
    aria: '연결에 필요한 키 넣기',
    title: (a: unknown) => `${a} 연결에 필요한 키를 넣어 주세요`,
    note: '넣은 키는 AI에게 보내지 않고 이 컴퓨터의 몸소가 보관합니다.',
    save: '저장',
    cancel: '취소',
    saved: '키를 저장했습니다',
    cancelled: '키 입력을 취소했습니다',
    /** 키를 넣기 전에 뒤쪽이 요청을 닫았을 때(agent-setup-secret-closed: 5분 지남이나 대화 멈춤). 2026-11-17 새로 씀, 그의 확인 전. */
    closed: '키를 넣기 전에 요청이 닫혔습니다',
    /** 5분 동안 키를 넣지 않아 닫혔을 때(reason timeout). 2026-11-17 새로 씀, 그의 확인 전. */
    closedTimeout: '키를 넣지 않아 요청이 닫혔습니다. 다시 하려면 AI에게 다시 말해 주세요',
    /** 2026-11-16 새로 씀(그의 확인 전): 그만두기(agent_setup_secret_cancel)가 실패했을 때. */
    cancelFailed: '취소하지 못했습니다',
    failed: '키를 저장하지 못했습니다',
    /**
     * 칸 이름: 선생님이 그 회사 화면에서 키를 복사해 오므로 그 회사 화면에 적힌 이름 그대로(2026-11-16 본부). 뒤쪽에 보내는 이름(id)은 계약 그대로다.
     *   Cloudflare: R2 문서의 대시보드 이름 "Access Key ID", "Secret Access Key"(확인).
     *   AWS: 콘솔에서 받는 .csv 의 머리 "Access key ID", "Secret access key"(AWS CLI 문서로 확인. 콘솔 화면 표의 글은 문서에 없어 확인 못 함).
     *   GCP: HMAC 키 화면의 "Access key", "Secret", 토큰은 "Access token" — 구글 문서에 화면 글이 적혀 있지 않아 확인 못 함.
     * 표에 없는 회사·칸은 뒤쪽 이름 그대로.
     */
    fieldLabel: {
      cloudflare: { accessKeyId: 'Access Key ID', secretAccessKey: 'Secret Access Key' },
      aws: { accessKeyId: 'Access key ID', secretAccessKey: 'Secret access key' },
      gcp: { accessKeyId: 'Access key', secretAccessKey: 'Secret', gcpOauthToken: 'Access token' },
    } as Record<string, Record<string, string>>,
    /** 뒤쪽이 주는 칸 이름(id)마다 한 줄 풀이. 모르는 칸은 풀이 없이. */
    help: {
      accessKeyId: '그 회사에서 받은 키의 아이디입니다.',
      secretAccessKey: '아이디와 짝을 이루는 비밀 키입니다. 다른 사람에게 보여 주지 마세요.',
      endpoint: '그 회사가 알려 준 연결 주소입니다.',
      region: '자료를 둘 지역입니다.',
      token: '그 회사에서 받은 비밀 번호입니다. 다른 사람에게 보여 주지 마세요.',
      apiKey: '그 회사에서 받은 비밀 번호입니다. 다른 사람에게 보여 주지 마세요.',
      gcpProjectId: 'Google Cloud에서 쓰는 프로젝트의 아이디입니다.',
      gcpOauthToken: 'Google Cloud에서 받은 로그인 토큰입니다. 다른 사람에게 보여 주지 마세요.',
    } as Record<string, string>,
  },
  // ---- components/AppSettingsParts.tsx ----
  appSettingsParts: {
    text1: '폴더를 옮기지 못했습니다',
    /** 2026-11-17 몸소 폴더 위치를 읽는 동안·읽지 못했을 때(본부 결정). */
    pathLoading: (a: unknown) => `${a} 위치 불러오는 중`,
    pathFailed: (a: unknown) => `${a} 위치를 읽지 못했습니다`,
    /** 2026-11-16 새로 쓴 글(그의 확인 전): 옮기지 못한 까닭. */
    moveNotEmpty: '선택한 폴더가 비어 있지 않습니다. 빈 폴더를 선택하세요.',
    moveInvalid: '그 폴더로는 옮길 수 없습니다. 다른 폴더를 선택하세요.',
    /** 2026-11-16 새로 쓴 글(그가 아직 정하지 않음): 옮긴 뒤 예전 폴더가 그대로 남아 있을 때(previousKept). */
    previousKept: (path: string | null) => (path ? `예전 폴더는 삭제하지 않고 그대로 두었습니다: ${path}` : '예전 폴더는 삭제하지 않고 그대로 두었습니다.'),
    text2: (a: unknown, b: unknown) => `${a}와 ${b}가 이 폴더에 들어 있습니다.`,
    text3: '폴더 열기',
    text4: '위치 변경',
    aria1: '폴더 옮기는 중',
    text5: '버전',
    text6: '업데이트',
    /** 2026-11-16 새로 씀(그의 확인 전): 앱 정보 아래 저작권. 저작권 줄은 NOTICE 의 표기 그대로(MOW and BigBlue), 나머지는 NOTICE·LICENSE 를 짧게 옮김. */
    copyright: 'Copyright © 2026 MOW and BigBlue',
    license: (name: string) => `몸소는 ${name} 라이선스를 따릅니다.`,
    licenseLink: '라이선스 전문 보기',
    text7: '최신 버전입니다.',
    text8: '새 버전 ',
    text9: '이 있습니다.',
    aria2: '업데이트 받는 중',
    text10: '다 받았습니다. 다시 시작하면 업데이트가 끝납니다.',
    text11: '업데이트 확인',
  },
  // ---- components/AssistantPanel.tsx ----
  assistantPanel: {
    text1: '새 채팅방',
    /** 이 채팅방의 몸소 브라우저가 열려 있을 때 채팅방 머리의 표시. 2026-11-16 새로 씀, 그의 확인 전. */
    browserOpen: '몸소 브라우저 열림',
    /** 2026-11-16 그의 말에서 가져옴(그의 확인 전): 아직 말이 없는 채팅방 한가운데, 몸소 CI 아래의 한 줄. */
    emptyTitle: '무엇을 할까요?',
    aria1: '채팅방',
    /** 2026-11-16 바꾼 글(그가 아직 정하지 않음): 넘기지 못한 말의 수가 온다(chat-done 의 notHandedOver). */
    text2: (a: unknown, n: number) => `이전 메시지 ${n}개를 ${a}에 넘기지 못했습니다. 아래 답은 그 메시지를 모르고 쓴 것입니다.`,
    text3: '답변 작성 중',
    aria2: '채팅방 이름',
    tip1: '이름 바꾸기',
    /** 답이 끝난 까닭(chat_read 의 finishReason). completed 와 옛 말(null)은 적지 않는다. 2026-11-16 새로 쓴 글. */
    endReason: {
      length_limit: '답이 너무 길어 중간에 끊겼습니다.',
      turn_limit: '정해진 작업 횟수를 다 써서 답이 중간에 끝났습니다.',
      cancelled: '답을 중간에 멈췄습니다.',
      error: '문제가 생겨 답이 중간에 끊겼습니다.',
      refusal: 'AI가 이 요청에는 답하지 않았습니다.',
    } as Record<string, string>,
  },
  // ---- components/BrandMotion.tsx ----
  brandMotion: {
    text1: '몸소 로고 애니메이션 다시 보기',
    text2: '몸소 심볼',
  },
  // ---- components/ChatSidebar.tsx ----
  chatSidebar: {
    aria1: '채팅방 목록',
    /** 2026-11-17 목록을 읽는 동안(본부 결정, 그가 보고 고친다). */
    loading: '채팅방 목록 불러오는 중',
    text1: '새 채팅방',
    label1: (a: unknown) => `새 ${a}`,
    title1: (a: unknown, b: unknown) => `“${a}” ${b}${josa(b, '을', '를')} 삭제할까요?`,
    text2: (a: unknown) => `안의 채팅방은 지워지지 않고 ${a} 밖으로 나옵니다.`,
    text3: '취소',
    text4: (a: unknown) => `${a} 삭제`,
    aria2: (a: unknown) => `${a} 이름`,
    label2: (a: unknown, b: unknown) => `${a} ${b} 메뉴`,
    text5: '이름 바꾸기',
    text6: '위로 옮기기',
    text7: '아래로 옮기기',
    aria3: (a: unknown) => `${a}, 답변 대기 중`,
    label3: (a: unknown) => `${a} 메뉴`,
    text8: (a: unknown) => `${a}에 넣기`,
    text9: (a: unknown) => `${a}에서 제거`,
  },
  // ---- components/Composer.tsx ----
  /** GitHub 로그인 정보가 안전하지 않은 곳에 남았을 때(2026-11-18 넷째 줄 D33·D34 provider-login-insecure-storage). 새로 씀, 그의 확인 전. */
  loginStorage: {
    removed: 'GitHub 로그인 정보가 안전하지 않은 곳에 남아 있어 지웠습니다. GitHub을 다시 연결해 주세요.',
    cleanup_failed: 'GitHub 로그인 정보가 안전하지 않은 곳에 남아 있는데 지우지 못했습니다. 몸소를 다시 켠 뒤 GitHub을 다시 연결해 주세요.',
    verification_required: 'GitHub 로그인 정보가 안전하게 저장됐는지 확인하지 못했습니다. 다음에 다시 확인합니다.',
    open: '연결과 백업 열기',
  } as Record<string, string>,
  /** 로그인 코드 카드(2026-11-18, 장치 코드 로그인·클로드 기본 브라우저 안내). 새로 씀, 그의 확인 전. */
  deviceLogin: {
    names: { github: 'GitHub', codex: '코덱스', claude: '클로드', gemini: '제미나이' } as Record<string, string>,
    title: (a: unknown) => `${a} 로그인`,
    text: '로그인 페이지에 아래 코드를 입력해 주세요.',
    codeLabel: '로그인 코드',
    copy: '복사',
    copied: '코드를 복사했습니다',
    copyFailed: '코드를 복사하지 못했습니다',
    left: (a: unknown) => `${a} 남음`,
    expired: '코드 시간이 지났습니다. 다시 로그인해 주세요.',
    openPage: '로그인 페이지 열기',
    cancel: '취소',
    browserText: (a: unknown) => `기본 브라우저에서 ${a} 로그인 창이 열립니다. 로그인을 마치면 몸소로 돌아와 주세요.`,
    ok: '확인',
    /** momso anywhere(휴대폰): 한 번 눌러 코드를 복사하고 로그인 페이지를 새 탭으로(VS Code 의 "Copy & Continue" 꼴), 코드 시간이 지났을 때 다시 받기. 2026-11-21 새로 씀, 그의 확인 전. */
    copyAndOpen: '코드 복사하고 로그인 페이지 열기',
    restart: '코드 다시 받기',
  },
  composer: {
    /** 사진과 동영상의 "다시 만들기"로 붙인 생성물 칩(2026-11-18). 새로 씀, 그의 확인 전. */
    regenerateImage: '다시 만들 사진',
    regenerateVideo: '다시 만들 동영상',
    regenerateRemove: '다시 만들기 빼기',
    /** 2026-11-17 모델 목록을 읽는 동안·읽지 못했을 때(본부 결정, 그가 보고 고친다). */
    modelsLoading: '모델 목록 불러오는 중',
    modelsFailed: '모델 목록을 읽지 못했습니다',
    text1: 'Enter 키로 보내기, Shift+Enter 키로 줄 바꾸기',
    text2: 'Enter 키로 보내기',
    /** 클립 단추(이 채팅방에 파일 첨부). 2026-11-16 "첨부"로 정해짐. */
    aria1: '파일 첨부',
    /** 마이크 단추의 풍선·화면 읽기 이름. Codex 앱 한국어판의 같은 자리 글("음성 입력"). 2026-11-16. */
    voice: '음성 입력',
    /** 받아쓰기를 열지 못했을 때(voice_typing_open 의 state). requested 는 아무것도 적지 않는다. unsupported 는 단추를 막는다. */
    voiceProblem: {
      not_focused: '몸소 창을 누른 뒤 다시 눌러 주세요.',
      keys_busy: '누르고 있는 키를 뗀 뒤 다시 눌러 주세요.',
      input_rejected: '음성 입력을 시작할 수 없습니다.',
    } as Record<string, string>,
    aria2: '메시지 입력',
    aria3: '응답 중단',
    aria4: '보내기',
    tip1: 'AI 선택',
    aria5: (a: unknown) => `모델: ${a}`,
    label1: '모델',
    aria6: (a: unknown) => `추론 수준: ${a}`,
    aria7: '추론 수준',
    tip3: '추론 수준 선택',
    aria8: '특별한 추론 수준',
    /** 그 회사가 추천으로 표시한 추론 수준 옆 꼬리표(Claude 앱의 공식 한국어 "추천"). */
    recommended: '추천',
    /** 추론 수준 슬라이더의 화면 읽기 이름. */
    effortSlider: '추론 수준 바꾸기',
    aria9: (a: unknown) => `권한: ${a}`,
    tip4: '권한 단계',
    text3: (a: unknown) => `이 채팅방이 AI가 한 번에 볼 수 있는 양의 ${a}%를 채웠습니다.`,
  },
  // ---- components/AssistantPanel.tsx 의 말 아래 단추(MessageActions). 2026-11-16 새로 씀(그의 확인 전, 낱말 목록에 올림). ----
  messageActions: {
    /** 내 말·AI 의 답 아래 복사 단추의 풍선·화면 읽기 이름. */
    copy: '복사',
    /** 누른 뒤 잠깐(그림이 체크로 바뀌는 동안) 풍선·화면 읽기 이름. */
    copied: '복사했습니다',
  },
  // ---- components/TermDictionarySettings.tsx: 설정 › 용어 사전(보기만). 이름 "용어 사전"은 그가 정함. 나머지는 2026-11-16 새로 씀(그가 보고 고친다). ----
  termDictionary: {
    title: '용어 사전',
    intro: 'AI가 수업 녹음과 글을 알아들을 때 참고하는 요가 용어입니다. 같은 말을 적는 여러 표기를 함께 적어 둡니다.',
    searchHint: '용어 찾기(한글, 산스크리트, 우리말 이름)',
    loading: '용어 사전을 여는 중',
    failed: '용어 사전을 읽지 못했습니다',
    total: (n: number) => `용어 ${n}개`,
    found: (n: number) => `찾은 용어 ${n}개`,
    empty: '찾는 용어가 없습니다.',
    otherCategory: '기타',
    mine: '내가 추가함',
    spellings: '다른 표기',
    edited: '내가 수정함',
    // 2026-11-16 쓰기(더하기·수정·끄기, 사람 이름). 새로 씀, 그의 확인 전.
    termsTab: '용어',
    peopleTab: '사람 이름',
    addTerm: '새 용어',
    addPerson: '새 이름',
    fieldSpelling: '표기',
    fieldMeaning: '뜻',
    save: '저장',
    cancel: '취소',
    edit: '수정',
    remove: '삭제',
    saveMeaning: '뜻 저장',
    addSpellingHint: '다른 표기 추가',
    addSpelling: '추가',
    removeSpelling: (a: unknown) => `${a} 삭제`,
    useTerm: '이 용어 사용',
    off: '꺼짐',
    saved: '저장했습니다',
    saveFailed: '저장하지 못했습니다',
    invalidInput: '표기와 이름은 100자, 뜻은 500자까지 적을 수 있습니다.',
    changed: '그사이 용어 사전이 바뀌어 다시 읽었습니다. 한 번 더 해 주세요.',
    peopleSearchHint: '이름 찾기',
    peopleTotal: (n: number) => `이름 ${n}개`,
    peopleFound: (n: number) => `찾은 이름 ${n}개`,
    peopleEmpty: '적어 둔 사람 이름이 없습니다.',
    peopleInvalid: '사람 이름 파일을 읽지 못했습니다. 파일은 그대로 두었습니다.',
    fieldName: '이름',
    fieldAliases: '별칭(쉼표로 구분)',
    aliases: '별칭',
    unconfirmed: '확인 안 됨',
    confirm: '확인됨으로 표시',
    removePersonTitle: (a: unknown) => `“${a}” 이름을 삭제할까요?`,
    /** 2026-11-17 본부 결정으로 고침(뒤쪽 대조 desktop-promise-audit 7): 앱에 되돌리는 단추가 없다는 뜻. 작업 내역의 글과 같은 말. */
    removePersonText: '삭제한 이름은 몸소에서는 되돌릴 수 없습니다.',
    // 2026-11-17 용어 삭제와 새 용어의 갈래. 새로 씀, 그의 확인 전.
    fieldCategory: '분류',
    categoryNone: '선택 안 함',
    deleteTerm: '용어 삭제',
    removeTermTitle: (a: unknown) => `“${a}” 용어를 삭제할까요?`,
    /** 2026-11-17 본부 결정으로 고침(뒤쪽 대조 desktop-promise-audit 7): 앱에 되돌리는 단추가 없다는 뜻. 작업 내역의 글과 같은 말. */
    removeTermText: '삭제한 용어는 몸소에서는 되돌릴 수 없습니다.',
    baseTerm: '기본 용어는 삭제할 수 없습니다. 끄기만 할 수 있습니다.',
  },
  // ---- components/SavedLoginsSettings.tsx, SavedLoginChoiceCard.tsx: 설정 › 저장된 로그인(2026-11-21 뒤쪽 275e017). ----
  // 낱말(저장된 로그인, 사이트 주소, 이름, 아이디, 비밀번호, 쓸 때마다 묻기)은 본부가 정해 준 말. 나머지 글은 새로 씀, 그의 확인 전.
  /**
   * 오른쪽 패널 브라우저의 확장 프로그램(2026-11-21 줄1 스냅샷 b18832c, 계약 hq/s28-done desktop-panel-extensions-s12.md). 어사이드의 퍼즐 단추 꼴
   * (aside-2·3·4: Manage all extensions · Import extensions · Installed extensions · 고정 · … · 확장 프로그램 관리 화면). 크롬 한국어판의 이름("확장 프로그램 관리", "고정").
   * 2026-11-21 새로 씀, 그의 확인 전.
   */
  extensions: {
    button: '확장 프로그램',
    manageAll: '확장 프로그램 관리',
    importMenu: '확장 프로그램 가져오기',
    installed: '설치된 확장 프로그램',
    none: '설치된 확장 프로그램이 없습니다.',
    pin: '고정',
    unpin: '고정 해제',
    more: '더 보기',
    remove: '삭제',
    enable: '사용',
    disable: '사용 중지',
    off: '사용 중지됨',
    pageTitle: '확장 프로그램',
    total: (n: number) => `확장 프로그램 ${n}개`,
    storeLabel: '크롬 웹 스토어 주소 또는 확장 프로그램 ID',
    storeExample: 'https://chromewebstore.google.com/detail/…',
    storeInstall: '설치',
    folderInstall: '폴더에서 설치',
    importTitle: '크롬·엣지에서 가져오기',
    importText: '크롬과 엣지에 있는 확장 프로그램 파일을 몸소로 복사합니다. 비밀번호와 쿠키는 가져오지 않습니다.',
    importIdNote: '가져온 확장 프로그램은 ID가 달라질 수 있어, 그 확장 프로그램의 로그인은 다시 해야 할 수 있습니다.',
    importButton: '가져오기',
    importNone: '가져올 확장 프로그램을 찾지 못했습니다.',
    browserName: { chrome: '크롬', edge: '엣지' } as Record<string, string>,
    permissionNote: '확장 프로그램은 권한에 따라 방문한 페이지와 로그인 정보를 볼 수 있습니다. 믿을 수 있는 것만 설치해 주세요.',
    loading: '불러오는 중',
    working: '하는 중',
    installedDone: '설치했습니다',
    removed: '삭제했습니다',
    removeTitle: (a: unknown) => `“${a}” 확장 프로그램을 삭제할까요?`,
    removeText: '삭제해도 원래 브라우저나 복사해 둔 파일은 지우지 않습니다.',
    close: '닫기',
    loadFailed: '확장 프로그램 목록을 읽지 못했습니다.',
    popupNone: '이 확장 프로그램에는 열 창이 없습니다.',
    popupFailed: '확장 프로그램 창을 열지 못했습니다.',
    /** 뒤쪽 오류 값마다(계약의 값). 모르는 값은 failed. cancelled 는 글 없이. timeout 은 목록을 다시 읽는다. */
    errors: {
      invalid_store_address: '크롬 웹 스토어 주소나 확장 프로그램 ID를 다시 확인해 주세요.',
      store_unavailable: '크롬 웹 스토어에서 받지 못했습니다. 잠시 뒤 다시 해 주세요.',
      store_package_invalid: '받은 파일이 올바르지 않아 설치하지 않았습니다.',
      store_signature_invalid: '서명을 확인하지 못해 설치하지 않았습니다.',
      store_id_mismatch: '요청한 확장 프로그램과 받은 파일이 달라 설치하지 않았습니다.',
      panel_unavailable: '브라우저를 연 뒤 다시 해 주세요.',
      unsupported: '이 컴퓨터에서는 확장 프로그램을 쓸 수 없습니다.',
      timeout: '끝났는지 확인하지 못해 목록을 다시 읽었습니다.',
      source_unavailable: '그 브라우저의 확장 프로그램 파일을 읽지 못했습니다.',
      failed: '하지 못했습니다. 잠시 뒤 다시 해 주세요.',
    } as Record<string, string>,
  },
  savedLogins: {
    title: '저장된 로그인',
    /** 새 컴퓨터에서는 다시 넣어야 한다는 뜻을 화면에서 알린다(계약 "WebView2와 새 컴퓨터"). */
    intro: '로그인이 풀리면 몸소가 저장된 로그인으로 다시 들어갑니다. 비밀번호는 AI에게 보내지 않고 이 컴퓨터에만 저장하므로, 새 컴퓨터에서는 저장된 로그인을 다시 넣어 주세요.',
    add: '새 로그인',
    total: (n: number) => `로그인 ${n}개`,
    empty: '저장된 로그인이 없습니다.',
    fieldSite: '사이트 주소',
    fieldName: '이름',
    fieldUsername: '아이디',
    fieldPassword: '비밀번호',
    askBeforeUse: '쓸 때마다 묻기',
    /** 고칠 때 비밀번호 칸의 안내(비워 두면 기존 비밀번호 유지). */
    keepPassword: '비워 두면 지금 비밀번호를 그대로 씁니다',
    /** 고칠 때 사이트 주소는 바꿀 수 없다(뒤쪽 origin_change_requires_new_login). */
    siteFixed: '사이트 주소를 바꾸려면 삭제한 뒤 새로 저장해 주세요.',
    /** 같은 사이트에 로그인이 둘 이상일 때만 보이는 칸(loginPath). */
    fieldLoginPath: '로그인 화면 경로',
    loginPathHint: '같은 사이트에 로그인이 여럿일 때, 이 로그인을 쓸 로그인 화면의 경로(/로 시작)를 적습니다. 비워 두면 모든 로그인 화면에서 씁니다.',
    /** 같은 사이트·경로에 로그인이 둘 이상일 때만 보이는 스위치(defaultForPath). */
    defaultForPath: '기본 계정',
    defaultHint: '같은 로그인 화면에 로그인이 여럿이면 기본 계정을 씁니다. 기본 계정이 없으면 쓸 때 어느 것을 쓸지 묻습니다.',
    save: '저장',
    cancel: '취소',
    edit: '수정',
    remove: '삭제',
    saved: '저장했습니다',
    removed: '삭제했습니다',
    loadFailed: '저장된 로그인을 읽지 못했습니다',
    saveFailed: '저장하지 못했습니다',
    removeFailed: '삭제하지 못했습니다',
    removeTitle: (a: unknown) => `“${a}” 로그인을 삭제할까요?`,
    /** 다른 목록의 삭제 확인과 같은 뜻(앱에 되돌리는 단추가 없다). */
    removeText: '삭제한 로그인은 몸소에서는 되돌릴 수 없습니다.',
    /** 뒤쪽 오류 값마다의 글. 입력한 값은 담지 않는다. */
    errors: {
      capacity_exceeded: '더 저장할 자리가 없습니다. 쓰지 않는 로그인을 삭제한 뒤 다시 해 주세요.',
      credential_store_unavailable: 'Windows 자격 증명 관리자를 쓰지 못했습니다. 잠시 뒤 다시 해 주세요.',
      invalid_origin: '사이트 주소는 https://로 시작하는 주소로 적어 주세요.',
      invalid_input: '이름은 100자, 아이디는 256자, 비밀번호는 1,024자까지 적을 수 있습니다. 로그인 화면 경로는 /로 시작합니다.',
      duplicate_label: '이 사이트에 같은 이름의 로그인이 이미 있습니다.',
      origin_change_requires_new_login: '사이트 주소를 바꾸려면 삭제한 뒤 새로 저장해 주세요.',
      default_conflict: '이 로그인 화면에는 이미 기본 계정이 있습니다.',
      not_found: '그사이 삭제된 로그인입니다. 목록을 다시 읽었습니다.',
    } as Record<string, string>,
    /** 오른쪽 패널 브라우저 주소 줄의 열쇠 단추를 누르면 뜨는 카드(2026-11-21 본부 지시, 어사이드 열쇠 창 꼴). 단추 이름은 title. 새로 씀, 그의 확인 전. */
    /** 왼쪽 위 찾기 칸(어사이드 "Search all accounts"). 이름·아이디·사이트로 모든 로그인에서 찾는다. */
    searchAll: '모든 로그인 찾기',
    /** 찾기 칸 아래 고르기(어사이드 "All accounts" 자리): 지금 사이트의 로그인만 / 모든 로그인. */
    scopeSite: '이 사이트',
    scopeAll: '모든 로그인',
    noMatch: '찾는 로그인이 없습니다.',
    /** 오른쪽 판 위 줄 왼쪽 작은 글(어사이드 "소유자 > 보관함" 자리): 저장된 로그인 › 이 컴퓨터(Windows 자격 증명 관리자에 저장). */
    onThisComputer: '이 컴퓨터',
    /** 오른쪽 판 위 줄 오른쪽 … 단추(수정·삭제). */
    more: '더 보기',
    /** 판 위 작은 회색 제목: 아이디·비밀번호 판(어사이드 "Credentials"), 몸소의 경로·쓸 때마다 묻기·기본 계정 판. */
    credentials: '로그인 정보',
    usage: '사용 설정',
    /** 입력 칸의 흐린 보기 글. */
    usernameExample: 'id@example.com',
    siteExample: 'https://example.com',
    /** 비밀번호 줄에 올리면 나오는 보기·복사(2026-11-21 S14 saved_login_reveal·saved_login_copy). */
    showPassword: '비밀번호 보기',
    hidePassword: '비밀번호 숨기기',
    copyPassword: '비밀번호 복사',
    copiedPassword: '비밀번호를 복사했습니다',
    copyPasswordFailed: '비밀번호를 복사하지 못했습니다',
    revealFailed: '비밀번호를 읽지 못했습니다',
    /** … 메뉴: 열고 채우기(saved_login_open_and_fill, 제출 단추는 누르지 않음)와 결과. */
    openFill: '열고 채우기',
    openFillDone: '로그인 정보를 채웠습니다. 로그인 단추는 직접 눌러 주세요.',
    openFillFailed: '채우지 못했습니다',
    openFillStatus: {
      human_required: '인증번호나 확인 단계가 있거나 입력 칸을 찾지 못했습니다. 직접 입력해 주세요.',
      target_changed: '그사이 페이지가 바뀌어 채우지 않았습니다.',
      panel_page_changed: '그사이 페이지가 바뀌어 채우지 않았습니다.',
      blocked: '이 페이지에는 채울 수 없습니다.',
      panel_closed: '브라우저가 닫혀 있어 채우지 못했습니다.',
      panel_busy: 'AI가 브라우저를 쓰고 있어 채우지 못했습니다.',
      human_takeover: '브라우저를 직접 쓰는 중이라 채우지 않았습니다.',
      panel_not_active: '이 채팅방의 브라우저가 아니어서 채우지 못했습니다.',
      driver_unavailable: '지금은 채울 수 없습니다. 잠시 뒤 다시 해 주세요.',
      unavailable: '지금은 채울 수 없습니다. 잠시 뒤 다시 해 주세요.',
    } as Record<string, string>,
    /** … 메뉴: 즐겨찾기(favorite). 목록·보기의 별 표시 이름도 favorite. */
    favorite: '즐겨찾기',
    unfavorite: '즐겨찾기 해제',
    favorited: '즐겨찾기에 넣었습니다',
    unfavorited: '즐겨찾기에서 뺐습니다',
    /** 왼쪽 위 거르기·순서 그림 단추(어사이드의 두 단추)와 메뉴. */
    filter: '거르기',
    filterAll: '전체',
    sort: '순서',
    sortCreated: '저장한 때',
    sortUpdated: '고친 때',
    sortName: '이름',
    sortSite: '사이트',
    /** 저장한 때·고친 때로 묶은 목록의 제목(어사이드 "This Week" 자리). 옛 항목은 때를 몰라 날짜 모름. */
    dateGroup: { today: '오늘', yesterday: '어제', week: '이번 주', month: '이번 달', unknown: '날짜 모름' } as Record<string, string>,
    dateGroupMonth: (year: number, month: number) => `${year}년 ${month}월`,
    /** 카드의 로그인 보기에서 아이디 옆 복사 단추(풍선·화면 읽기 이름)와 알림 줄. */
    copyUsername: '아이디 복사',
    copiedUsername: '아이디를 복사했습니다',
    copyUsernameFailed: '아이디를 복사하지 못했습니다',
    /** 채팅방 안에서 계정을 고르는 카드(saved-login-selection-request). 지금 있는 승인 카드 꼴. */
    choice: {
      aria: '저장된 로그인 선택',
      head: '어느 로그인을 쓸지 선택해 주세요',
      chosen: '선택했습니다',
      failed: '선택을 보내지 못했습니다. 다시 눌러 주세요',
    },
  },
  /**
   * 네트워크 길(momso server, 같은 출처) 화면의 문(2026-11-21 S19b): 처음에 서버를 확인하는 동안, 앞단 로그인(Cloudflare Access)이 풀렸을 때,
   * 서버에 닿지 못할 때(VM 이 꺼짐 등). 새로 씀, 그의 확인 전.
   */
  /** momso anywhere 의 AI 연결(채팅 전에 먼저 연결하게 이끄는 화면). 2026-11-21 새로 씀, 그의 확인 전. 제목·상태·단추·안내는 첫 실행·설정의 말을 그대로 쓴다. */
  serverAi: {
    intro: '몸소가 쓸 AI에 로그인해 주세요. 휴대폰에서 바로 할 수 있습니다.',
    startFailed: 'AI 로그인을 시작하지 못했습니다. 잠시 뒤 다시 해 주세요.',
    /** 클로드·제미나이 줄: server 가 키를 보관할 암호화 보관소를 열 수 없을 때(S55 storage_unavailable). 2026-11-22 새로 씀, 그의 확인 전. */
    keyStorageUnavailable: '키를 보관할 곳을 열지 못했습니다. 지금은 API 키를 저장할 수 없습니다.',
  },
  /**
   * 휴대폰에서 시작하기(줄2 S26 hq/s26-done 7fa9efbc1 momso-anywhere-onboarding-s26.md "화면 담당 계약", StartScreen.tsx). 2026-11-21 새로 씀, 그의 확인 전.
   * 첫 실행(FirstRun)처럼 제목 한 줄 + 흐린 설명, 단추는 아래. 홈 화면에 추가하는 법은 Apple·Google 공식 도움말의 단추 이름 그대로
   * (아이폰: support.apple.com/ko-kr/guide/iphone/iph42ab2f3a7 "공유", "홈 화면에 추가", "동작 편집", "웹 앱으로 열기", "추가",
   *  안드로이드 크롬: support.google.com/chrome/answer/9658361?hl=ko "더보기", "설치 및 바로가기 만들기", "설치").
   */
  start: {
    preparing: '준비하는 중',
    unavailable: '시작 화면을 준비하지 못했습니다. 잠시 뒤 다시 열어 주세요.',
    title: '내 Google Cloud에 몸소를 설치합니다',
    text: '설치가 끝나면 휴대폰에서 바로 몸소를 쓸 수 있습니다. 몸소의 자료와 대화는 내 Google Cloud 안에만 있습니다.',
    whatTitle: '시작하면',
    what: [
      'Google 계정으로 로그인합니다.',
      '몸소가 쓸 Google Cloud 프로젝트를 만들고 관리할 권한을 허락합니다.',
      '프로젝트와 서버, 몸소 입구를 만듭니다. 몇 분 걸립니다.',
    ],
    cost: 'Google Cloud 사용 요금은 내 결제 계정으로 청구됩니다. 서버를 꺼 두어도 디스크 같은 일부 요금은 남을 수 있습니다.',
    begin: 'Google 계정으로 시작하기',
    find: '이미 설치했다면 내 몸소 찾기',
    runningTitle: '몸소를 설치하는 중',
    runningText: '몇 분 걸릴 수 있습니다. 이 화면을 닫으면 멈추고, 다시 시작하면 멈춘 곳부터 이어서 합니다.',
    // S64 프로젝트·결제 계정 고르기. 2026-11-22 새로 씀, 그의 확인 전.
    chooseTitle: '설치할 곳 고르기',
    chooseText: '몸소를 설치할 Google Cloud 프로젝트와 사용 요금을 낼 결제 계정을 고릅니다.',
    project: 'Google Cloud 프로젝트',
    newProject: '새 프로젝트 만들기',
    billingAccount: '결제 계정',
    billingPick: '결제 계정 고르기',
    billingKept: '이 프로젝트에 이미 결제 계정이 연결되어 있으면 바꾸지 않습니다.',
    billingNone: '열린 결제 계정이 없습니다. Google Cloud에서 결제 계정을 만들거나 다시 연 뒤 처음부터 다시 해 주세요.',
    install: '설치 시작',
    chooseAgain: '다시 고르기',
    // S65 돌아온 뒤 이어 가기·지운 결과. 2026-11-22 새로 씀, 그의 확인 전.
    next: '다음',
    destroyingTitle: '몸소를 지우는 중',
    destroyingText: '몇 분 걸릴 수 있습니다. 이 화면을 닫지 말아 주세요.',
    destroyedTitle: '몸소를 지웠습니다',
    destroyedText: '몸소 서버와 그 안의 자료, 저장된 로그인을 여는 열쇠까지 지웠습니다. 프로젝트와 결제 계정은 그대로 둡니다.',
    destroyPartialTitle: '아직 남은 것이 있습니다',
    destroyPartialText: '아래 항목은 아직 지우지 못했습니다. 나중에 같은 몸소를 다시 지우면 남은 것만 확인해 지웁니다.',
    destroyItems: {
      gateway: '몸소 입구', vm: '몸소 서버', router: '라우터', firewall: '방화벽', subnet: '서브넷', network: '네트워크',
      secret_transport: '비밀값(연결)', secret_session: '비밀값(로그인 유지)', secret_vault: '저장된 로그인을 여는 열쇠',
      account_vm: '서비스 계정(서버)', account_gateway: '서비스 계정(입구)', role_stop: '역할(서버 멈춤)', role_anywhere: '역할(서버 켜기)',
      unknown: '확인하지 못한 항목',
    } as Record<string, string>,
    steps: {
      project: 'Google Cloud 프로젝트 만들기',
      billing: '결제 계정 연결',
      services: '필요한 서비스 켜기',
      privateNetwork: '전용 네트워크 만들기',
      server: '몸소 서버 만들기',
      gateway: '몸소 입구 만들기',
      gatewayReady: '입구 준비 확인',
    } as Record<string, string>,
    stepState: { waiting: '기다리는 중', working: '하는 중', done: '끝남', failed: '멈춤', pending: '나중에 다시 함' } as Record<string, string>,
    failedTitle: '설치가 멈췄습니다',
    /** 멈춘 까닭(SetupError 의 step 값을 묶음). 모르는 값은 other. */
    errors: {
      billing: '이 프로젝트에 쓸 수 있는 결제 계정이 없습니다. Google Cloud에서 결제 계정을 만들거나 다시 켠 뒤 다시 시도해 주세요.',
      billingChoice: '쓸 결제 계정을 고르지 않았습니다. 결제 계정을 고른 뒤 다시 시작해 주세요.',
      // S64. 2026-11-22 새로 씀, 그의 확인 전.
      billingAccount: '고른 결제 계정을 쓸 수 없습니다. 닫혔거나 이 계정에 권한이 없습니다. 다른 결제 계정을 골라 주세요.',
      project: '고른 프로젝트를 쓸 수 없습니다. 이 계정에 권한이 있는지 확인하거나 다른 프로젝트를 골라 주세요.',
      serverNotReady: '몸소 서버가 아직 준비되지 않았습니다. 다시 시도하면 멈춘 곳부터 다시 확인합니다.',
      gatewayNotReady: '몸소 입구가 아직 준비되지 않았습니다. 다시 시도하면 멈춘 곳부터 다시 확인합니다.',
      declined: 'Google 권한을 허락하지 않아 멈췄습니다. 다시 시도하면 다시 묻습니다.',
      expired: 'Google 로그인이 만료되었습니다. 다시 시도해 주세요.',
      permission: '이 Google 계정에는 프로젝트를 만들거나 바꿀 권한이 없습니다. 다른 계정으로 다시 시도해 주세요.',
      quota: 'Google Cloud 사용 한도에 걸렸습니다. 잠시 뒤 다시 시도해 주세요.',
      notReady: '아직 설치를 시작할 수 없습니다. 몸소 쪽 준비가 끝나지 않았습니다.',
      checkpoint: '이 브라우저에 설치 기록을 남길 수 없습니다. 사생활 보호 모드를 끄고 다시 시도해 주세요.',
      identity: 'Google 계정 정보를 확인하지 못했습니다. 다시 시도해 주세요.',
      ownership: '이미 있는 자원이 몸소가 만든 것과 맞지 않아 멈췄습니다. 바꾸지 않았습니다.',
      google: 'Google Cloud가 잠깐 응답하지 않습니다. 다시 시도하면 멈춘 곳부터 다시 확인합니다.',
      other: '설치하는 중 문제가 생겼습니다. 다시 시도하면 멈춘 곳부터 다시 확인합니다.',
    } as Record<string, string>,
    retry: '다시 시도',
    openBilling: 'Google Cloud 결제 열기',
    back: '처음으로',
    doneTitle: '몸소가 준비되었습니다',
    doneText: '아래 주소가 내 몸소입니다. 열면 Google 계정으로 한 번 로그인합니다.',
    address: '내 몸소 주소',
    copy: '주소 복사',
    copied: '주소를 복사했습니다',
    copyFailed: '주소를 복사하지 못했습니다',
    open: '몸소 열기',
    homeTitle: '홈 화면에 추가하기',
    homeText: '몸소를 연 화면에서 홈 화면에 추가하면 앱처럼 열립니다.',
    iphone: 'iPhone(Safari)',
    android: 'Android(Chrome)',
    iphoneSteps: [
      '아래 막대의 … 버튼을 누른 뒤 공유를 누릅니다. 공유 버튼이 바로 보이면 그것을 누릅니다.',
      '목록을 아래로 내려 홈 화면에 추가를 누릅니다. 보이지 않으면 맨 아래 동작 편집에서 켭니다.',
      '웹 앱으로 열기가 켜져 있는지 보고 추가를 누릅니다.',
    ],
    androidSteps: [
      '주소 표시줄 오른쪽의 더보기(⋮)를 누릅니다.',
      '설치 및 바로가기 만들기를 누릅니다.',
      '설치를 누릅니다.',
    ],
    homeNote: '홈 화면의 몸소는 브라우저와 로그인이 따로라, 처음 열 때 Google로 다시 로그인할 수 있습니다.',
    forgetNote: '주소를 잊어도 이 화면의 내 몸소 찾기로 다시 찾을 수 있습니다.',
    findingTitle: '내 몸소를 찾는 중',
    foundTitle: '내 몸소',
    foundText: '이 Google 계정으로 설치한 몸소입니다.',
    foundNone: '이 Google 계정으로 설치한 몸소를 찾지 못했습니다. 다른 계정으로 설치했거나, 막 설치했다면 잠시 뒤 다시 찾아 주세요.',
    findFailed: '내 몸소를 찾지 못했습니다. 다시 시도해 주세요.',
  },
  serverGate: {
    checking: '몸소에 연결하는 중',
    signedOutTitle: '로그인이 풀렸습니다',
    signedOutText: '다시 로그인하면 이어서 쓸 수 있습니다.',
    signIn: '다시 로그인',
    unreachableTitle: '몸소에 닿지 못했습니다',
    unreachableText: '서버가 꺼져 있거나 인터넷 연결이 끊겼을 수 있습니다. 잠시 뒤 다시 해 주세요.',
  },
  /** 고른 지 5분이 지나 뒤쪽이 거절했을 때(selection_required). 첨부·대화 가져오기·몸소 폴더가 같은 글을 쓴다. 2026-11-17 새로 씀, 그의 확인 전. */
  selectAgain: '선택한 지 오래되어 진행하지 못했습니다. 파일을 다시 선택해 주세요',
  // ---- components/ChatImportSettings.tsx: 다른 AI 에서 내보낸 대화 가져오기. 2026-11-16 새로 씀(그가 보고 고친다, 낱말 목록에 올림). ----
  chatImport: {
    title: '다른 AI의 채팅방 가져오기',
    intro: 'ChatGPT, 클로드, 제미나이에서 내보낸 파일을 선택하면 왼쪽 목록에 채팅방으로 생깁니다.',
    name: { chatgpt: 'ChatGPT', claude: '클로드', gemini: '제미나이' } as Record<string, string>,
    hint: {
      chatgpt: 'ChatGPT 설정의 데이터 내보내기로 받은 파일(zip)을 선택합니다.',
      claude: '클로드 설정의 데이터 내보내기로 받은 파일(zip)을 선택합니다.',
      // 2026-11-17 제미나이 가져오기가 생김. 새로 씀, 그의 확인 전.
      gemini: '구글 데이터 내보내기로 받은 제미나이 파일(zip 또는 JSON)을 선택합니다.',
    } as Record<string, string>,
    pick: '파일 선택',
    fileKind: '내보낸 채팅방 파일',
    working: '가져오는 중',
    done: (n: number) => `채팅방 ${n}개를 가져왔습니다.`,
    duplicate: (n: number) => `${n}개는 이미 있어 건너뛰었습니다.`,
    updated: (n: number) => `${n}개는 새로 이어진 메시지를 추가했습니다.`,
    failed: '채팅방을 가져오지 못했습니다',
    // 2026-11-16 미리 세기·진행·그만두기. 새로 씀, 그의 확인 전.
    counting: '파일 안의 채팅방을 세는 중',
    preview: (n: number) => `채팅방 ${n}개가 들어 있습니다.`,
    previewAlready: (n: number) => `${n}개는 이미 가져온 적이 있어 새로 이어진 말만 더합니다.`,
    previewUnreadable: (n: number) => `${n}개는 읽을 수 없어 건너뜁니다.`,
    previewRange: (a: string, b: string) => `${a}부터 ${b}까지의 기록입니다.`,
    previewDay: (a: string) => `${a}의 기록입니다.`,
    previewEmpty: '이 파일에는 가져올 채팅방이 없습니다.',
    /** 제미나이: 뒤쪽 groupingNotice·guidance 가 있을 때(2026-11-18). 뒤쪽 글 대신 이 글. 새로 씀, 그의 확인 전. */
    geminiGrouped: '제미나이 기록은 채팅방 구분이 없어, 같은 날 30분 안에 이어진 기록을 한 채팅방으로 묶었습니다.',
    geminiJsonGuide: '구글 테이크아웃에서 \'내 활동\'을 JSON 형식으로 골라 내보낸 파일을 선택해 주세요.',
    start: '가져오기',
    cancel: '취소',
    progress: (done: number, total: number) => `가져오는 중 ${done}/${total}`,
    cancelling: '취소하는 중',
    cancelled: '가져오기를 취소했습니다. 이번에 가져온 채팅방은 남기지 않았습니다.',
    reason: {
      invalid_format: '이 파일에서 채팅방을 찾지 못했습니다. 내보내기로 받은 파일인지 확인해 주세요.',
      unsupported_format: '이 파일에서 채팅방을 찾지 못했습니다. 내보내기로 받은 파일인지 확인해 주세요.',
      archive_limit: '파일이 너무 커서 가져오지 못했습니다.',
      file_error: '파일을 열지 못했습니다.',
      // 2026-11-17 제미나이. 새로 씀, 그의 확인 전.
      gemini_json_required: 'HTML 파일은 가져올 수 없습니다. 구글에서 내보낼 때 형식을 JSON 으로 골라 주세요.',
      gemini_file_too_large: '파일이 64MB보다 커서 가져오지 못했습니다.',
    } as Record<string, string>,
  },
  // ---- components/FirstLaunch.tsx ----
  firstLaunch: {
    text1: '몸을 소중히 여기는 실천, ',
    text2: '몸소',
    text3: '시작하기 ',
  },
  // ---- components/FirstRun.tsx: 첫 실행의 이름 받기와 AI 연결. 2026-11-16 새로 쓴 글(본부·그의 확인 전, 낱말 목록에 올림). ----
  firstRun: {
    nameTitle: '사용자 이름을 정해 주세요',
    nameText: '영어로 적습니다. 영문자, 숫자, -, _, . 를 쓸 수 있습니다.',
    /** 이름 칸 아래: 이 이름이 어디에 쓰이는지(2026-11-17 새로 씀, 그의 확인 전). 적는 동안 momso-적은이름 이 보인다. */
    nameUse: (a: unknown) => `이 이름으로 내 백업 공간이 만들어집니다(${a}).`,
    nameUseSample: 'momso-이름',
    nameLabel: '사용자 이름',
    nameChars: '영문자, 숫자, -, _, . 만 쓸 수 있습니다.',
    nameRejected: '쓸 수 없는 이름입니다. 다른 이름을 적어 주세요.',
    nameSaveFailed: '이름을 저장하지 못했습니다',
    next: '다음',
    back: '이전',
    aiTitle: 'AI를 하나 이상 연결해 주세요',
    /** 이미 하나 이상 연결됐을 때의 제목과 안내(2026-11-18). 새로 씀, 그의 확인 전. */
    aiTitleReady: 'AI가 연결되었습니다',
    aiTextReady: '다른 AI도 연결하거나 바로 시작할 수 있습니다.',
    aiText: (a: unknown, b: unknown, c: unknown) => `${a}, ${b}, ${c} 가운데 하나 이상 연결해야 몸소를 시작할 수 있습니다.`,
    checking: 'AI 연결 상태를 확인하고 있습니다',
    checkFailed: 'AI 연결 상태를 확인하지 못했습니다.',
    retry: '다시 시도',
    start: '시작하기',
  },
  // ---- components/MediaView.tsx ----
  mediaView: {
    /** 화면 이름(2026-11-18 그가 정함: "미디어 라 적지 말고 '사진과 동영상'이라고 적어"). 오른쪽 패널의 탭 이름도 이것. */
    aria1: '사진과 동영상',
    aria2: '동영상',
    aria3: '사진',
    aria4: '크게 보기',
    aria5: '이전',
    aria6: '다음',
    aria7: '닫기',
    /** 2026-11-17 읽는 동안·없을 때·읽지 못했을 때(본부 결정, 그가 보고 고친다). */
    /** 2026-11-18 사진과 동영상 화면(AI 생성 도구들의 꼴): 거르기·찾기·버전·프롬프트. 새로 씀, 그의 확인 전("프롬프트", "버전"은 그의 말). */
    all: '전체',
    kindLabel: '보기',
    searchPlaceholder: '프롬프트로 찾기',
    clearSearch: '찾는 말 지우기',
    noMatch: '찾는 사진과 동영상이 없습니다',
    versions: (n: number) => `버전 ${n}개`,
    version: (n: number) => `버전 ${n}`,
    versionHow: { regenerated: '같은 프롬프트로 다시 만듦', edited: '고침' } as Record<string, string>,
    copyPrompt: '프롬프트 복사',
    /** 도구가 알리지 않아 자동 등록된 생성물(프롬프트가 빈 값, 2026-11-18 뒤쪽 f42672f). 새로 씀, 그의 확인 전. */
    noPrompt: '프롬프트 기록이 없습니다',
    /** 다시 만들기로 새 판이 기록됐을 때(chat-media-regeneration recorded). 새로 씀, 그의 확인 전. */
    regenerated: '새 버전을 사진과 동영상에 넣었습니다',
    /** 내려받기(media_save_as, 2026-11-18 뒤쪽 C52). 새로 씀, 그의 확인 전. */
    saveAs: '내려받기',
    saved: '파일을 저장했습니다',
    saveFailed: {
      destination_exists: '같은 이름의 파일이 있어 저장하지 않았습니다. 다른 이름을 골라 주세요.',
      invalid_destination: '이 자리에는 저장할 수 없습니다. 다른 폴더를 골라 주세요.',
      asset_not_found: '원래 파일을 찾지 못했습니다',
      asset_changed: '원래 파일이 바뀌어 저장하지 않았습니다',
      other: '파일을 저장하지 못했습니다',
    } as Record<string, string>,
    /** "다시 만들기"(2026-11-18 그의 결정: 원래 채팅방으로). 원래 채팅방이 없을 때 단추 풍선. 새로 씀, 그의 확인 전. */
    regenerate: '다시 만들기',
    regenerateNoChat: '만든 채팅방이 없어 다시 만들 수 없습니다',
    copied: '프롬프트를 복사했습니다',
    copyFailed: '프롬프트를 복사하지 못했습니다',
    loading: '사진과 동영상 불러오는 중',
    empty: '사진과 동영상이 없습니다',
    failed: '사진과 동영상을 읽지 못했습니다',
  },
  // ---- components/MessageRail.tsx ----
  messageRail: {
    aria1: '내 메시지',
    aria2: (a: unknown) => `내 메시지 ${a}번째로 가기`,
  },
  // ---- components/SearchDialog.tsx ----
  searchDialog: {
    text1: '전체',
    text2: '할 일',
    text3: (a: unknown) => `새 ${a}`,
    text4: (a: unknown) => `최근 ${a}`,
    aria1: '닫기',
    aria2: '검색 범위',
    /** 2026-11-16 바꿈(원자료 찾기를 더함, 그의 확인 전). 예전 글은 "채팅방, 문서, 할 일 찾기". */
    hint2: (a: unknown, b: unknown, c: unknown) => `${a}, ${b}, ${c}, 할 일 찾기`,
    aria3: '검색 결과',
    aria4: '키 안내',
    label1: '선택',
    label2: '열기',
    label3: '종류 바꾸기',
    /** 2026-11-17 찾는 동안·찾은 것이 없을 때·검색이 실패했을 때(본부 결정, 그가 보고 고친다). */
    searching: '검색하는 중',
    empty: '검색 결과가 없습니다',
    failed: '검색하지 못했습니다',
  },
  // ---- components/SettingsView.tsx ----
  settingsView: {
    text1: '나에 대해',
    /** 설정 쪽 이름. 2026-11-16 김성균 개발자가 정함("개인정보라는 글자는 이제 필요 없는거지? 저기는 그냥 3사 연결하는것만"). */
    text4: 'AI 연결',
    text5: '연결과 백업',
    text6: '저장공간',
    text7: '앱 정보',
    text11: '설정',
    aria1: '설정 닫기',
    aria2: '설정 항목',
    text16: 'AI가 나를 어떻게 부르고, 무엇을 알고, 어떻게 답할지 적어 둡니다.',
    text51: '지금 버전과 업데이트를 확인합니다.',
  },
  // ---- components/UsageMeter.tsx ----
  usageMeter: {
    /** 2026-11-16 새로 씀(그의 확인 전): 남은 양을 모를 때 단추 풍선에 이름 뒤로 붙는 까닭(usage_status 의 reason 값, G18). */
    reason: {
      checking: '사용량을 확인하는 중입니다.',
      not_installed: '프로그램이 설치되어 있지 않습니다.',
      not_connected: '연결되어 있지 않습니다.',
      api_key: 'API 키로 쓰는 중이라 요금제의 남은 양이 없습니다.',
      no_quota_api: '이 연결 방식은 남은 양을 알려 주지 않습니다.',
      fetch_failed: '남은 양을 읽지 못했습니다.',
      // 2026-11-17 화면이 붙인 까닭: 저장한 API 키를 아직 확인하는 중. 새로 씀, 그의 확인 전.
      key_verifying: 'API 키를 확인하는 중입니다.',
    } as Record<string, string>,
    text1: '5시간',
    text2: '1주',
    /** 2026-11-17 본부 결정: 회사가 기간을 알려 주지 않은 창(코덱스 primary·secondary, 클로드 session)은 기간을 지어내지 않는다. 새로 씀, 그의 확인 전. */
    windowFirst: '첫째 한도',
    windowSecond: '둘째 한도',
    windowSession: '사용 한도',
    text3: '하루',
    text4: '한 달',
    text5: '일',
    text6: '시간',
    text7: '분',
    aria1: '사용량',
    aria2: (a: unknown) => `${a} 사용량`,
    text8: (a: unknown, b: unknown) => `${a} ${b}% 남음`,
    text9: '불러오는 중',
    text10: (a: unknown) => `${a}에 확인`,
    tip1: '사용량 고리에 보이는 한도',
    text11: (a: unknown) => `${a}% 남음`,
    text12: (a: unknown) => `${a}에 다시 참`,
  },
  // ---- components/UserOwnedBackupSettings.tsx ----
  userOwnedBackupSettings: {
    text1: '설정과 전사 보정 규칙',
    text2: '암호화된 기록 파일',
    text3: '백업 순서와 연결 정보',
    text4: '공간 준비 필요',
    text5: '세 계정 연결 뒤 사용자 소유 백업 공간을 만듭니다.',
    text6: '첫 백업 준비됨',
    text7: '로컬 기록을 처음으로 원격 공간에 보존할 수 있습니다.',
    text8: '최신',
    text9: '로컬 기록과 마지막 원격 백업이 같습니다.',
    text10: '로컬 변경 있음',
    text11: '마지막 백업 뒤 이 기기의 기록이 바뀌었습니다.',
    text12: '원격 변경 있음',
    text13: '다른 기기에서 더 새 백업을 만들었습니다.',
    text14: '원격이 과거',
    text15: '원격 머리가 이 기기가 기억한 백업보다 뒤에 있어 자동 덮어쓰지 않습니다.',
    text16: '서로 다른 변경',
    text17: '로컬과 원격이 각각 바뀌어 자동 백업과 복원을 멈췄습니다.',
    text18: '원격 확인 실패',
    text19: '연결 상태와 네트워크를 확인한 뒤 다시 비교해 주세요.',
    text20: '작업 대기 중',
    text21: '로컬 기록 확인 중',
    text22: '원격 백업 비교 중',
    text23: '암호화 백업 파일 만드는 중',
    text24: '암호화 파일 업로드 중',
    text25: '백업 연결 정보 기록 중',
    text26: '복원 계획 다시 확인 중',
    text27: '암호화 파일 다운로드 중',
    text28: '백업 파일 여는 중',
    text29: '복원 내용 검증 중',
    text30: '로컬 사본 교체 중',
    text31: '완료',
    text32: '중단됨',
    text33: '실패',
    text34: '작업을 중단했습니다',
    text35: '작업을 완료하지 못했습니다. 연결 상태를 확인한 뒤 다시 시도해 주세요',
    text36: '중단 요청을 보냈습니다',
    text37: 'GitHub, Cloudflare R2, Neon 백업 공간을 준비했습니다',
    text38: '이 GitHub 계정에서 기존 몸소 백업을 찾지 못했습니다',
    text39: '기존 백업 공간을 연결했습니다. 복구 키를 가져온 뒤 원격 내용을 확인해 주세요',
    text40: '세 원격 공간에 백업을 완료했습니다',
    text41: (a: unknown) => `원격 백업을 복원했습니다. 복원 전 사본: ${a}`,
    text42: '복구 키를 Windows 자격 증명 관리자에 저장했습니다',
    text43: '복구 키의 외부 보관을 확인했습니다',
    text44: '계정 연결',
    text45: '토큰은 각 공식 연결 도구가 관리하며 몸소 데이터베이스에 저장하지 않습니다.',
    aria1: '연결 상태 새로고침',
    text46: '확인 중',
    text47: '로그인됨',
    text48: '로그아웃',
    text49: '미설치',
    text50: '처리 중',
    text51: '로그인',
    text52: '설치하고 로그인',
    aria2: '연결 상태 확인 중',
    text53: '백업 공간',
    text54: '사용자 계정 안에 private 저장소, 암호화 파일 버킷, 연결 정보 DB를 만듭니다.',
    text55: '기존 백업 찾기',
    text56: '새 공간 준비',
    text57: 'Neon 조직',
    text58: '조직 선택',
    text59: '로컬 기록과 원격 백업 상태를 확인하고 있습니다',
    aria3: '기존 백업 목록',
    text60: '연결만으로 이 기기의 기록은 바뀌지 않습니다. 실제 복원 전에 내용을 다시 비교합니다.',
    text61: (a: unknown) => `마지막 메타데이터 변경 ${a}`,
    text62: '연결',
    text63: '백업과 복원',
    text64: '로컬 기록이 정본이며, 원격본과 선후관계를 확인한 뒤에만 이동합니다.',
    text65: '원격 확인',
    text66: '지금 백업',
    text67: (a: unknown, b: unknown) => `${a}개 기록 · ${b}개 파일`,
    text68: (a: unknown) => `원격 백업 ${a}`,
    text69: (a: unknown, b: unknown) => `${a}개 기록과 ${b}개 파일을 다운로드합니다.`,
    text70: '현재 로컬 사본을 먼저 보존한 뒤 원격 기록으로 복원합니다',
    text71: '취소',
    text72: '원격 백업 다운로드',
    text73: '백업 작업 진행 중',
    text74: (a: unknown, b: unknown) => `${a} / ${b} 단계`,
    text75: '현재 단계를 준비하고 있습니다',
    text76: '중단',
    text77: '복구 키',
    text78: '새 기기에서 암호화된 기록 파일을 열 때 필요합니다. 앱 밖에 보관한 뒤 같은 키를 다시 입력해야 백업할 수 있습니다.',
    text79: '보관 확인됨',
    text80: '확인 필요',
    text81: '키 확인',
    text82: '키 만들기',
    aria4: '복구 키 복사',
    /** 2026-11-16 바꾸고 더함(그의 확인 전): 설정의 복구 키는 평소 가려 두고 "보기"를 눌러야 보인다(그의 말 "당연히 그렇게 해야지"). */
    aria5: '복구 키 가리기',
    keyShow: '보기',
    keyHide: '가리기',
    keyShowAria: '복구 키 보기',
    hint1: '보관한 복구 키',
    text83: '보관 확인',
    text84: '기존 키 가져오기',
  },
  // ---- components/UserProfileSettings.tsx ----
  userProfileSettings: {
    text1: '저장했습니다',
    text2: '저장하지 못했습니다',
    aria1: '나에 대해',
    text3: (a: unknown, b: unknown, c: unknown) => `${a}, ${b}, ${c} 모두 이 내용을 보고 답합니다.`,
    text4: '어떻게 불러 드릴까요?',
    hint1: '지민 선생님',
    text5: 'AI가 알아 두면 좋은 나에 대한 것',
    hint2: '하타와 빈야사를 가르칩니다. 저녁 수업에는 처음 오는 분이 많고, 어깨나 허리가 불편한 수련생이 자주 옵니다.',
    text6: 'AI가 이렇게 답해 주면 좋겠습니다',
    hint3: '짧고 쉬운 말로, 존댓말로 답해 주세요. 동작 이름은 한국어와 산스크리트어를 같이 적어 주세요.',
    text7: '저장',
    /** 2026-11-17 저장한 내용을 읽는 동안·읽지 못했을 때(본부 결정). */
    loading: '저장한 내용 불러오는 중',
    failed: '저장한 내용을 읽지 못했습니다',
  },
  // ---- components/WindowTitlebar.tsx ----
  windowTitlebar: {
    aria1: '창 제목 표시줄',
    label1: '목록 표시',
    label2: '목록 숨기기',
    tip1: '최소화',
    tip2: '이전 크기로 복원',
    tip3: '최대화',
    tip4: '닫기',
  },
  // ---- components/ZetPanel.tsx ----
  zetPanel: {
    /** 오른쪽 패널 위의 두 단추(문서·브라우저)와 문서에서 목록으로 돌아가는 줄. 2026-11-18 새로 씀, 그의 확인 전. */
    browser: '브라우저',
    backToList: '목록',
    /** 문서 화면의 폴더(2026-11-18). 새로 씀, 그의 확인 전. */
    folders: '폴더',
    folderPath: '지금 폴더',
    folderEmpty: '이 폴더에 문서가 없습니다',
    folderCount: (n: number) => `문서 ${n}개`,
    folderExpand: (a: unknown) => `${a} 펼치기`,
    folderCollapse: (a: unknown) => `${a} 접기`,
    /** 폴더 만들기·이름 바꾸기·옮기기(2026-11-18 뒤쪽 1568490). 새로 씀, 그의 확인 전. */
    newFolder: '새 폴더',
    newFolderName: '새 폴더',
    rename: '이름 바꾸기',
    renameLabel: (a: unknown) => `${a} 새 이름`,
    moveTo: '다른 폴더로 옮기기',
    moveTitle: '어느 폴더로 옮길까요?',
    moveText: (a: unknown) => `“${a}”${josa(a, '을', '를')} 옮길 폴더를 고르세요.`,
    moved: (a: unknown, b: unknown) => `“${a}”${josa(a, '을', '를')} ${b}${josa(b, '으로', '로')} 옮겼습니다`,
    itemMenu: (a: unknown) => `${a} 메뉴`,
    folderBadName: '폴더 이름에는 / 나 \ 를 쓸 수 없고, 앞뒤에 빈칸을 둘 수 없습니다',
    folderFailed: '폴더를 바꾸지 못했습니다',
    text1: '요약',
    text2: '보정된 전사',
    text3: '노트',
    text4: '초안',
    text5: '확정',
    text6: '이 문서가 요약한 문서',
    text7: '이 문서의 바탕이 된 문서',
    text8: '이 문서가 이어받은 문서',
    text9: '이 문서가 가리키는 문서',
    text10: '이 문서가 대신하는 문서',
    text11: '이 문서를 요약한 문서',
    text12: '이 문서에서 만들어진 문서',
    text13: '이 문서를 이어받은 문서',
    text14: '이 문서를 가리키는 문서',
    text15: '이 문서를 대신하는 문서',
    text16: '직접 작성',
    text17: '문서 목록을 읽지 못했습니다',
    text18: '주소를 열지 못했습니다',
    aria1: '문서 패널 너비',
    aria2: '열린 문서',
    aria3: '저장하지 않은 내용 있음',
    aria4: (a: unknown) => `${a} 닫기`,
    tip1: '닫기',
    aria5: '문서 선택',
    title1: '저장하지 않은 내용이 있습니다',
    text19: (a: unknown) => `"${a}" 글에서 수정한 내용을 아직 저장하지 않았습니다.`,
    text20: '계속 수정',
    text21: '저장 안 함',
    text22: '저장',
    text23: '저장했습니다',
    text24: '이전 버전으로 되돌렸습니다',
    text25: '다시 불러오기',
    text26: '문서 목록 불러오는 중',
    /** 2026-11-17 오른쪽 패널 "이 채팅방"을 읽는 동안·읽지 못했을 때(본부 결정, 그가 보고 고친다). */
    chatItemsLoading: '이 채팅방에서 다룬 것 불러오는 중',
    chatItemsFailed: '이 채팅방에서 다룬 것을 읽지 못했습니다',
    text27: '아직 문서가 없습니다',
    text28: '문서 찾기',
    text29: '찾는 문서가 없습니다',
    text30: '본문',
    text31: '연결된 문서',
    text32: '이전 버전',
    text33: '문서를 읽지 못했습니다',
    text34: '연결된 문서를 읽지 못했습니다',
    text35: '이전 버전을 읽지 못했습니다',
    text36: '제목을 적어 주세요',
    text37: '요약을 적어 주세요',
    text38: (a: unknown) => `저장하지 못했습니다. ${a}`,
    text39: '문서 불러오는 중',
    aria6: '제목',
    text40: '취소',
    title2: '이 문서가 그사이 바뀌었습니다',
    text41: '수정하는 동안 이 문서가 다른 곳에서 바뀌었습니다. 바뀐 문서를 불러오면 내가 수정한 내용이 사라지고, 내 글로 덮어쓰면 그사이 바뀐 내용이 사라집니다.',
    text42: '바뀐 문서 불러오기',
    text43: '내가 수정한 것으로 덮어쓰기',
    title3: '수정한 내용을 버릴까요?',
    text44: '저장하지 않은 내용은 사라집니다.',
    aria7: '문서 보기',
    text45: '수정',
    text46: '본문이 비어 있습니다',
    text47: '연결된 문서 불러오는 중',
    text48: '이전 버전 불러오는 중',
    text49: '저장한 버전이 아직 없습니다',
    text50: (a: unknown) => `당시 제목: ${a}`,
    text51: (a: unknown) => `다른 문서 ${a}개와 함께 저장`,
    text52: '종류',
    text53: '상태',
    text54: '생성일',
    text55: '수정일',
    text56: '지금은 없는 문서',
    text57: (a: unknown) => `이름을 알 수 없는 ${a}`,
    text58: '연결된 문서가 없습니다',
    text59: '문서',
    text60: '이전 버전 목록',
    text61: '되돌리기',
    text62: (a: unknown) => `${a}에 저장한 버전`,
    text63: '이 버전은 제목과 요약을 읽을 수 없습니다. 파일에 적힌 그대로 보여 줍니다.',
    text64: '이전 버전으로 되돌리지 못했습니다',
    text65: '저장하지 않은 내용이 사라집니다',
    text66: (a: unknown) => `이 글에는 저장하지 않은 내용이 있습니다. ${a}에 저장한 버전으로 되돌리면 그 내용은 사라집니다. 그래도 되돌릴까요?`,
    text67: '이 버전으로 되돌릴까요?',
    text68: (a: unknown) => `${a}에 저장한 버전의 내용으로 문서가 바뀝니다.`,
    text69: '취소',
    text70: '그래도 되돌리기',
    text71: (a: unknown) => `${a}을 읽지 못했습니다`,
    text72: (a: unknown) => `${a} 불러오는 중`,
    text73: '누가',
    text74: '모델',
    text75: '시간',
    text76: '읽은 파일',
    text77: '수정한 파일',
  },
  // ---- runtime.ts ----
  runtime: {
    text1: '이 기기의 몸소 작업공간',
    text2: '로컬 구조 데이터',
    text3: '로컬 원자료 폴더',
    text4: '로컬 내보내기 폴더',
    text5: '로컬 백업 폴더',
    text6: '로컬 진단 로그',
    text24: '복구 키는 설치된 몸소 앱에서 확인할 수 있습니다',
    text25: '복구 키는 설치된 몸소 앱에서 저장할 수 있습니다',
  },
}

export type Words = typeof ko

type DeepPartial<T> = T extends (...args: never[]) => unknown ? T : T extends object ? { [K in keyof T]?: DeepPartial<T[K]> } : T

// 영어판: 지금은 비워 둔다(영어판을 만들 때 ko 와 같은 열쇠로 채운다). 앞서 정해 둔 몇 낱말만 있다.
const en: DeepPartial<Words> = {
  chatRoom: 'Chat',
  document: 'Document',
  rawMaterial: 'Raw material',
  workLog: 'Work log',
  appFolder: 'momso folder',
  group: 'Group',
  search: 'Search',
  provider: { codex: 'Codex', claude: 'Claude', gemini: 'Gemini' },
  mediaView: { aria1: 'Media' },
  tip: { home: 'Chat', documents: 'Documents', media: 'Media', settings: 'Settings', back: 'Go back', forward: 'Go forward' },
}

export const WORDS = { ko, en }

/** 지금 화면의 낱말(한글판). */
export const W = WORDS.ko

// 채팅방을 모은 것: "프로젝트"(2026-11-16 김성균 개발자가 정함).
export const GROUP_WORD = W.group

/** 모델 화면 이름 안의 계열 낱말을 한글로(표에 있는 것만). 모양을 다시 짤 수 없는 이름에만 쓴다. */
export function localizeModelLabel(label: string) {
  return label.replace(/[A-Za-z]+/g, (word) => W.family[word.toLowerCase()] ?? word)
}

/**
 * 모델 이름을 한 가지 모양으로(SCREEN_RULES 3절 "모델 이름은 한 가지 모양으로 적는다").
 * 2026-11-16 김성균 개발자가 정함(사진 495 의 "나"): 이름이 주인공, 판 번호는 바로 뒤에 작고 흐리게. 단추에는 이름만,
 * 올리면 "솔 · GPT-6.1". 회사가 준 이름을 "머리말 / 숫자 / 계열 이름"으로 갈라 다시 짠다:
 *   머리말이 있으면 이름 "솔", 판 번호 "GPT-6.1"(GPT-6.1-Sol, GPT-6.1 Terra, GPT-6-Luna 모두 같은 모양),
 *   머리말이 없으면 이름 "페이블", 판 번호 "5.2".
 * 이 모양으로 가를 수 없는 이름은 계열 낱말만 한글로 바꿔 그대로 둔다(parts 가 null).
 */
export type ModelNameParts = { prefix: string | null; version: string; name: string }

const PREFIXES = new Set(['gpt'])

export function modelNameParts(label: string): ModelNameParts | null {
  const text = label.trim()
  // 머리말-숫자-이름: "GPT-6.1-Sol", "GPT-6.1 Terra", "GPT 6 Luna"
  const withPrefix = /^([A-Za-z]+)[-\s]?(\d+(?:\.\d+)*)[-\s]+([A-Za-z]+)$/.exec(text)
  if (withPrefix && PREFIXES.has(withPrefix[1].toLowerCase())) {
    return { prefix: withPrefix[1].toUpperCase(), version: withPrefix[2], name: familyWord(withPrefix[3]) }
  }
  // 이름-숫자: "Fable 5.2", "Opus 5.5"
  const nameFirst = /^([A-Za-z]+)[-\s]+(\d+(?:\.\d+)*)$/.exec(text)
  if (nameFirst && !PREFIXES.has(nameFirst[1].toLowerCase())) return { prefix: null, version: nameFirst[2], name: familyWord(nameFirst[1]) }
  // 클로드 실제 모델 id(답에 저장되는 값): "claude-haiku-5-5", "claude-opus-5", 날짜가 붙은 "claude-fable-5-2-20261101"
  const claudeId = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/i.exec(text)
  if (claudeId) return { prefix: null, version: claudeId[3] ? `${claudeId[2]}.${claudeId[3]}` : claudeId[2], name: familyWord(claudeId[1]) }
  return null
}

function familyWord(word: string) {
  return W.family[word.toLowerCase()] ?? word
}

/** 이름과 판 번호(그릴 때 둘을 따로 쓴다). 가를 수 없는 이름은 판 번호가 null. */
export function modelNameView(label: string): { name: string; version: string | null } {
  const parts = modelNameParts(label)
  if (!parts) return { name: localizeModelLabel(label.trim()), version: null }
  return { name: parts.name, version: parts.prefix ? `${parts.prefix}-${parts.version}` : parts.version }
}

/** 글로만 적어야 하는 자리(올렸을 때 뜨는 글, 화면 읽기 도구)의 모델 이름: "솔 · GPT-6.1", "페이블 · 5.2". */
export function modelDisplayName(label: string) {
  const { name, version } = modelNameView(label)
  return version ? `${name} · ${version}` : name
}

/** 입력 칸 아래 단추처럼 이름만 적는 자리: "솔", "페이블". */
export function modelShortName(label: string) {
  return modelNameView(label).name
}
