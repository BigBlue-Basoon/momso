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
  backup: {
    pageIntro: '문서와 고친 이력은 GitHub에, 원자료는 고른 오브젝트 스토리지에 백업합니다.',
    githubTitle: 'GitHub',
    githubWhat: '문서와 고친 이력',
    /** GitHub 연결 때의 알림: 방향 문서 L243 의 글 그대로. */
    githubNotice: '수업 글이 내 GitHub 비공개 저장소에 올라간다',
    repoLabel: '저장소',
    connect: '연결',
    push: '올리기',
    pull: '받기',
    disconnect: '연결 끊기',
    account: 'GitHub 계정',
    lastPushed: '마지막으로 올린 때',
    ahead: (a: unknown) => `올릴 것 ${a}`,
    behind: (a: unknown) => `받을 것 ${a}`,
    githubDisconnectTitle: 'GitHub 연결을 끊을까요?',
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
    pending: (a: unknown, b: unknown) => `아직 올리지 않은 원자료 ${a}개 · ${b}`,
    backupNow: '지금 백업',
    restore: '백업에서 되살리기',
    storeDisconnectTitle: '오브젝트 스토리지 연결을 끊을까요?',
    storeDisconnectText: '스토리지에 올린 원자료는 지워지지 않습니다.',
    restoreTitle: '백업에서 되살릴까요?',
    restoreText: '앱의 목록을 마지막 백업 때로 되돌리고, 이 기기에 없는 원자료를 받아옵니다. 되돌리기 전에 지금 상태를 따로 남겨 둡니다.',
    cancel: '그만두기',
    done: '끝났습니다',
    failed: '하지 못했습니다',
    /** 뒤쪽 상태 값 → 쉬운 말(실패 알림). */
    /** 원자료 저장소 회사 이름: 영어 그대로(2026-11-16 김성균 개발자가 정함, "저거는 그냥 AWS GCP 이렇게 적고"). */
    storeName: { aws: 'AWS', gcp: 'GCP', cloudflare: 'Cloudflare', ncp: 'NCP' } as Record<'aws' | 'gcp' | 'cloudflare' | 'ncp', string>,
    /** 2026-11-16 새로 쓴 글(그가 아직 정하지 않음): 이력이 막힌 까닭이 사용자 정보일 때(historyReason). 그가 "올린다"로 정하면 없어질 수 있다. */
    profileHeld: "설정의 '나에 대해'를 저장한 이력이 있어 GitHub 백업을 멈췄습니다",
    reason: {
      git_missing: 'Git이 필요합니다', gh_missing: 'GitHub 연결 도구가 필요합니다', signed_out: 'GitHub에 로그인해 주세요',
      invalid_repository: '저장소 이름을 확인해 주세요', repository_unavailable: '저장소를 찾지 못했습니다', repository_not_private: '비공개 저장소만 연결할 수 있습니다',
      remote_ahead: 'GitHub에 더 새 내용이 있습니다. 먼저 받아 주세요', diverged: '이 기기와 GitHub의 이력이 서로 달라 멈췄습니다',
      unsaved_changes: '저장하지 않은 문서가 있습니다. 먼저 저장해 주세요', nothing_saved: '올릴 저장한 내용이 없습니다', busy: '다른 작업이 끝난 뒤에 다시 해 주세요',
      invalid_credentials: '키를 확인해 주세요', bucket_not_found: '버킷을 찾지 못했습니다', access_denied: '이 키로는 버킷에 쓸 수 없습니다', write_denied: '이 키로는 버킷에 쓸 수 없습니다',
      invalid_endpoint: '주소를 확인해 주세요', invalid_region: '지역을 확인해 주세요', invalid_bucket: '버킷 이름을 확인해 주세요',
      recovery_key_unconfirmed: '먼저 복구 키를 만들고 보관을 확인해 주세요', recovery_key_missing: '먼저 복구 키를 만들고 보관을 확인해 주세요',
      file_too_large: '너무 큰 원자료가 있어 멈췄습니다',
    } as Record<string, string>,
  },
  // ---- 설정의 AI 로그인(코덱스·클로드·제미나이). 2026-11-16 새로 쓴 글(본부·그의 확인 전, 낱말 목록에 올림). ----
  signIn: {
    title: 'AI 연결',
    intro: '쓰려는 AI에 계정으로 로그인하거나 API 키를 넣습니다.',
    state: { account: '계정으로 연결됨', apiKey: 'API 키로 연결됨', signedOut: '연결 안 됨' } as Record<string, string>,
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
    keyClear: '키 지우기',
    logoutTitle: (a: unknown) => `${a}에서 로그아웃할까요?`,
    logoutText: '다시 쓰려면 로그인해야 합니다.',
    clearTitle: (a: unknown) => `${a} API 키를 지울까요?`,
    clearText: '지운 키는 다시 볼 수 없습니다. 다시 쓰려면 키를 새로 넣어야 합니다.',
    invalidKey: '키 모양이 맞지 않습니다. 다시 확인해 주세요.',
    loginFailed: '로그인을 시작하지 못했습니다',
    logoutFailed: '로그아웃하지 못했습니다',
    keySaveFailed: '키를 저장하지 못했습니다',
    keyClearFailed: '키를 지우지 못했습니다',
    /** API 키로 쓸 때 따로 드는 비용 안내(그 회사의 글을 Codex 가 확인해 오면 넣는다. 비어 있으면 그리지 않는다). */
    apiCost: { codex: '', claude: '', gemini: '' } as Record<string, string>,
  },
  // ---- 스킬(그가 정한 이름 "스킬"). 아래 글은 2026-11-16 새로 쓴 것(본부·그의 확인 전, 낱말 목록에 올림). ----
  skills: {
    word: '스킬',
    /** 입력 칸에서 / 를 쳤을 때 뜨는 목록의 화면 읽기 이름. */
    pickerLabel: '스킬 고르기',
    /** 입력 칸 안 스킬 조각의 지우기 단추. */
    removeChip: (a: unknown) => `${a} 빼기`,
    /** 보낸 말 위에 붙는 표시의 화면 읽기 이름. */
    usedLabel: (a: unknown) => `스킬: ${a}`,
    /** 설정 쪽 */
    settingsIntro: '자주 시키는 일을 한 번 적어 두고, 입력 칸에서 / 를 쳐서 불러 씁니다.',
    empty: '스킬은 자주 시키는 일을 한 번 적어 두는 것입니다. 입력 칸에서 / 를 치면 골라 쓸 수 있습니다.',
    add: '새 스킬',
    builtin: '처음부터 있던 것',
    mine: '내가 만든 것',
    conflict: '다른 곳에서 고친 내용과 다릅니다',
    enable: (a: unknown) => `${a} 켜기`,
    fieldTitle: '이름',
    fieldName: '부르는 이름',
    fieldNameHelp: '영어 소문자, 숫자, 붙임표(-)만 쓸 수 있습니다. 만든 뒤에는 바꿀 수 없습니다.',
    fieldDescription: '한 줄 설명',
    fieldBody: '내용',
    bodyHint: 'AI가 이 스킬을 쓸 때 따를 내용을 적어 주세요.',
    save: '저장',
    cancel: '그만두기',
    remove: '없애기',
    back: '스킬 목록',
    removeTitle: (a: unknown) => `“${a}” 스킬을 없앨까요?`,
    removeText: '없앤 스킬은 되돌릴 수 없습니다.',
    saved: '저장했습니다',
    saveFailed: '저장하지 못했습니다',
    invalidName: '부르는 이름은 영어 소문자, 숫자, 붙임표(-)로 적어 주세요.',
    nameTaken: '같은 부르는 이름의 스킬이 이미 있습니다.',
    needTitle: '이름을 적어 주세요.',
    conflictTitle: '이 스킬이 그사이 바뀌었습니다',
    conflictText: '고치는 동안 이 스킬이 다른 곳에서 바뀌었습니다. 바뀐 내용을 불러오면 내가 고친 내용이 사라지고, 내 글로 덮어쓰면 그사이 바뀐 내용이 사라집니다.',
    reload: '바뀐 내용 불러오기',
    overwrite: '내가 고친 것으로 덮어쓰기',
    loadFailed: '스킬을 읽지 못했습니다',
    toggleFailed: '스킬을 켜거나 끄지 못했습니다',
    removeFailed: '스킬을 없애지 못했습니다',
    /** 보낼 때 고른 스킬이 그사이 꺼졌거나 없어졌을 때(chat_send → skill_unavailable). 새로 쓴 글, 그의 확인 전. */
    unavailable: '이 스킬은 지금 쓸 수 없어 보내지 않았습니다',
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
    notice: '이 폴더는 OneDrive가 자동으로 올리는 곳입니다. 몸소 폴더를 여기에 두면 문서의 이력이 엉킬 수 있습니다. 다른 폴더를 권합니다.',
    pickOther: '다른 폴더 고르기',
    keep: '그래도 여기에 두기',
  },
  // ---- 미디어 갤러리(미디어 화면과 오른쪽 패널) · 그 채팅방에서 다룬 것. 2026-11-16 새로 쓴 글(본부·그의 확인 전) ----
  gallery: {
    /** 무엇을 했는지(본부가 전한 말). */
    how: { created: '만듦', edited: '고침', read: '읽음', attached: '붙임' } as Record<string, string>,
    /** 크게 보기에서 채팅방 이름 단추의 풍선. */
    openChat: '채팅방으로 가기',
    audio: '소리',
    /** 오른쪽 패널의 첫 탭 이름(그 채팅방에서 다룬 문서와 미디어). */
    chatTab: '이 채팅방',
    /** 그 채팅방에서 다룬 것이 없을 때 패널의 한 줄(본부가 든 예를 따름). */
    chatEmpty: '이 채팅방에서 다룬 문서와 미디어가 여기에 모입니다.',
    /** 패널을 닫아 둔 동안 새로 생긴 것이 있을 때 패널 단추의 화면 읽기 이름. */
    newInPanel: (a: unknown) => `${a}, 새로 생긴 것 있음`,
  },
  // ---- ai-conversation-import.ts ----
  aiConversationImport: {
    text1: '코덱스 JSONL 기록',
    text2: 'Claude Code JSONL 기록',
    text3: 'ChatGPT 내보내기 ZIP 또는 conversations.json',
    text4: '클로드 내보내기 JSON 또는 채팅방 텍스트',
    text5: 'ChatGPT 채팅방',
    text6: (a: unknown) => `${a} 채팅방`,
    text7: '코덱스 채팅방',
    text8: 'Claude Code 채팅방',
    text9: (a: unknown) => `${a}: 압축 파일 안의 전체 항목이 5,000개를 넘습니다`,
    text10: (a: unknown) => `${a}: 채팅방 파일이 200개를 넘어 나눠서 가져와야 합니다`,
    text11: (a: unknown, b: unknown) => `${a} / ${b}: 압축을 푼 파일이 128MB를 넘습니다`,
    text12: (a: unknown) => `${a}: 압축을 푼 채팅방 파일 합계가 256MB를 넘습니다`,
    text13: (a: unknown) => `${a}: 500MB보다 큰 내보내기 파일은 나눠서 가져와 주세요`,
    text14: (a: unknown) => `${a}: 지원하는 채팅방 기록을 찾지 못했습니다`,
    text15: '선택한 파일에서 사용자와 AI의 채팅방을 찾지 못했습니다',
    text16: '오늘',
    text17: '어제',
    text18: (a: unknown) => `메시지 1–${a}`,
    text19: '메시지 1',
    text20: '첫 요청',
    text21: (a: unknown) => `메시지 ${a}`,
    text22: '가져온 범위',
    text23: (a: unknown, b: unknown) => `${a}에서 사용자와 AI가 주고받은 메시지 ${b}개를 원문 순서대로 보존했습니다. 도구 실행과 시스템 지시는 포함하지 않았습니다.`,
    text24: '나',
    text25: (a: unknown) => `${a}개 메시지`,
    text26: (a: unknown) => `${a} 대화`,
    text27: (a: unknown) => `나 · ${a}`,
    text28: (a: unknown) => `외부 AI 대화 원문을 가져왔습니다. ${a}개 메시지의 화자와 순서를 유지했으며, 자동 해석이나 요약은 추가하지 않았습니다.`,
    text29: '대화 원문 전체를 로컬 기록으로 보존',
  },
  // ---- app-update.ts ----
  appUpdate: {
    text1: '업데이트를 확인하지 못했습니다',
    text2: '업데이트를 받지 못했습니다',
    text3: '다시 켜서 업데이트 마치기',
    text4: '업데이트 받는 중',
    text5: (a: unknown) => `업데이트${a}`,
  },
  // ---- App.tsx ----
  app: {
    text1: '채팅방 목록을 읽지 못했습니다',
    text2: '채팅방을 읽지 못했습니다',
    text3: '로컬 저장소를 준비하지 못했습니다',
    text4: (a: unknown) => `“${a}”을 가져왔습니다`,
    text5: (a: unknown) => `AI 채팅방 ${a}개를 가져왔습니다`,
    text6: '권한 단계를 바꾸지 못했습니다',
    text7: 'AI 의 답을 받지 못했습니다',
    text8: '답을 보내지 못했습니다',
    text9: '말을 저장하지 못했습니다',
    text10: '말을 보내지 못했습니다',
    text11: '이름을 바꾸지 못했습니다',
    text12: '이 환경에서는 폴더 선택을 사용할 수 없습니다',
    text13: '다시 켜지 못했습니다',
    text14: (a: unknown) => `새 ${a} 만들기`,
    text15: (a: unknown) => `${a} 화면으로 가기`,
    text16: '설정 열기',
    text17: (a: unknown) => `새 ${a}`,
    text18: (a: unknown) => `${a}${josa(a, '을', '를')} 만들지 못했습니다`,
    text19: '차례를 바꾸지 못했습니다',
    text20: (a: unknown) => `${a}${josa(a, '을', '를')} 없애지 못했습니다`,
    text21: (a: unknown) => `${a}${josa(a, '을', '를')} 바꾸지 못했습니다`,
    text22: '채팅방이 길어져 앞의 내용을 줄여서 이어 갑니다.',
    title1: '다시 켤까요?',
    text23: '저장하지 않은 고친 내용이 있거나 AI가 답하는 중입니다. 지금 다시 켜면 그 내용이 사라집니다.',
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
  // ---- components/AiConnectionSettings.tsx ----
  aiConnectionSettings: {
    text1: 'ChatGPT 계정 또는 OpenAI API 키',
    text2: 'Claude.ai 계정 또는 Anthropic API 키',
    text3: '구독 계정',
    text4: '공식 CLI의 브라우저 로그인 사용',
    text5: 'API 키',
    text6: '사용량에 따라 제공자에 직접 결제',
    text7: 'API 키를 Windows 자격 증명 관리자에 저장했습니다',
    aria1: 'AI 제공자',
    aria2: 'AI 이용 방식',
    text8: '공식 연결 상태를 확인하고 있습니다',
    text9: '구독 계정 로그인됨',
    text10: '로그인이 필요합니다',
    text11: '공식 연결 도구가 필요합니다',
    text12: '확인 중',
    text13: '연결됨',
    text14: '로그아웃',
    text15: '미설치',
    text16: '처리 중',
    text17: '로그인',
    text18: '설치하고 로그인',
    aria3: '연결 상태 새로고침',
    aria4: '연결 상태 확인 중',
    text19: 'OpenAI API 키',
    text20: 'Anthropic API 키',
    hint1: '새 키로 교체',
    hint2: 'API 키 입력',
    text21: '저장됨',
    text22: '미설정',
    text23: '삭제',
    text24: '저장',
    text25: '사용량',
    text26: '구독 한도와 이 기기의 요청 기록',
    text27: '이 기기에서 발생한 추정 비용',
    aria5: '사용량 새로고침',
    text28: '사용량을 확인하고 있습니다',
    text29: '사용량 정보가 없습니다',
    text30: (a: unknown) => `${a} 남음`,
    aria6: (a: unknown) => `${a} 남은 사용량`,
    text31: (a: unknown) => `${a} 초기화`,
    text32: '초기화 시각 미제공',
    text33: '현재 구독 한도를 확인할 수 없습니다',
    text34: (a: unknown, b: unknown) => `이번 달 이 기기에서 ${a}회 · ${b}`,
    text35: '비용을 집계하고 있습니다',
    label1: '최근 요청',
    label2: '오늘',
    label3: '이번 달',
    text36: (a: unknown, b: unknown) => `이번 달 ${a}회 · ${b} · 실제 청구액은 제공자 청구서 기준`,
    text37: '추정',
    text38: '집계 전',
    text39: (a: unknown) => `${a} 토큰`,
  },
  // ---- components/AiConversationImporter.tsx ----
  aiConversationImporter: {
    text1: '날짜 정보 없음',
    text2: (a: unknown) => `${a}의 로컬 채팅방 기록을 찾지 못했습니다`,
    text3: '가져올 수 있는 채팅방 원문이 없습니다',
    aria1: 'AI 서비스 선택',
    text4: '이 컴퓨터에서 찾기',
    text5: '내보내기 파일 선택',
    text6: '채팅방 기록을 읽고 있습니다',
    text7: (a: unknown) => `${a} 채팅방 가져오기`,
    text8: '선택한 원문은 외부 서버로 보내지 않고 이 기기에서만 읽습니다.',
    aria2: '가져올 채팅방 목록',
    text9: '현재 목록 전체',
    text10: (a: unknown) => `${a}개`,
    text11: '채팅방 찾기',
    hint1: '제목 또는 프로젝트 검색',
    aria3: (a: unknown) => `${a} 선택`,
    text12: (a: unknown, b: unknown) => `${a} · ${b}개 메시지`,
    text13: '가져옴',
    text14: '검색 결과가 없습니다',
    aria4: '채팅방 내용',
    text15: '나',
    text16: '전체 ',
    text17: '개 메시지를 원문 순서대로 가져옵니다',
  },
  // ---- components/AiWelcome.tsx ----
  aiWelcome: {
    text1: '어떤 일이든 고르게',
    text2: '이해력, 속도, 사용량이 전반적으로 준수합니다. 큰 작업을 이어가거나 이미지를 만드는 데에도 좋습니다.',
    text3: '사용량은 제미나이보다 적고, 섬세한 대화에서는 클로드가 더 나을 때가 있습니다.',
    text4: '사람의 이야기를 깊이 있게',
    text5: '감정과 관계처럼 복잡하고 섬세한 맥락을 잘 이해합니다. 생각을 함께 정리할 때 잘 어울립니다.',
    text6: '같은 구독료에서 사용량이 가장 적은 편입니다. 이미지 생성에는 별도의 도구가 필요합니다.',
    text7: '빠르고 넉넉하게',
    text8: '응답이 빠르고, 같은 구독료로 가장 넉넉하게 사용할 수 있는 편입니다. 이미지 작업도 할 수 있습니다.',
    text9: '셋 중 대화의 맥락이나 세부 의도를 놓치는 경우가 가장 많습니다.',
    text10: '함께할 AI를 선택해 주세요',
    text11: '나에게 잘 맞는 AI와 몸소를 시작해 보세요.',
    aria1: 'AI 선택',
    text12: '좋은 점',
    text13: '아쉬운 점',
    text14: '하나를 연결하면 시작할 수 있습니다. 다른 AI는 나중에 추가할 수 있습니다.',
    text15: '비교는 사용 경험을 바탕으로 하며 모델과 이용 조건에 따라 달라질 수 있습니다.',
    text16: (a: unknown) => `${a} 연결하기`,
    text17: 'AI를 선택해 주세요',
  },
  // ---- components/ApprovalCard.tsx ----
  approvalCard: {
    text1: (a: unknown) => `${a}${josa(a, '이', '가')} 컴퓨터에서 이 명령을 실행하려고 합니다`,
    text2: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 고치려고 합니다`,
    text3: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 고치려고 합니다`,
    text4: (a: unknown) => `${a}${josa(a, '이', '가')} 허락을 구합니다`,
    aria1: '허락을 구하는 물음',
    /**
     * 2026-11-16 새로 쓴 글(그가 아직 정하지 않음): 파일을 어떻게 바꾸려는지(change)에 따른 머리 글.
     * edit 는 위 text2/text3 을 그대로 쓴다. 종류를 모르면(null) "바꾸려고"로 짐작하지 않고 적는다.
     */
    change: {
      create: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 새로 만들려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 새로 만들려고 합니다` },
      overwrite: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 덮어쓰려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 덮어쓰려고 합니다` },
      delete: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 지우려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 지우려고 합니다` },
      unknown: { doc: (a: unknown) => `${a}${josa(a, '이', '가')} 이 문서를 바꾸려고 합니다`, file: (a: unknown) => `${a}${josa(a, '이', '가')} 이 파일을 바꾸려고 합니다` },
    },
    /** 대상이 여럿일 때 머리 글과 대상마다 붙는 종류 낱말. */
    many: (a: unknown, n: number) => `${a}${josa(a, '이', '가')} 파일 ${n}개를 바꾸려고 합니다`,
    /** 코덱스가 파일 목록 없이 허락할 폴더(grantRoot)만 보냈을 때, 그것도 없을 때. */
    inFolder: (a: unknown) => `${a}${josa(a, '이', '가')} 이 폴더 안의 파일을 바꾸려고 합니다`,
    someFile: (a: unknown) => `${a}${josa(a, '이', '가')} 파일을 바꾸려고 합니다`,
    targetChange: { create: '새로 만들기', edit: '고치기', overwrite: '덮어쓰기', delete: '지우기' } as Record<string, string>,
  },
  // ---- components/AppSettingsParts.tsx ----
  appSettingsParts: {
    text1: '폴더를 옮기지 못했습니다',
    /** 2026-11-16 새로 쓴 글(그의 확인 전): 옮기지 못한 까닭. */
    moveNotEmpty: '고른 폴더가 비어 있지 않습니다. 빈 폴더를 고르세요.',
    moveInvalid: '그 폴더로는 옮길 수 없습니다. 다른 폴더를 고르세요.',
    /** 2026-11-16 새로 쓴 글(그가 아직 정하지 않음): 옮긴 뒤 예전 폴더가 그대로 남아 있을 때(previousKept). */
    previousKept: (path: string | null) => (path ? `예전 폴더는 지우지 않고 그대로 두었습니다: ${path}` : '예전 폴더는 지우지 않고 그대로 두었습니다.'),
    text2: (a: unknown, b: unknown) => `${a}와 ${b}가 이 폴더에 들어 있습니다.`,
    text3: '폴더 열기',
    text4: '위치 바꾸기',
    aria1: '폴더 옮기는 중',
    text5: '버전',
    text6: '업데이트',
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
    aria1: '채팅방',
    /** 2026-11-16 바꾼 글(그가 아직 정하지 않음): 넘기지 못한 말의 수가 온다(chat-done 의 notHandedOver). */
    text2: (a: unknown, n: number) => `앞의 말 ${n}개를 ${a}에 넘기지 못했습니다. 아래 답은 그 말을 모르고 쓴 것입니다.`,
    text3: '답을 쓰는 중',
    aria2: '채팅방 이름',
    tip1: '이름 바꾸기',
  },
  // ---- components/BrandMotion.tsx ----
  brandMotion: {
    text1: '몸소 로고 애니메이션 다시 보기',
    text2: '몸소 심볼',
  },
  // ---- components/ChatSidebar.tsx ----
  chatSidebar: {
    aria1: '채팅방 목록',
    text1: '새 채팅방',
    label1: (a: unknown) => `새 ${a}`,
    title1: (a: unknown, b: unknown) => `“${a}” ${b}${josa(b, '을', '를')} 없앨까요?`,
    text2: (a: unknown) => `안의 채팅방은 지워지지 않고 ${a} 밖으로 나옵니다.`,
    text3: '그만두기',
    text4: (a: unknown) => `${a} 없애기`,
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
    text1: 'Enter 키로 보내기, Shift+Enter 키로 줄 바꾸기',
    text2: 'Enter 키로 보내기',
    aria1: '파일 가져오기',
    aria2: '몸소 AI에게 보낼 말',
    aria3: '응답 중단',
    aria4: '보내기',
    tip1: 'AI 고르기',
    aria5: (a: unknown) => `모델: ${a}`,
    tip2: '모델 고르기',
    label1: '모델',
    aria6: (a: unknown) => `추론 수준: ${a}`,
    aria7: '추론 수준',
    tip3: '추론 수준 고르기',
    aria8: '특별한 추론 수준',
    /** 그 회사가 추천으로 표시한 추론 수준 옆 꼬리표(Claude 앱의 공식 한국어 "추천"). */
    recommended: '추천',
    /** 추론 수준 슬라이더의 화면 읽기 이름. */
    effortSlider: '추론 수준 바꾸기',
    aria9: (a: unknown) => `권한: ${a}`,
    tip4: '권한 단계',
    text3: (a: unknown) => `이 채팅방이 AI가 한 번에 볼 수 있는 양의 ${a}%를 채웠습니다.`,
    text4: (a: unknown) => `${a}% 더 차면 앞의 내용을 줄여서 이어 갑니다.`,
    text5: '곧 앞의 내용을 줄여서 이어 갑니다.',
  },
  // ---- components/FirstLaunch.tsx ----
  firstLaunch: {
    text1: '몸을 소중히 여기는 실천, ',
    text2: '몸소',
    text3: '시작하기 ',
  },
  // ---- components/ImportDialog.tsx ----
  importDialog: {
    text1: (a: unknown, b: unknown, c: unknown) => `${a}시간 ${b}분 ${c}초`,
    text2: (a: unknown, b: unknown) => `${a}분 ${b}초`,
    text3: (a: unknown, b: unknown, c: unknown) => `지원하지 않는 파일입니다: ${a}. 오디오(${b})나 문서(${c})만 가져올 수 있습니다`,
    text4: '오디오 또는 문서 선택',
    text5: '오디오·문서',
    text6: '오디오',
    text7: '문서',
    text8: '직접 메모',
    text9: '메모 1개',
    text10: '가져온 원문과 기록 메모를 분리해 두었습니다. 원문에 없는 진단이나 원인 추정은 추가하지 않습니다.',
    text11: (a: unknown) => `메모 ${a}개`,
    text12: (a: unknown) => `메모 ${a}`,
    text13: '작성자',
    text14: '기록 메모',
    text15: (a: unknown) => `메모 1–${a}`,
    text16: '메모 1',
    text17: '문서 원문',
    text18: (a: unknown) => `${a}개 문단`,
    text19: '가져오기 정보',
    text20: (a: unknown) => `원자료는 보존했지만 본문에는 넣지 않았습니다. ${a}`,
    text21: '오디오 원자료',
    text22: '가져온 오디오의 제목, 참여자, 기록 메모를 확인했습니다. 원문 표현을 추가하지 않고 메모 범위 안에서 정리합니다.',
    text23: '오늘',
    text24: '직접 작성한 기록',
    text25: (a: unknown) => `기록은 추가됐고 원자료도 보존됐지만 둘을 연결하지 못했습니다: ${a}`,
    text26: '새 기록',
    aria1: '새 기록 닫기',
    tip1: '닫기',
    aria2: '새 기록 방식',
    text27: '파일 가져오기',
    text28: 'AI 채팅방',
    text29: '바꾸기',
    text30: '원자료를 읽고 있습니다',
    text31: '내용 해시를 계산해 보존 계획을 만듭니다.',
    text32: '파일을 끌어 놓거나 Windows에서 고릅니다.',
    text33: '파일 선택',
    text34: '기록 제목',
    text35: '참여자',
    text36: '원자료 보관',
    hint1: '수업 맥락이나 확인할 점을 적습니다',
    text37: '녹음 참여자에게 기록 목적과 보관 방식을 알리고 동의를 확인했습니다.',
    hint2: '예: 오전 수업 관찰',
    text38: '메모',
    hint3: '관찰한 사실, 참여자의 표현, 다음에 확인할 점을 그대로 적습니다',
    text39: (a: unknown) => `${a}개 채팅방 선택 · 원문 전체 로컬 보존`,
    text40: '가져오기 전에 채팅방 원문을 확인하고 선택합니다',
    text41: '원자료는 이 기기의 몸소 폴더에 그대로 보존되고 새 기록에 연결됩니다.',
    text42: '입력 내용은 이 기기의 작업함에 추가됩니다.',
    text43: '취소',
    text44: '가져오는 중',
    text45: (a: unknown) => `${a}개 가져오기`,
    text46: '채팅방 가져오기',
    text47: '보존 중',
    text48: '추가 중',
    text49: '보존하고 작업함에 추가',
    text50: '작업함에 추가',
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
  },
  // ---- components/MessageRail.tsx ----
  messageRail: {
    aria1: '내가 한 말',
    aria2: (a: unknown) => `내가 한 말 ${a}번째로 가기`,
  },
  // ---- components/SearchDialog.tsx ----
  searchDialog: {
    text1: '전체',
    text2: '할 일',
    text3: (a: unknown) => `새 ${a}`,
    text4: (a: unknown) => `최근 ${a}`,
    aria1: '닫기',
    aria2: '찾을 종류',
    hint1: (a: unknown, b: unknown) => `${a}, ${b}, 할 일 찾기`,
    aria3: '찾은 것',
    aria4: '키 안내',
    label1: '고르기',
    label2: '열기',
    label3: '종류 바꾸기',
  },
  // ---- components/SettingsView.tsx ----
  settingsView: {
    text1: '나에 대해',
    text2: '일반',
    text3: '기록',
    text4: 'AI와 개인정보',
    text5: '연결과 백업',
    text6: '저장공간',
    text7: '앱 정보',
    text8: (a: unknown) => `이 기기의 ${a}`,
    text9: '로컬 구조 데이터',
    text10: '사용자가 저장한 백업 파일',
    text11: '설정',
    aria1: '설정 닫기',
    aria2: '설정 항목',
    text12: '작업공간의 이름을 정합니다.',
    text13: '작업공간',
    text14: '이름',
    text15: '왼쪽 채팅방 목록 위에 표시됩니다.',
    text16: 'AI가 나를 어떻게 부르고, 무엇을 알고, 어떻게 답할지 적어 둡니다.',
    text17: '새 기록을 만들 때 반복해서 쓰는 기본값과 검수 흐름을 정합니다.',
    text18: '새 기록 기본값',
    text19: '참여자 표기',
    text20: '원자료 보관',
    text21: '가져오기 창에서 기록별로 다시 확인할 수 있습니다.',
    text22: '확인과 검수',
    label1: '녹음 동의 확인',
    description1: '오디오 기록을 작업함에 넣기 전에 참여자 동의를 확인합니다.',
    text23: 'AI 제공자와 결제 방식을 선택하고, 채팅방 이력의 처리 범위를 정합니다.',
    text24: 'AI 연결',
    text25: '구독 계정 로그인과 API 키 중 한 가지를 선택합니다.',
    text26: '채팅방 기본값',
    label2: 'AI 채팅방 이력 보관',
    description2: '기록별 채팅방을 이 기기의 앱 저장공간에 보관합니다.',
    text27: '응답 밀도',
    text28: '간결하게',
    text29: '균형 있게',
    text30: '자세하게',
    text31: '처리 경계',
    text32: '로컬 기록이 정본',
    text33: '요청을 보낼 때 선택한 기록 문맥만 선택한 AI 제공자에 전달되며, 로그인 정보와 API 키는 몸소 데이터베이스에 저장하지 않습니다.',
    text34: '요청 시 전송',
    text35: '이전 채팅방',
    text36: (a: unknown, b: unknown) => `${a}, ${b}, ChatGPT에서 주고받은 채팅방 원문을 몸소로 옮깁니다.`,
    text37: 'AI 채팅방 가져오기',
    text38: '로컬 정본을 사용자가 소유한 GitHub, Cloudflare R2, Neon 공간에 선택적으로 보존합니다.',
    text39: '이 기기에 있는 몸소 데이터의 위치와 백업 상태를 확인합니다.',
    text40: '로컬 데이터',
    text41: '폴더 열기',
    label3: '구조 데이터',
    label4: '백업',
    text42: '수동 백업',
    text43: '현재 구조 데이터를 별도 로컬 파일로 보존합니다.',
    text44: '백업 중',
    text45: '지금 백업',
    text46: '버전',
    text47: '데이터 상태',
    text48: '확인 필요',
    text49: '정상',
    text50: '스키마',
    text51: '지금 버전과 업데이트를 확인합니다.',
  },
  // ---- components/UsageMeter.tsx ----
  usageMeter: {
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
    text27: '암호화 파일 내려받는 중',
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
    text69: (a: unknown, b: unknown) => `${a}개 기록과 ${b}개 파일을 내려받습니다.`,
    text70: '현재 로컬 사본을 먼저 보존한 뒤 원격 기록으로 복원합니다',
    text71: '취소',
    text72: '원격 백업 내려받기',
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
    aria5: '복구 키 숨기기',
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
    aria5: '문서 고르기',
    title1: '저장하지 않은 내용이 있습니다',
    text19: (a: unknown) => `"${a}" 글에서 고친 내용을 아직 저장하지 않았습니다.`,
    text20: '계속 고치기',
    text21: '버리기',
    text22: '저장',
    text23: '저장했습니다',
    text24: '지난 판으로 되돌렸습니다',
    text25: '다시 불러오기',
    text26: '문서 목록 불러오는 중',
    text27: '아직 문서가 없습니다',
    text28: '문서 찾기',
    text29: '찾는 문서가 없습니다',
    text30: '본문',
    text31: '이어진 것',
    text32: '지난 판',
    text33: '문서를 읽지 못했습니다',
    text34: '이어진 것을 읽지 못했습니다',
    text35: '지난 판을 읽지 못했습니다',
    text36: '제목을 적어 주세요',
    text37: '요약을 적어 주세요',
    text38: (a: unknown) => `저장하지 못했습니다. ${a}`,
    text39: '문서 불러오는 중',
    aria6: '제목',
    text40: '그만두기',
    title2: '이 문서가 그사이 바뀌었습니다',
    text41: '고치는 동안 이 문서가 다른 곳에서 바뀌었습니다. 바뀐 문서를 불러오면 내가 고친 내용이 사라지고, 내 글로 덮어쓰면 그사이 바뀐 내용이 사라집니다.',
    text42: '바뀐 문서 불러오기',
    text43: '내가 고친 것으로 덮어쓰기',
    title3: '고친 내용을 버릴까요?',
    text44: '저장하지 않은 내용은 사라집니다.',
    aria7: '문서 보기',
    text45: '고치기',
    text46: '본문이 비어 있습니다',
    text47: '이어진 것 불러오는 중',
    text48: '지난 판 불러오는 중',
    text49: '저장한 판이 아직 없습니다',
    text50: (a: unknown) => `그때 제목: ${a}`,
    text51: (a: unknown) => `다른 문서 ${a}개와 함께 저장`,
    text52: '종류',
    text53: '상태',
    text54: '만든 때',
    text55: '고친 때',
    text56: '지금은 없는 문서',
    text57: (a: unknown) => `이름을 알 수 없는 ${a}`,
    text58: '이어진 것이 없습니다',
    text59: '이어진 문서',
    text60: '지난 판 목록',
    text61: '되돌리기',
    text62: (a: unknown) => `${a}에 저장한 판`,
    text63: '이 판은 제목과 요약을 읽을 수 없습니다. 파일에 적힌 그대로 보여 줍니다.',
    text64: '지난 판으로 되돌리지 못했습니다',
    text65: '저장하지 않은 내용이 사라집니다',
    text66: (a: unknown) => `이 글에는 저장하지 않은 내용이 있습니다. ${a}에 저장한 판으로 되돌리면 그 내용은 사라집니다. 그래도 되돌릴까요?`,
    text67: '이 판으로 되돌릴까요?',
    text68: (a: unknown) => `${a}에 저장한 판의 내용으로 문서가 바뀝니다.`,
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
  // ---- preferences.ts ----
  preferences: {
    text1: '내 작업공간',
    text2: '참여자 1명',
    text3: '검수 완료 뒤 원자료 삭제',
    text4: '기록마다 선택',
    text5: '원자료 계속 보관',
  },
  // ---- record-persistence.ts ----
  recordPersistence: {
    text1: '저장된 기록 버전을 찾을 수 없습니다',
    text2: '로컬 기록',
    text3: '기록 정리',
    text4: '현재 기록',
    text5: '원문',
    text6: '작성자',
    text7: '기존 로컬 기록 형식을 그대로 읽었습니다.',
  },
  // ---- runtime.ts ----
  runtime: {
    text1: '이 기기의 몸소 작업공간',
    text2: '로컬 구조 데이터',
    text3: '로컬 원자료 폴더',
    text4: '로컬 내보내기 폴더',
    text5: '로컬 백업 폴더',
    text6: '로컬 진단 로그',
    text7: '계정 연결은 설치된 몸소 앱에서 사용할 수 있습니다',
    text8: '연결 도구 설치는 설치된 몸소 앱에서 사용할 수 있습니다',
    text9: 'API 키는 설치된 몸소 앱에서 안전하게 저장할 수 있습니다',
    text10: 'API 키는 설치된 몸소 앱에서 관리할 수 있습니다',
    text11: '전사 보정 사전은 설치된 몸소 앱에 저장됩니다',
    text12: '전사 보정 사전은 설치된 몸소 앱에서 관리할 수 있습니다',
    text13: '원문은 설치된 몸소 앱에서 읽을 수 있습니다',
    text14: '기록 본문은 설치된 몸소 앱에서 읽을 수 있습니다',
    text15: '기록은 설치된 몸소 앱에서 안전하게 저장할 수 있습니다',
    text16: '기록은 설치된 몸소 앱에서 읽을 수 있습니다',
    text17: '기록 변경은 설치된 몸소 앱에서 검토할 수 있습니다',
    text18: '기록 변경은 설치된 몸소 앱에서 저장할 수 있습니다',
    text19: '작업 상태는 설치된 몸소 앱에서 확인할 수 있습니다',
    text20: '원격 백업 공간은 설치된 몸소 앱에서 준비할 수 있습니다',
    text21: '기존 원격 백업은 설치된 몸소 앱에서 연결할 수 있습니다',
    text22: '원격 백업은 설치된 몸소 앱에서 실행할 수 있습니다',
    text23: '원격 복원은 설치된 몸소 앱에서 실행할 수 있습니다',
    text24: '복구 키는 설치된 몸소 앱에서 확인할 수 있습니다',
    text25: '복구 키는 설치된 몸소 앱에서 저장할 수 있습니다',
    text26: 'AI 응답은 설치된 몸소 앱에서 실행할 수 있습니다',
  },
  // ---- source-import.ts ----
  sourceImport: {
    text1: (a: unknown, b: unknown) => `${a} · ${b} · 이미 보존된 원자료와 같은 내용이라 새로 복사하지 않고 이 기록에 연결합니다`,
    text2: (a: unknown, b: unknown) => `${a} · ${b} · 원자료를 복사해 보존합니다`,
    text3: (a: unknown) => `원문이 ${a}를 넘어 본문에 넣지 않고 원자료만 보존합니다`,
    text4: (a: unknown) => `문단 ${a}`,
    text5: '원문',
    text6: '가져온 원문',
    text7: (a: unknown) => `문단 1–${a}`,
    text8: '문단 1',
    text9: '기록 메모',
    text10: '가져오기 메모',
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
