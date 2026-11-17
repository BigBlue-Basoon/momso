// 화면 낱말 파일(SCREEN_RULES 3절). 화면에 보이는 글은 모두 여기에 있다. 한글판(ko)과 영어판(en)을 나란히 둔다.
// 지금 앱은 한글판(W = WORDS.ko)을 쓴다. 영어판은 아직 채우지 않는다(나중에 같은 열쇠에 영어를 넣으면 된다).
//
// - 한글로 옮기는 말은 김성균 개발자가 정한 것만 적는다(SCREEN_RULES 3절 표). 표에 없는 회사 표기는 그대로 둔다.
// - 백엔드가 주는 영어 값(id)은 열쇠로 쓰고, 화면은 이 파일에서 그 열쇠의 글을 찾아 보인다(permission, effort).
// - 아래 "화면마다" 칸은 그 글이 쓰이는 소스 파일별로 묶었다. 값이 함수인 것은 ${a} 자리에 이름·숫자 같은 값이 들어간다.
// - 한 곳만 고치면 그 글이 쓰이는 화면 전체가 바뀐다. 화면에 보이는 글을 코드에 새로 박지 않는다.
// - 김성균 개발자가 보는 목록: momso-public-prep/ui-words-ko.md (이 파일에서 뽑았다).

/** 앞 낱말의 끝에 받침이 있으면 첫째, 없으면 둘째 조사(을/를). 값이 바뀌어도 문장이 맞게. */
function josa(word: unknown, withFinal: string, without: string) {
  const text = String(word)
  const last = text.charCodeAt(text.length - 1)
  if (last < 0xac00 || last > 0xd7a3) return without
  return (last - 0xac00) % 28 ? withFinal : without
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
  tip: { home: '대화하기', documents: '문서', media: '미디어', settings: '설정', back: '뒤로 가기', forward: '앞으로 가기' },

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
    githubNotice: '수업 글과 채팅방이 내 GitHub 비공개 저장소에 올라간다',
    repoLabel: '저장소',
    connect: '연결',
    push: '업로드',
    pull: '다운로드',
    disconnect: '연결 해제',
    account: 'GitHub 계정',
    lastPushed: '마지막으로 올린 때',
    ahead: (a: unknown) => `업로드할 것 ${a}`,
    behind: (a: unknown) => `다운로드할 것 ${a}`,
    githubDisconnectTitle: 'GitHub 연결을 해제할까요?',
    githubDisconnectText: 'GitHub의 저장소와 문서는 지워지지 않습니다.',
    storeTitle: '오브젝트 스토리지',
    storeWhat: '원자료',
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
    cancel: '그만두기',
    done: '끝났습니다',
    failed: '하지 못했습니다',
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
    cancel: '그만두기',
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
    removeChip: (a: unknown) => `${a} 빼기`,
    /** 보낸 말 위에 붙는 표시의 화면 읽기 이름. */
    usedLabel: (a: unknown) => `스킬: ${a}`,
    /** 설정 쪽 */
    settingsIntro: '자주 시키는 일을 한 번 적어 두고, 입력창에서 / 를 쳐서 불러 씁니다.',
    empty: '스킬은 자주 시키는 일을 한 번 적어 두는 것입니다. 입력창에서 / 를 치면 선택해 쓸 수 있습니다.',
    add: '새 스킬',
    builtin: '처음부터 있던 것',
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
    cancel: '그만두기',
    remove: '삭제',
    back: '스킬 목록',
    removeTitle: (a: unknown) => `“${a}” 스킬을 삭제할까요?`,
    removeText: '삭제한 스킬은 되돌릴 수 없습니다.',
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
    unavailableTag: '쓸 수 없음',
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
    removeText: '이 컴퓨터에서 지워지며 되돌릴 수 없습니다.',
    removeFailed: '영구 삭제하지 못했습니다',
    cancel: '그만두기',
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
    chatEmpty: '이 채팅방에서 다룬 문서와 미디어, 첨부파일이 여기에 모입니다.',
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
    settingsIntro: '삭제한 원자료는 첨부파일과 미디어에 보이지 않습니다. 되돌리면 다시 보입니다.',
    empty: '삭제한 원자료가 없습니다.',
    loading: '삭제한 원자료 불러오는 중',
    loadFailed: '삭제한 원자료를 읽지 못했습니다',
    usedCount: (a: unknown) => `쓰던 곳 ${a}개`,
    purgeFailed: '완전히 삭제하지 못했습니다',
    /** 완전히 삭제를 시작해 이름이 지워진 원자료의 줄 이름. */
    purgedName: '완전히 삭제한 원자료',
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
    checking: '쓰는 곳 확인하는 중',
    checkFailed: '쓰는 곳을 확인하지 못해 삭제하지 않았습니다',
    usedBy: (a: unknown) => `이 원자료를 쓰는 곳: ${a}.`,
    unused: '이 원자료를 쓰는 문서나 채팅방이 없습니다.',
    documents: (a: unknown) => `문서 ${a}개`,
    receipts: (a: unknown) => `작업 내역 ${a}개`,
    chats: (a: unknown) => `채팅방 ${a}개`,
    records: (a: unknown) => `예전 기록 ${a}개`,
    more: (a: unknown) => `외 ${a}개`,
    unnamedDocument: '이름 없는 문서',
    trashText: '삭제해도 30일 동안 되살릴 수 있습니다.',
    changed: '쓰는 곳이 바뀌어 다시 확인했습니다.',
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
    text3: '다시 켜서 업데이트 마치기',
    text4: '업데이트 받는 중',
    text5: (a: unknown) => `업데이트${a}`,
  },
  // ---- App.tsx ----
  app: {
    /** 몸소 브라우저에서 선생님이 직접 해야 할 때(browser-human-required 의 reason). 채팅방 끝 한 줄. 2026-11-16 새로 씀, 그의 확인 전. */
    browserHuman: {
      login: '브라우저 창에서 직접 로그인해 주세요.',
      password: '브라우저 창에서 비밀번호를 직접 넣어 주세요.',
      one_time_code: '브라우저 창에서 인증 번호를 직접 넣어 주세요.',
      captcha: '브라우저 창에서 사람인지 확인하는 문제를 직접 풀어 주세요.',
      verification: '브라우저 창에서 본인 확인을 직접 해 주세요.',
    } as Record<string, string>,
    browserHumanAfter: '끝나면 이 채팅방에 알려 주세요.',
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
    text9: '말을 저장하지 못했습니다',
    text10: '말을 보내지 못했습니다',
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
    } as Record<string, (a: unknown) => string>,
    /** 회사가 다시 쓸 수 있는 때를 주었을 때 위 줄 뒤에 붙는다. */
    retryAt: (a: unknown) => `${a}에 다시 쓸 수 있습니다.`,
    /** 그 줄의 단추(지금 할 수 있는 일). */
    failureAction: {
      retry: '다시 보내기',
      settings: 'AI 연결 열기',
      switchModel: '모델 바꾸기',
      switchProvider: '다른 AI로 바꾸기',
      shorten: '새 채팅방',
    } as Record<string, string>,
    authFailed: (a: unknown) => `${a} 연결이 끊어졌습니다. 설정의 AI 연결에서 다시 연결해 주세요.`,
    text11: '이름을 바꾸지 못했습니다',
    text12: '이 환경에서는 폴더 선택을 사용할 수 없습니다',
    text13: '다시 켜지 못했습니다',
    text14: (a: unknown) => `새 ${a} 만들기`,
    text15: (a: unknown) => `${a} 화면으로 가기`,
    text16: '설정 열기',
    text17: (a: unknown) => `새 ${a}`,
    text18: (a: unknown) => `${a}${josa(a, '을', '를')} 만들지 못했습니다`,
    text19: '차례를 바꾸지 못했습니다',
    text20: (a: unknown) => `${a}${josa(a, '을', '를')} 삭제하지 못했습니다`,
    text21: (a: unknown) => `${a}${josa(a, '을', '를')} 바꾸지 못했습니다`,
    text22: '채팅방이 길어져 앞의 내용을 줄여서 이어 갑니다.',
    title1: '다시 켤까요?',
    text23: '저장하지 않은 수정 내용이 있거나 AI가 답하는 중입니다. 지금 다시 켜면 그 내용이 사라집니다.',
    text24: '그만두기',
    text25: '다시 켜기',
  },
  // ---- approvals.ts ----
  approvals: {
    text1: '이번만 허락',
    // 2026-11-16 김성균 개발자가 정함("이 채팅방에서 항상 허락 이렇게").
    text2: '이 채팅방에서 항상 허락',
    text3: '거절',
    text4: '그만두기',
    text5: '이번만 허락했습니다',
    text6: '이 채팅방에서 항상 허락했습니다',
    text7: '거절했습니다',
    text8: '그만두었습니다',
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
    aria1: '허락을 구하는 물음',
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
      visibility: '보이는 사람',
      visibilityText: { private: '나만 볼 수 있습니다.' } as Record<string, string>,
      cost: '비용',
      costText: { possible: '쓰는 양에 따라 그 회사에서 돈이 나갈 수 있습니다.' } as Record<string, string>,
    },
  },
  // ---- components/SecretRequestCard.tsx: 스킬로 하는 연결에서 키를 사람이 직접 넣는 칸. 2026-11-16 새로 쓴 글. ----
  secretRequest: {
    aria: '연결에 필요한 키 넣기',
    title: (a: unknown) => `${a} 연결에 필요한 키를 넣어 주세요`,
    note: '넣은 키는 AI에게 보내지 않고 이 컴퓨터의 몸소가 보관합니다.',
    save: '저장',
    cancel: '그만두기',
    saved: '키를 저장했습니다',
    cancelled: '키 넣기를 그만두었습니다',
    /** 2026-11-16 새로 씀(그의 확인 전): 그만두기(agent_setup_secret_cancel)가 실패했을 때. */
    cancelFailed: '그만두지 못했습니다',
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
    text4: '위치 바꾸기',
    aria1: '폴더 옮기는 중',
    text5: '버전',
    text6: '업데이트',
    /** 2026-11-16 새로 씀(그의 확인 전): 앱 정보 아래 저작권. 저작권 줄은 NOTICE 의 표기 그대로(MOW and BigBlue), 나머지는 NOTICE·LICENSE 를 짧게 옮김. */
    copyright: 'Copyright © 2026 MOW and BigBlue',
    license: (name: string) => `몸소는 ${name} 라이선스를 따릅니다.`,
    licenseLink: '라이선스 전문 보기',
    otherMarks: '앱에 나오는 다른 회사의 이름과 로고는 각 회사의 것입니다.',
    text7: '최신 버전입니다.',
    text8: '새 버전 ',
    text9: '이 있습니다.',
    aria2: '업데이트 받는 중',
    text10: '다 받았습니다. 다시 켜면 업데이트가 끝납니다.',
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
    text3: '그만두기',
    text4: (a: unknown) => `${a} 삭제`,
    aria2: (a: unknown) => `${a} 이름`,
    label2: (a: unknown, b: unknown) => `${a} ${b} 메뉴`,
    text5: '이름 바꾸기',
    text6: '위로 옮기기',
    text7: '아래로 옮기기',
    aria3: (a: unknown) => `${a}, 답을 기다림`,
    label3: (a: unknown) => `${a} 메뉴`,
    text8: (a: unknown) => `${a}에 넣기`,
    text9: (a: unknown) => `${a}에서 빼기`,
  },
  // ---- components/Composer.tsx ----
  composer: {
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
    aria2: '몸소 AI에게 보낼 말',
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
    otherCategory: '그 밖',
    mine: '내가 더함',
    spellings: '다른 표기',
    edited: '내가 고침',
    // 2026-11-16 쓰기(더하기·수정·끄기, 사람 이름). 새로 씀, 그의 확인 전.
    termsTab: '용어',
    peopleTab: '사람 이름',
    addTerm: '새 용어',
    addPerson: '새 이름',
    fieldSpelling: '표기',
    fieldMeaning: '뜻',
    save: '저장',
    cancel: '그만두기',
    edit: '수정',
    remove: '삭제',
    saveMeaning: '뜻 저장',
    addSpellingHint: '다른 표기 더하기',
    addSpelling: '더하기',
    removeSpelling: (a: unknown) => `${a} 삭제`,
    useTerm: '이 용어 쓰기',
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
    fieldAliases: '다르게 부르는 이름(쉼표로 나눔)',
    aliases: '다르게 부르는 이름',
    unconfirmed: '확인 안 됨',
    confirm: '확인됨으로 바꾸기',
    removePersonTitle: (a: unknown) => `“${a}” 이름을 삭제할까요?`,
    removePersonText: '삭제한 이름은 되돌릴 수 없습니다.',
    // 2026-11-17 용어 삭제와 새 용어의 갈래. 새로 씀, 그의 확인 전.
    fieldCategory: '갈래',
    categoryNone: '고르지 않음',
    deleteTerm: '용어 삭제',
    removeTermTitle: (a: unknown) => `“${a}” 용어를 삭제할까요?`,
    removeTermText: '삭제한 용어는 되돌릴 수 없습니다.',
    baseTerm: '기본 용어는 삭제할 수 없습니다. 끄기만 할 수 있습니다.',
  },
  /** 고른 지 5분이 지나 뒤쪽이 거절했을 때(selection_required). 첨부·대화 가져오기·몸소 폴더가 같은 글을 쓴다. 2026-11-17 새로 씀, 그의 확인 전. */
  selectAgain: '선택한 지 오래되어 진행하지 못했습니다. 파일을 다시 선택해 주세요',
  // ---- components/ChatImportSettings.tsx: 다른 AI 에서 내보낸 대화 가져오기. 2026-11-16 새로 씀(그가 보고 고친다, 낱말 목록에 올림). ----
  chatImport: {
    title: '다른 AI의 대화 가져오기',
    intro: 'ChatGPT, 클로드, 제미나이에서 내보낸 파일을 선택하면, 그 대화가 왼쪽 목록에 채팅방으로 생깁니다.',
    name: { chatgpt: 'ChatGPT', claude: '클로드', gemini: '제미나이' } as Record<string, string>,
    hint: {
      chatgpt: 'ChatGPT 설정의 데이터 내보내기로 받은 파일(zip)을 선택합니다.',
      claude: '클로드 설정의 데이터 내보내기로 받은 파일(zip)을 선택합니다.',
      // 2026-11-17 제미나이 가져오기가 생김. 새로 씀, 그의 확인 전.
      gemini: '구글 데이터 내보내기로 받은 제미나이 파일(zip 또는 JSON)을 선택합니다.',
    } as Record<string, string>,
    pick: '파일 선택',
    fileKind: '내보낸 대화 파일',
    working: '가져오는 중',
    done: (n: number) => `대화 ${n}개를 가져왔습니다.`,
    duplicate: (n: number) => `${n}개는 이미 있어 건너뛰었습니다.`,
    updated: (n: number) => `${n}개는 새로 이어진 말을 더했습니다.`,
    failed: '대화를 가져오지 못했습니다',
    // 2026-11-16 미리 세기·진행·그만두기. 새로 씀, 그의 확인 전.
    counting: '파일 안의 대화를 세는 중',
    preview: (n: number) => `대화 ${n}개가 들어 있습니다.`,
    previewAlready: (n: number) => `${n}개는 이미 가져온 적이 있어 새로 이어진 말만 더합니다.`,
    previewUnreadable: (n: number) => `${n}개는 읽을 수 없어 건너뜁니다.`,
    previewRange: (a: string, b: string) => `${a}부터 ${b}까지의 대화입니다.`,
    previewDay: (a: string) => `${a}의 대화입니다.`,
    previewEmpty: '이 파일에는 가져올 대화가 없습니다.',
    start: '가져오기',
    cancel: '그만두기',
    progress: (done: number, total: number) => `가져오는 중 ${done}/${total}`,
    cancelling: '그만두는 중',
    cancelled: '가져오기를 그만두었습니다. 이번에 가져온 대화는 남기지 않았습니다.',
    reason: {
      invalid_format: '이 파일에서 대화를 찾지 못했습니다. 내보내기로 받은 파일인지 확인해 주세요.',
      unsupported_format: '이 파일에서 대화를 찾지 못했습니다. 내보내기로 받은 파일인지 확인해 주세요.',
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
    nameUse: (a: unknown) => `이 이름으로 내 GitHub 저장소와 오브젝트 스토리지 버킷이 만들어집니다(${a}).`,
    nameUseSample: 'momso-이름',
    nameLabel: '사용자 이름',
    nameChars: '영문자, 숫자, -, _, . 만 쓸 수 있습니다.',
    nameRejected: '쓸 수 없는 이름입니다. 다른 이름을 적어 주세요.',
    nameSaveFailed: '이름을 저장하지 못했습니다',
    next: '다음',
    back: '이전',
    aiTitle: 'AI를 하나 이상 연결해 주세요',
    aiText: (a: unknown, b: unknown, c: unknown) => `${a}, ${b}, ${c} 가운데 하나 이상 연결해야 몸소를 시작할 수 있습니다.`,
    checking: 'AI 연결 상태를 확인하고 있습니다',
    checkFailed: 'AI 연결 상태를 확인하지 못했습니다.',
    retry: '다시 시도',
    start: '시작하기',
  },
  // ---- components/MediaView.tsx ----
  mediaView: {
    aria1: '미디어',
    aria2: '동영상',
    aria3: '사진',
    aria4: '크게 보기',
    aria5: '앞 것',
    aria6: '다음 것',
    aria7: '닫기',
    /** 2026-11-17 읽는 동안·없을 때·읽지 못했을 때(본부 결정, 그가 보고 고친다). */
    loading: '미디어 불러오는 중',
    empty: '미디어가 없습니다',
    failed: '미디어를 읽지 못했습니다',
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
    aria2: '찾을 종류',
    /** 2026-11-16 바꿈(원자료 찾기를 더함, 그의 확인 전). 예전 글은 "채팅방, 문서, 할 일 찾기". */
    hint2: (a: unknown, b: unknown, c: unknown) => `${a}, ${b}, ${c}, 할 일 찾기`,
    aria3: '찾은 것',
    aria4: '키 안내',
    label1: '선택',
    label2: '열기',
    label3: '종류 바꾸기',
    /** 2026-11-17 찾는 동안·찾은 것이 없을 때·검색이 실패했을 때(본부 결정, 그가 보고 고친다). */
    searching: '검색하는 중',
    empty: '찾은 것이 없습니다',
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
    tip1: '띠의 고리가 보이는 갈래',
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
    label1: '목록 보이기',
    label2: '목록 숨기기',
    tip1: '최소화',
    tip2: '이전 크기로',
    tip3: '최대화',
    tip4: '닫기',
  },
  // ---- components/ZetPanel.tsx ----
  zetPanel: {
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
    text16: '사람이 직접',
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
    text21: '버리기',
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
    text40: '그만두기',
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
    text50: (a: unknown) => `그때 제목: ${a}`,
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
    text75: '한 때',
    text76: '읽은 것',
    text77: '쓴 것',
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
