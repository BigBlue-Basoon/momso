// 화면 확인용 가짜 백엔드. 앱 코드에는 들어가지 않고, tests/ui-check/zet-panel.html 에서만 쓴다.
// 주소 끝에 ?case=empty (글 없음), ?case=fail (읽기 실패), ?case=missing (명령 없음) 을 붙일 수 있다.
(function () {
  const scenario = new URLSearchParams(location.search).get('case') || 'normal'
  const withRecords = new URLSearchParams(location.search).has('records')

  // 주소에 ?records=1&chat=few 또는 chat=many 를 붙이면 첫 기록의 AI 대화에 주고받은 말이 들어 있는 상태가 된다
  // (대화 칸 왼쪽의 막대 목차 확인용). 앱이 대화를 기억해 두는 자리(localStorage)에 미리 넣어 둔다.
  const chat = new URLSearchParams(location.search).get('chat')
  const historyKey = 'momso.desktop.assistant-history.v1'
  const preferencesKey = 'momso.desktop.preferences.v1'
  if (chat) {
    const turns = chat === 'many' ? 60 : 3
    const messages = [{ id: 'm-0', role: 'assistant', text: '원문에서 "걸리는 느낌"이라고 한 표현을 그대로 보존했어요.' }]
    for (let i = 1; i <= turns; i += 1) {
      messages.push({ id: `m-u-${i}`, role: 'user', text: `${i}번째로 묻는 말이다. 이 수업에서 어깨 이야기가 나온 곳을 다시 알려 줘.` })
      messages.push({ id: `m-a-${i}`, role: 'assistant', text: `${i}번째 답이다. 31분 42초에 한 수련생이 왼쪽 어깨가 조금 걸린다고 말했고, 강사는 팔을 어깨 높이에서 멈추게 했습니다.\n통증으로 단정하지 않았습니다.` })
    }
    localStorage.setItem(preferencesKey, JSON.stringify({ persistAssistantHistory: true }))
    localStorage.setItem(historyKey, JSON.stringify({ '11111110-1111-4111-8111-111111111111': messages }))
  } else {
    localStorage.removeItem(preferencesKey)
    localStorage.removeItem(historyKey)
  }
  const asset = 'sha256:' + 'a1b2c3d4'.repeat(8)

  const longParagraphs = Array.from({ length: 40 }, (_, index) => (
    `${index + 1}번째 문단입니다. 호흡을 길게 내쉬면서 골반의 무게를 바닥에 내려놓습니다. ` +
    '어깨는 귀에서 멀어지게 두고, 시선은 코끝 아래로 부드럽게 둡니다. 서두르지 않습니다.'
  )).join('\n\n')

  const zets = {
    zet_20261115_143052: {
      head: {
        id: 'zet_20261115_143052',
        title: '저녁 하타 수업 요약',
        summary: '호흡 속도를 낮춘 뒤 고관절과 흉곽을 차례로 열었다. 한 수련생이 왼쪽 어깨가 걸린다고 말해 팔 범위를 줄였다.',
        kind: 'summary',
        status: 'draft',
        created_at: '2026-11-15T14:30:52+09:00',
        updated_at: '2026-11-15T16:05:10+09:00',
        assets: [asset],
        edges: [{ type: 'summarizes', target: 'zet_20261115_143055' }],
      },
      body: [
        '# 수업의 흐름',
        '',
        '초반에는 **앉은 호흡**으로 속도를 낮췄다. 그다음 *고관절*과 흉곽을 차례로 열었다.',
        '줄을 바꿔 쓴 문장은 화면에서도 줄이 바뀐다.',
        '',
        '## 순서',
        '',
        '1. 앉은 호흡 5분',
        '2. 고양이 소 자세',
        '3. 낮은 런지',
        '   - 왼쪽 먼저',
        '   - 오른쪽은 조금 짧게',
        '',
        '## 주의할 점',
        '',
        '- 왼쪽 어깨가 걸린다는 말이 있었다',
        '- 통증으로 단정하지 않는다',
        '- [ ] 다음 수업에서 먼저 묻기',
        '- [x] 팔 범위 줄이기',
        '',
        '> "조금 걸리는 느낌"이라고 했다. 표현을 그대로 둔다.',
        '',
        '| 구간 | 시간 | 한 것 | 메모 |',
        '| --- | --- | --- | --- |',
        '| 시작 | 00:00~08:14 | 앉은 호흡 | 속도를 낮춤 |',
        '| 가운데 | 08:14~31:42 | 고관절과 흉곽 열기 | 왼쪽 어깨에서 팔 범위를 줄임, 설명을 길게 함 |',
        '| 마무리 | 31:42~52:16 | 사바아사나 | 조명을 낮춤 |',
        '',
        '---',
        '',
        '전체 전사는 [보정된 전사](zet_20261115_143055.md)에 있다. 참고한 글: [요가 저널](https://example.com/journal), 주소만 쓴 경우 https://example.com/plain',
        '',
        '`inline code` 와 코드 블록:',
        '',
        '```',
        'const breath = "길게 내쉰다, 아주 길게 내쉰다, 화면 밖으로 넘어갈 만큼 긴 한 줄을 일부러 넣었다"',
        '```',
        '',
        '본문 안의 HTML 은 실행되지 않는다: <b>굵게 아님</b> <script>window.__zetHtmlRan = true</script>',
        '',
        '<div onclick="window.__zetHtmlRan = true">누를 수 없는 글자</div>',
        '',
        '![수업 사진](https://example.com/photo.png)',
      ].join('\n'),
    },
    zet_20261115_143055: {
      head: {
        id: 'zet_20261115_143055',
        title: '저녁 하타 수업 보정된 전사, 제목이 아주 길어서 두 줄로 넘어가는 경우를 확인하려고 일부러 길게 쓴 제목',
        summary: '수업 전체를 받아 적고 용어를 고쳤다.',
        kind: 'transcript',
        status: 'confirmed',
        created_at: '2026-11-15T14:30:55+09:00',
        updated_at: '2026-11-15T14:40:00+09:00',
        assets: [asset],
        edges: [{ type: 'derived_from', target: 'zet_20261114_090000' }],
      },
      // 화자 표시(뒤쪽 규칙): 사람이 확인한 화자는 그냥 이름, AI 가 스스로 가린 화자는 이름 앞에 ?.
      // 사람 이름에는 추정이든 아니든 회색 바탕(2026-11-16). "참고:" 처럼 한 번뿐인 머리말은 사람 이름이 아니다.
      body: ['**00:00 강사** 편하게 앉아서 숨을 고릅니다.', '', '**31:42 ?수련생** 왼쪽 어깨가 조금 걸리는 느낌이에요.', '', '?김성균: 그럼 팔 범위를 줄여 볼게요. 괜찮으세요?', '', '김성균: 네, 그 높이에서 세 번 숨을 쉬겠습니다.', '', '참고: 이 대목은 수업 녹음에서 옮겼습니다.', '', longParagraphs].join(String.fromCharCode(10)),
    },
    zet_20261114_090000: {
      head: {
        id: 'zet_20261114_090000',
        title: '수업 준비 노트',
        summary: '이번 주 수업에서 볼 것.',
        kind: 'note',
        status: 'draft',
        created_at: '2026-11-14T09:00:00+09:00',
        updated_at: '2026-11-14T09:00:00+09:00',
        assets: [],
        edges: [],
      },
      body: '',
    },
    zet_20251201_101500: {
      head: {
        id: 'zet_20251201_101500',
        title: '넓은 표가 있는 글',
        summary: '칸이 많은 표.',
        kind: 'note',
        status: 'confirmed',
        created_at: '2025-12-01T10:15:00+09:00',
        updated_at: '2025-12-01T10:15:00+09:00',
        assets: [],
        edges: [{ type: 'references', target: 'zet_20200101_000000' }],
      },
      body: [
        '| 요일 | 새벽 | 오전 | 점심 | 오후 | 저녁 | 밤 | 담당 | 메모 |',
        '| --- | --- | --- | --- | --- | --- | --- | --- | --- |',
        '| 월 | 하타 | 빈야사 | 쉬는 시간 | 인요가 | 하타 | 명상 | 유동환 원장 | 대체 수업 있음 |',
        '| 화 | 아쉬탕가 | 하타 | 쉬는 시간 | 빈야사 | 인요가 | 명상 | 유동환 원장 | 없음 |',
      ].join('\n'),
    },
  }

  const receipts = {
    rec_20261115_143110: {
      head: {
        id: 'rec_20261115_143110',
        created_at: '2026-11-15T14:31:10+09:00',
        tool: 'claude',
        model: 'claude-fable-5-2',
        read: [asset, 'sha256:' + 'f'.repeat(64), 'zet_20261115_143055', 'zet_20200101_000000'],
        wrote: ['zet_20261115_143052'],
      },
      body: '## 시킨 말\n오늘 저녁 수업을 요약해 줘.\n\n## 고친 내용\n- 요약 글을 새로 만들었다\n- **주의할 점**을 따로 뽑았다\n',
    },
  }

  // 주소에 ?dirty=1 을 붙이면 요약 글에 저장하지 않은 내용이 있는 상태가 된다.
  // ?dirty=hidden 은 화면이 미리 알아채지 못하는 경우(백엔드만 저장하지 않은 내용이 있다고 답하는 경우)다.
  const dirtyMode = new URLSearchParams(location.search).get('dirty')
  const saveFail = new URLSearchParams(location.search).get('savefail')
  const summaryId = 'zet_20261115_143052'
  const savedBody = zets[summaryId].body
  const firstBody = '# 수업의 흐름\n\n처음 저장한 판의 본문이다. **짧다.**'
  const unsaved = { [summaryId]: Boolean(dirtyMode) }
  if (dirtyMode === '1') {
    zets[summaryId].body = savedBody + '\n\n저장하지 않고 덧붙인 줄.'
    zets[summaryId].head.updated_at = '2026-11-15T16:40:00+09:00'
  }

  const history = {
    [summaryId]: [
      { revision: 'b'.repeat(40), time: '2026-11-15T16:05:10+09:00', title: '저녁 하타 수업 요약', otherZets: 0 },
      { revision: 'd'.repeat(40), time: '2026-11-15T16:05:02+09:00', title: '저녁 하타 수업 요약', otherZets: 1 },
      { revision: 'a'.repeat(40), time: '2026-11-15T14:35:00+09:00', title: '저녁 수업 정리', otherZets: 2 },
      { revision: 'e'.repeat(40), time: '2026-11-14T09:10:00+09:00', title: null, otherZets: 0 },
    ],
  }

  function versionOf(id, revision) {
    const zet = zets[id]
    if (revision.startsWith('e')) {
      return { head: null, body: 'title 저녁 수업\n---\n머리를 읽을 수 없는 옛 파일의 내용이다.\n**굵게** 표시도 그대로 보인다.', headUnreadable: true }
    }
    if (revision.startsWith('a')) {
      return {
        head: { ...zet.head, title: '저녁 수업 정리', summary: '처음 저장할 때의 요약이다.', updated_at: '2026-11-15T14:35:00+09:00' },
        body: firstBody,
        headUnreadable: false,
      }
    }
    return {
      head: { ...zet.head, updated_at: '2026-11-15T16:05:10+09:00' },
      body: id === summaryId ? savedBody : zet.body,
      headUnreadable: false,
    }
  }

  // ?manydocs=1000 이면 문서를 그만큼 더 만든다(문서 화면 성능 확인, 2026-11-18). 다섯 폴더에 나눠 담고 일부는 맨 위.
  const manyDocs = Number(new URLSearchParams(location.search).get('manydocs') || 0)
  if (manyDocs > 0 && !window.__manyDocsMade) {
    window.__manyDocsMade = true
    // 기본은 모두 맨 위(가장 무거운 꼴). ?manyfolders=1 이면 다섯 자리에 나눠 담는다.
    const folders = new URLSearchParams(location.search).get('manyfolders') ? ['', '수업 기록/2026-11', '수업 기록/2026-10', '홍보 글', '회원 메모'] : ['']
    window.__zetFolderOf = window.__zetFolderOf || {}
    for (let i = 0; i < manyDocs; i += 1) {
      const id = `zet_many_${String(i).padStart(4, '0')}`
      const day = String(1 + (i % 28)).padStart(2, '0')
      zets[id] = { head: { id, title: `수업 메모 ${i + 1}`, summary: `${i + 1}번째 메모의 요약입니다. 호흡과 자세를 적었다.`, kind: i % 3 === 0 ? 'summary' : 'note', status: i % 2 ? 'draft' : 'confirmed', created_at: `2026-10-${day}T09:00:00Z`, updated_at: `2026-11-${day}T09:${String(i % 60).padStart(2, '0')}:00Z`, assets: [], edges: [] }, body: `${i + 1}번째 메모.` }
      window.__zetFolderOf[id] = folders[i % folders.length]
    }
  }

  function listEntries() {
    return Object.values(zets)
      .map(({ head }) => ({
        id: head.id,
        title: head.title,
        summary: head.summary,
        kind: head.kind,
        status: head.status,
        createdAt: head.created_at,
        updatedAt: head.updated_at,
        folder: (window.__zetFolderOf || {})[head.id] ?? '',
      }))
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  }

  // 글 내용이 바뀌면 달라지는 값(진짜 지문은 아니다).
  function hashOf(zet) {
    const text = JSON.stringify([zet.head, zet.body])
    let sum = 0
    for (let i = 0; i < text.length; i += 1) sum = (sum * 31 + text.charCodeAt(i)) >>> 0
    return 'sha256:' + sum.toString(16).padStart(64, '0')
  }

  function linkedZet(type, id) {
    const target = zets[id]
    return target
      ? { type, id, title: target.head.title, zetKind: target.head.kind, status: target.head.status, missing: false }
      : { type, id, title: null, zetKind: null, status: null, missing: true }
  }

  // ---- 대화(본부가 전한 계약대로 흉내 낸다). 주소에 ?chats=none / ?chats=many 를 붙일 수 있다. ----
  const chatMode = new URLSearchParams(location.search).get('chats') || 'few'
  const pad = (n) => String(n).padStart(2, '0')
  let chatClock = Date.parse('2026-11-15T09:00:00+09:00')
  function nextStamp() {
    chatClock += 61 * 1000
    return new Date(chatClock).toISOString()
  }
  function chatIdFor(time) {
    const d = new Date(time)
    return `chat_${d.getFullYear()}${pad(d.getMonth() + 1)}${pad(d.getDate())}_${pad(d.getHours())}${pad(d.getMinutes())}${pad(d.getSeconds())}`
  }
  const chats = {}
  function seedChat(title, turns, answeredBy) {
    const createdAt = nextStamp()
    const id = chatIdFor(createdAt)
    const messages = []
    for (let i = 1; i <= turns; i += 1) {
      messages.push({ id: `${id}-u${i}`, role: 'user', text: `${i}번째로 묻는 말이다. ${title} 이야기를 더 해 줘.`, createdAt: nextStamp() })
      const by = answeredBy ? answeredBy[i - 1] : null
      messages.push({ id: `${id}-a${i}`, role: 'assistant', text: `${i}번째 답이다. ${title}에서는 호흡을 길게 두고 어깨 높이를 먼저 확인했습니다.`, createdAt: nextStamp(), ...(by || {}) })
    }
    chats[id] = { id, title, createdAt, updatedAt: messages.length ? messages[messages.length - 1].createdAt : createdAt, messages }
  }
  if (chatMode === 'few') {
    seedChat('11월 12일 저녁 하타 수업 돌아보기', 2)
    seedChat('왼쪽 어깨가 걸린다고 한 수련생', 3, [
      // chat_read 의 fast·notHandedOver(T33): 둘째 답은 코덱스에서 클로드로 바꿔 고속 모드로 받은 답, 셋째는 옛 말(둘 다 null).
      // T43: actualEffort(실제로 쓰인 추론 수준), finishReason(답이 끝난 까닭). 첫째는 정상, 둘째는 길이 한도로 끊김, 셋째는 옛 말(null).
      { provider: 'codex', model: 'test-codex-terra', fast: false, notHandedOver: 0, actualEffort: 'medium', finishReason: 'completed' },
      { provider: 'claude', model: 'test-claude-fable', fast: true, fastState: 'on', notHandedOver: 2, actualEffort: 'high', finishReason: new URLSearchParams(location.search).get('end') || 'length_limit' },
      { provider: 'claude', model: 'test-claude-opus', fast: null, notHandedOver: null, actualEffort: null, finishReason: null },
    ])
    seedChat('다음 주 빈야사 순서 짜기', 1)
  } else if (chatMode === 'mixed') {
    // 세 회사의 답이 섞인 대화(답 모양 견주기용). 긴 한글 답, 목록과 굵은 글씨가 있는 답.
    const createdAt = nextStamp()
    const id = chatIdFor(createdAt)
    const say = (role, text, by) => ({ id: `${id}-${role}-${Math.random().toString(16).slice(2, 8)}`, role, text, createdAt: nextStamp(), ...(by || {}) })
    const messages = [
      say('user', '오늘 저녁 하타 수업에서 왼쪽 어깨가 걸린다고 한 수련생에게 다음 수업에서 무엇을 먼저 물어보면 좋을까?'),
      say('assistant', '수업을 시작하기 전에 어깨가 지금 어떤지 먼저 물어보는 것이 좋겠습니다. 지난번에는 팔을 머리 위로 올릴 때 걸린다고 했으니, 오늘은 같은 동작에서 어느 높이부터 불편한지, 숨을 내쉬면서 올릴 때는 어떤지도 같이 들어 보세요. 통증이라고 단정하지 말고 수련생이 쓴 말("걸리는 느낌")을 그대로 쓰면 이야기가 편해집니다.', { provider: 'claude', model: 'test-claude-fable' }),
      say('user', '그럼 자세는 어떻게 바꿔 주면 좋을까? 목록으로 정리해 줘.'),
      say('assistant', ['이렇게 바꿔 볼 수 있습니다.', '- **팔 높이**: 어깨 높이에서 먼저 멈춥니다.', '- **호흡**: 내쉬는 숨에 천천히 올립니다.', '- **벽 짚기**: 손바닥을 벽에 대고 미끄러지듯 올립니다.', '수업 뒤에 어깨가 어땠는지 **꼭 다시** 물어보세요.'].join(String.fromCharCode(10)), { provider: 'codex', model: 'test-codex-terra' }),
      say('user', '다음 주 빈야사 순서에 이걸 어떻게 넣을지 짧게 알려 줘.'),
      say('assistant', '태양 경배 B 의 팔 올리기를 어깨 높이까지만 하는 판으로 바꾸고, 전사 자세 2 에서 팔을 옆으로 뻗는 동안 숨을 길게 내쉬게 하세요. 마지막 휴식 전에 어깨 돌리기를 한 번 넣으면 좋습니다.', { provider: 'gemini', model: 'test-gemini-pro' }),
    ]
    chats[id] = { id, title: '세 회사의 답이 섞인 대화', createdAt, updatedAt: messages[messages.length - 1].createdAt, messages }
  } else if (chatMode === 'long') {
    // ?chats=long&longcount=2000: 긴 채팅방 하나(성능 확인, 2026-11-18). 말 수는 longcount(기본 2000).
    seedChat('긴 채팅방', Math.round(Number(new URLSearchParams(location.search).get('longcount') || 2000) / 2))
  } else if (chatMode === 'many') {
    for (let i = 1; i <= 40; i += 1) seedChat(`${i}번째 대화, 이름이 길면 줄 끝이 말줄임표로 줄어드는지 보려고 길게 쓴 이름`, i === 40 ? 60 : 1)
  }
  // ---- 채팅방 묶음(가짜). ?groups=ok(기본) / none(명령 없음). ?chats=groups 면 지어낸 수련생 한 명의 채팅방 셋을 한 묶음에. ----
  const groupsMode = new URLSearchParams(location.search).get('groups') || 'ok'
  let groupCount = 0
  const chatGroups = []
  function addGroup(name) { groupCount += 1; const g = { id: `group-${groupCount}`, name, order: chatGroups.length }; chatGroups.push(g); return g }
  if (chatMode === 'groups') {
    const g = addGroup('윤하늘 수련생')
    const g2 = addGroup('수업 준비')
    seedChat('윤하늘 수련생 첫 상담', 1)
    seedChat('윤하늘 수련생 어깨 이야기', 2)
    seedChat('윤하늘 수련생 다음 달 목표', 1)
    seedChat('다음 주 빈야사 순서 짜기', 1)
    seedChat('11월 12일 저녁 하타 수업 돌아보기', 1)
    seedChat('명상 수업 음악 고르기', 1)
    const ids = Object.keys(chats)
    chats[ids[0]].groupId = g.id; chats[ids[1]].groupId = g.id; chats[ids[2]].groupId = g.id; chats[ids[3]].groupId = g2.id
  }
  function groupList() {
    if (groupsMode === 'none') throw 'Command chat_group_list not found'
    return chatGroups.slice().sort((a, b) => a.order - b.order).map((g) => ({ ...g, chatCount: Object.values(chats).filter((c) => c.groupId === g.id).length }))
  }
  function chatEntry(chat) {
    return { id: chat.id, title: chat.title, createdAt: chat.createdAt, updatedAt: chat.updatedAt, messageCount: chat.messages.length, groupId: chat.groupId || null, pinned: Boolean(chat.pinned), pinnedOrder: chat.pinnedOrder ?? null }
  }
  // 보관한 채팅방은 목록과 찾기에 오지 않는다(짐작한 계약).
  function chatList() {
    return Object.values(chats).filter((chat) => !chat.archived).map(chatEntry).sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  }
  // ---- 채팅방 고정·보관(가짜, 실제 계약 T38). ?keep=none 이면 명령이 없다. ?keep=seed 면 보관된 채팅방 둘을 미리 둔다. ----
  const keepMode = new URLSearchParams(location.search).get('keep') || 'ok'
  window.__keepCalls = []
  let pinCounter = 0
  function keepCommand(command, args) {
    if (keepMode === 'none') throw `Command ${command} not found`
    window.__keepCalls.push([command, args || {}])
    const chat = args && chats[args.chatId]
    const done = (state) => { if (state === 'ok' || state === 'deleted') window.setTimeout(() => emit('chat-list-updated', { chatId: args.chatId }), 0); return { state } }
    switch (command) {
      case 'chat_pin': if (!chat) return { state: 'not_found' }; chat.pinned = Boolean(args.pinned); chat.pinnedOrder = chat.pinned ? (pinCounter += 1) : null; return done('ok')
      case 'chat_archive': if (!chat) return { state: 'not_found' }; if (running[args.chatId]) return { state: 'busy' }; chat.archived = true; chat.archivedAt = nextStamp(); return done('ok')
      case 'chat_archived_list': return Object.values(chats).filter((c) => c.archived).sort((a, b) => b.archivedAt.localeCompare(a.archivedAt)).map((c) => ({ id: c.id, title: c.title, groupId: c.groupId || null, updatedAt: c.updatedAt, archivedAt: c.archivedAt, provider: null }))
      case 'chat_restore': if (!chat) return { state: 'not_found' }; chat.archived = false; return done('ok')
      case 'chat_delete': if (!chat) return { state: 'not_found' }; delete chats[args.chatId]; return { ...done('deleted'), codexSession: 'not_present', claudeSession: 'not_present' }
    }
    return undefined
  }
  if (keepMode === 'seed') {
    ;['지난달 아쉬탕가 수업 메모', '휴강 공지 문구 다듬기'].forEach((title) => {
      seedChat(title, 1)
      const made = Object.values(chats).find((c) => c.title === title)
      if (made) { made.archived = true; made.archivedAt = nextStamp() }
    })
  }

  // ---- ?big=1: 많이 쌓였을 때(2026-11-17 화면 재기). 채팅방 500개(묶음 20개에 나눔, 고정 10개) + 보관 200개,
  //      말 500개인 채팅방 하나(긴 답·코드·표), 문서 1,000개, 미디어 300개, 용어 1,000개, 사람 이름 300개. ----
  const bigMode = new URLSearchParams(location.search).has('big')
  if (bigMode) {
    const NL = String.fromCharCode(10)
    const groups = Array.from({ length: 20 }, (_, i) => addGroup(`${i + 1}번 묶음 수련생들`))
    for (let i = 1; i <= 500; i += 1) {
      seedChat(`${i}번째 채팅방, 수업과 수련생 이야기`, 1)
    }
    const all = Object.values(chats).filter((c) => /번째 채팅방/.test(c.title))
    all.forEach((c, i) => { if (i % 3 !== 0) c.groupId = groups[i % 20].id; if (i >= all.length - 10) { c.pinned = true; c.pinnedOrder = i } })
    for (let i = 1; i <= 200; i += 1) {
      seedChat(`${i}번째 보관한 채팅방`, 1)
      const made = Object.values(chats).find((c) => c.title === `${i}번째 보관한 채팅방`)
      made.archived = true; made.archivedAt = nextStamp()
    }
    // 말 500개인 채팅방: 긴 답, 코드, 표.
    const createdAt = nextStamp()
    const id = chatIdFor(createdAt)
    const messages = []
    for (let i = 1; i <= 250; i += 1) {
      messages.push({ id: `${id}-u${i}`, role: 'user', text: `${i}번째 물음: 다음 수업 순서를 표로 정리하고, 기록용 코드도 보여 줘.`, createdAt: nextStamp() })
      const long = Array.from({ length: 6 }, (_, k) => `${k + 1}. 호흡을 길게 내쉬며 골반의 무게를 내려놓고, 어깨는 귀에서 멀어지게 둡니다. 서두르지 않고 다음 자세로 넘어갑니다.`).join(NL)
      const table = ['| 순서 | 자세 | 시간 |', '| --- | --- | --- |', '| 1 | 앉은 호흡 | 5분 |', '| 2 | 고양이 소 | 3분 |', '| 3 | 낮은 런지 | 4분 |'].join(NL)
      const code = ['```js', 'const flow = ["호흡", "고양이 소", "런지"]', 'for (const pose of flow) console.log(pose)', '```'].join(NL)
      messages.push({ id: `${id}-a${i}`, role: 'assistant', text: [`**${i}번째 답**입니다.`, long, table, code].join(NL + NL), createdAt: nextStamp(), provider: ['codex', 'claude', 'gemini'][i % 3], model: null })
    }
    chats[id] = { id, title: '말이 500개인 채팅방', createdAt, updatedAt: messages[messages.length - 1].createdAt, messages }
    // 문서 1,000개.
    for (let i = 1; i <= 1000; i += 1) {
      const zid = `zet_2026${pad(1 + (i % 11))}${pad(1 + (i % 27))}_${String(100000 + i).slice(1)}`
      const at = new Date(Date.parse('2026-11-15T12:00:00+09:00') - i * 3600000).toISOString()
      zets[zid] = { head: { id: zid, title: `${i}번째 문서, 수업 기록`, summary: `${i}번째 수업에서 호흡과 어깨를 살폈다.`, kind: ['summary', 'note', 'transcript'][i % 3], status: 'draft', created_at: at, updated_at: at, assets: [], edges: [] }, body: `# ${i}번째 문서` + NL + NL + '호흡을 길게 내쉬었다.' }
    }
  }

  // ---- 사용량(본부가 전한 계약대로 흉내 낸다). 주소에 ?usage=mixed(기본) / all-ok / none / low / fail 를 붙일 수 있다. ----
  const usageMode = new URLSearchParams(location.search).get('usage') || 'mixed'
  function soon(hours) {
    return new Date(Date.now() + hours * 3600 * 1000).toISOString()
  }
  function usageStatus() {
    if (usageMode === 'fail') throw 'Command usage_status not found'
    const checkedAt = new Date(Date.now() - 2 * 60 * 1000).toISOString()
    const ok = (provider, plan, five, week) => ({ provider, state: 'ok', reason: null, plan, checkedAt, windows: [{ kind: '5h', usedPercent: five, resetsAt: soon(2.5) }, { kind: 'weekly', usedPercent: week, resetsAt: soon(80) }] })
    const notConnected = (provider) => ({ provider, state: 'not_connected', reason: 'not_connected', plan: null, checkedAt, windows: [] })
    const unavailable = (provider, reason = 'fetch_failed') => ({ provider, state: 'unavailable', reason, plan: null, checkedAt, windows: [] })
    // ?usage=reasons1 / reasons2: G18 의 까닭 값 여섯을 세 곳에 나눠 보인다(checking·not_installed·not_connected / api_key·no_quota_api·fetch_failed).
    if (usageMode === 'reasons1') return [unavailable('codex', 'checking'), { ...unavailable('claude', 'not_installed'), state: 'not_connected' }, notConnected('gemini')]
    if (usageMode === 'reasons3') return [unavailable('codex', 'fetch_failed'), unavailable('claude', 'checking'), unavailable('gemini', 'api_key')]
    if (usageMode === 'reasons2') return [unavailable('codex', 'api_key'), unavailable('claude', 'no_quota_api'), unavailable('gemini', 'fetch_failed')]
    if (usageMode === 'all-ok') return [ok('codex', 'Plus', 38, 12), ok('claude', 'Pro', 64, 41), ok('gemini', 'AI Pro', 5, 2)]
    if (usageMode === 'scopes') return [
      { ...ok('codex', 'pro', 38, 12), planLabel: '보기 요금제' },
      { provider: 'claude', state: 'ok', reason: null, plan: 'max', planLabel: null, checkedAt, windows: [
        { kind: 'session', usedPercent: 22, resetsAt: soon(3) },
        { kind: 'weekly_all', usedPercent: 47, resetsAt: soon(90) },
        { kind: 'weekly_scoped', scope: 'Fable', usedPercent: 81, resetsAt: soon(90) },
        { kind: 'other', label: '보기 갈래', usedPercent: 10, resetsAt: null },
      ] },
      notConnected('gemini'),
    ]
    // ?usage=nominutes: 기간(분)을 받지 못한 창(primary·secondary, 클로드 session)과 받은 창(15_minutes·2_days).
    if (usageMode === 'nominutes') return [
      { provider: 'codex', state: 'ok', reason: null, plan: 'pro', checkedAt, windows: [{ kind: 'primary', usedPercent: 30, resetsAt: null }, { kind: 'secondary', usedPercent: 12, resetsAt: soon(40) }, { kind: '15_minutes', usedPercent: 5, resetsAt: soon(0.2) }, { kind: '2_days', usedPercent: 50, resetsAt: soon(30) }] },
      { provider: 'claude', state: 'ok', reason: null, plan: 'max', planLabel: null, checkedAt, windows: [{ kind: 'session', usedPercent: 22, resetsAt: soon(3) }, { kind: 'weekly_all', usedPercent: 47, resetsAt: soon(90) }] },
      notConnected('gemini'),
    ]
    if (usageMode === 'none') return [notConnected('codex'), notConnected('claude'), notConnected('gemini')]
    // 지금 실제 백엔드와 같은 모습: Codex 만 숫자, 나머지는 까닭 글과 함께 unavailable.
    if (usageMode === 'real-like') return [{ provider: 'codex', state: 'ok', reason: null, plan: 'pro', checkedAt, windows: [{ kind: 'weekly', usedPercent: 54, resetsAt: Math.round(Date.now() / 1000) + 50 * 3600 }] }, unavailable('claude', 'no_quota_api'), unavailable('gemini', 'fetch_failed')]
    if (usageMode === 'low') return [ok('codex', 'Plus', 96, 70), ok('claude', 'Pro', 100, 88), ok('gemini', 'AI Pro', 91, 30)]
    return [ok('codex', 'Plus', 38, 12), unavailable('claude'), notConnected('gemini')]
  }

  // ---- 모델 목록, 권한 단계, 보내기, 대화가 찬 정도(화면 확인용 가짜). ----
  // ?models=ok(기본) / partial(Gemini 를 읽지 못함) / fail(명령 없음)
  // ?perm=ok(기본, 백엔드처럼 모두 쓸 수 없음) / some(수동과 계획만 쓸 수 있음) / fail(명령 없음)
  // ?send=new(기본, chat_send 와 이벤트) / old(chat_send 없음) · ?context=ok(기본) / none(알 수 없음) / fail(명령 없음)
  const query = new URLSearchParams(location.search)
  const modelsMode = query.get('models') || 'ok'
  const permMode = query.get('perm') || 'ok'
  const sendMode = query.get('send') || 'new'
  const contextMode = query.get('context') || 'ok'
  let permissionMode = 'manual'
  // ?models=ok(기본) / partial(Gemini 를 읽지 못함) / one(Claude 만 ok) / fail(명령 없음). 이름은 화면 확인용 가짜.
  function aiModels() {
    if (modelsMode === 'fail') throw 'Command ai_models not found'
    const three = ['low', 'medium', 'high']
    const list = [
      { provider: 'codex', state: 'ok', reason: null, models: [
        { id: 'test-codex-terra', family: 'terra', label: 'Terra', resolved: 'test-codex-terra-0', efforts: three, defaultEffort: 'medium', isDefault: true, fast: { supported: true, state: 'on', label: '보기 빠름' } },
        { id: 'test-codex-sol', family: 'sol', label: 'Sol', resolved: null, efforts: three, defaultEffort: 'medium' },
        { id: 'test-codex-luna', family: 'luna', label: 'Luna', resolved: null, efforts: ['low'], defaultEffort: 'low' },
      ] },
      { provider: 'claude', state: 'ok', reason: null, models: [
        { id: 'test-claude-fable', family: 'fable', label: 'Fable', resolved: null, defaultEffort: 'high', isDefault: true,
          effortNote: '보기 설명 한 줄: 수준이 높을수록 더 오래 생각합니다.',
          efforts: [
            { id: 'low', label: 'low', description: '보기 설명 하나', special: false, available: true },
            { id: 'medium', label: 'medium', description: null, special: false, available: true },
            { id: 'high', label: 'high', description: '보기 설명 둘', special: false, available: true },
            { id: 'max', label: 'max', description: null, special: false, available: false },
            { id: 'ultra', label: 'ultra', description: '보기 특별 수준', special: true, available: true },
          ],
          fast: { supported: true, state: 'off', label: '보기 빠름' } },
        { id: 'test-claude-opus', family: 'opus', label: 'Opus', resolved: null, efforts: three, defaultEffort: 'high', fast: { supported: true, state: 'cooldown', label: '보기 빠름' } },
        { id: 'test-claude-sonnet', family: 'sonnet', label: 'Sonnet', resolved: null, efforts: three, defaultEffort: 'medium', fast: { supported: true, state: 'unavailable', label: '보기 빠름' } },
        { id: 'test-claude-haiku', family: 'haiku', label: 'Haiku', resolved: null, efforts: [], defaultEffort: null },
      ] },
      { provider: 'gemini', state: 'ok', reason: null, models: [
        { id: 'test-gemini-pro', family: 'pro', label: 'Pro', resolved: null, efforts: ['low', 'high'], defaultEffort: 'high', isDefault: true },
        { id: 'test-gemini-flash', family: 'flash', label: 'Flash', resolved: null, efforts: null, defaultEffort: null },
      ] },
    ]
    // ?models=slider: 추론 수준 슬라이더 보기. 코덱스 테라 넷, 루나 둘, 클로드 페이블 다섯(중간 추천 + 켜고 끄는 항목 하나), 오푸스 다섯(끝 하나 못 씀).
    if (modelsMode === 'slider') {
      const item = (id, extra) => ({ id, label: id, description: null, special: false, available: true, ...(extra || {}) })
      return [
        { provider: 'codex', state: 'ok', reason: null, models: [
          { id: 'gpt-6.1-terra', family: null, label: 'GPT-6.1 Terra', resolved: null, efforts: ['low', 'medium', 'high', 'xhigh'], defaultEffort: 'high', isDefault: true },
          { id: 'gpt-6-luna', family: null, label: 'GPT-6-Luna', resolved: null, efforts: ['low', 'high'], defaultEffort: 'low', isDefault: false },
        ] },
        { provider: 'claude', state: 'ok', reason: null, models: [
          // T43: 고르지 않았을 때 실제로 쓰이는 수준(effectiveDefaultEffort). ?effective=none 이면 null.
          { id: 'claude-fable-5-2', family: 'Fable', label: 'Fable 5.2', resolved: 'claude-fable-5-2', isDefault: true, defaultEffort: null, effectiveDefaultEffort: query.get('effective') === 'none' ? null : 'xhigh',
            efforts: [item('low'), item('medium', { recommended: true }), item('high'), item('xhigh'), item('max'), item('ultracode', { label: 'Ultracode', special: true, kind: 'toggle' })] },
          { id: 'opus', family: 'Opus', label: 'Opus 5.5', resolved: 'claude-opus-5-5', isDefault: null, defaultEffort: 'high',
            efforts: [item('low'), item('medium', { recommended: true }), item('high'), item('xhigh'), item('max', { available: false })] },
        ] },
        { provider: 'gemini', state: 'unavailable', reason: null, models: [] },
      ]
    }
    // ?models=real: 실제 백엔드가 주는 이름 모양 그대로(제각각인 GPT 이름, Claude 별칭). 계열마다 하나와 이름 모양을 본다.
    if (modelsMode === 'real') {
      const e6 = ['low', 'medium', 'high', 'xhigh', 'max', 'ultra']
      const e5 = ['low', 'medium', 'high', 'xhigh', 'max']
      return [
        { provider: 'codex', state: 'ok', reason: null, models: [
          { id: 'gpt-6.1-sol', family: null, label: 'GPT-6.1-Sol', resolved: null, efforts: e6, defaultEffort: 'low', isDefault: true },
          { id: 'gpt-6-astra', family: null, label: 'GPT-6-Astra', resolved: null, efforts: e6, defaultEffort: 'medium', isDefault: false },
          { id: 'gpt-6.1-terra', family: null, label: 'GPT-6.1 Terra', resolved: null, efforts: e6, defaultEffort: 'low', isDefault: false },
          { id: 'gpt-6-sol', family: null, label: 'GPT-6-Sol', resolved: null, efforts: e6, defaultEffort: 'medium', isDefault: false },
          { id: 'gpt-6-luna', family: null, label: 'GPT-6-Luna', resolved: null, efforts: e5, defaultEffort: 'medium', isDefault: false },
          { id: 'gpt-5.6-terra', family: null, label: 'GPT-5.6-Terra', resolved: null, efforts: e6, defaultEffort: 'medium', isDefault: false },
        ] },
        { provider: 'claude', state: 'ok', reason: null, models: [
          { id: 'default', family: null, label: 'Default (recommended)', resolved: 'claude-fable-5-2', efforts: e5, defaultEffort: null, isDefault: true },
          { id: 'opus', family: 'Opus', label: 'Opus 5.5', resolved: 'claude-opus-5-5', efforts: e5, defaultEffort: null, isDefault: null },
          { id: 'claude-fable-5-2', family: 'Fable', label: 'Fable 5.2', resolved: 'claude-fable-5-2', efforts: e5, defaultEffort: null, isDefault: null },
          { id: 'sonnet', family: 'Sonnet', label: 'Sonnet 5.5', resolved: 'claude-sonnet-5-5', efforts: e5, defaultEffort: null, isDefault: null },
          { id: 'haiku', family: 'Haiku', label: 'Haiku 5.5', resolved: 'claude-haiku-5-5', efforts: e5, defaultEffort: null, isDefault: null },
          { id: 'claude-opus-4-8', family: 'Opus', label: 'Opus 4.8', resolved: 'claude-opus-4-8', efforts: e5, defaultEffort: null, isDefault: null },
        ] },
        { provider: 'gemini', state: 'unavailable', reason: null, models: [] },
      ]
    }
    if (modelsMode === 'partial') list[2] = { provider: 'gemini', state: 'unavailable', reason: null, models: [] }
    if (modelsMode === 'one') { list[0] = { provider: 'codex', state: 'not_connected', reason: null, models: [] }; list[2] = { provider: 'gemini', state: 'unavailable', reason: null, models: [] } }
    return list
  }
  // 권한 단계(회사마다 다름). ?perm=ok(기본) / old(예전 다섯 값 계약) / fail(명령 없음). 이름과 설명은 일부러 가짜 글.
  const permissionByProvider = permMode === 'real' ? { codex: 'Ask for approval', claude: 'default', gemini: 'default' } : { codex: 'codex-test-2', claude: 'claude-test-1', gemini: 'gemini-test-1' }
  function permissionModes(provider) {
    if (permMode === 'fail') throw 'Command permission_modes_available not found'
    if (permMode === 'old') return ['manual', 'accept_edits', 'plan', 'auto', 'bypass'].map((mode) => ({ mode, available: false, reason: null }))
    const long = '보기 설명: 고르기 창 안에서 두 줄로 넘어가는지 보려고 일부러 길게 쓴 설명입니다.'
    // ?perm=real: 실제 백엔드가 주는 id 와 영어 글 그대로(2026-11-16 실제 앱에서 읽음). 화면이 한국어로 바꿔 보이는지 본다.
    if (permMode === 'real') {
      if (provider === 'codex') return [
        { id: 'Ask for approval', label: 'Ask for approval', description: 'ask to edit external files and use the internet', available: true, isDefault: true },
        { id: 'Approve for me', label: 'Approve for me', description: 'actions detected as potentially unsafe', available: true, isDefault: false },
        { id: 'Full access', label: 'Full access', description: 'internet and any file on your computer', available: true, isDefault: false },
      ]
      if (provider === 'claude') return [
        { id: 'default', label: 'Manual', description: 'Prompts for permission on first use of each tool', available: true, isDefault: true },
        { id: 'acceptEdits', label: 'Accept edits', description: 'Automatically accepts file edits', available: true, isDefault: false },
        { id: 'plan', label: 'Plan', description: 'doesn’t edit your source files', available: true, isDefault: false },
        { id: 'auto', label: 'Auto', description: 'a background classifier checks', available: true, isDefault: false },
        { id: 'bypassPermissions', label: 'Bypass permissions', description: 'Skips permission prompts', available: true, isDefault: false },
      ]
      return [
        { id: 'default', label: 'Default', description: 'Prompts for approval', available: true, isDefault: true },
        { id: 'auto_edit', label: 'Auto Edit', description: 'Auto-approves edit tools', available: true, isDefault: false },
        { id: 'plan', label: 'Plan', description: 'Read-only mode', available: true, isDefault: false },
        { id: 'yolo', label: 'YOLO', description: 'Auto-approves all tools', available: true, isDefault: false },
      ]
    }
    if (provider === 'codex') return [
      { id: 'codex-test-1', label: '보기 단계 하나', description: long, available: true, isDefault: false },
      { id: 'codex-test-2', label: '보기 단계 둘', description: '보기 설명: 짧은 설명.', available: true, isDefault: true },
      { id: 'codex-test-3', label: '보기 단계 셋', description: long, available: false, isDefault: false, warning: '보기 경고: 가장 높은 단계의 경고 자리.' },
    ]
    if (provider === 'claude') return [
      { id: 'claude-test-1', label: '보기 단계 하나', description: '보기 설명: 짧은 설명.', available: true, isDefault: true },
      { id: 'claude-test-2', label: '보기 단계 둘', description: long, available: true, isDefault: false },
      { id: 'claude-test-3', label: '보기 단계 셋', description: null, available: true, isDefault: false },
      { id: 'claude-test-4', label: '보기 단계 넷', description: long, available: false, isDefault: false },
    ]
    return [
      { id: 'gemini-test-1', label: '보기 단계 하나', description: '보기 설명: 짧은 설명.', available: true, isDefault: true },
      { id: 'gemini-test-2', label: '보기 단계 둘', description: long, available: true, isDefault: false },
    ]
  }
  function emit(event, payload) {
    for (const handler of listeners[event] || []) callbacks[handler]?.({ event, id: handler, payload })
  }
  window.__emit = emit
  window.__emitCompacted = (chatId) => emit('chat-compacted', { chatId })
  const running = {}
  function chatSend({ chatId, text, provider, model, effort, fast, skill, retryMessageId }) {
    if (sendMode === 'old') throw 'Command chat_send not found'
    const chat = chats[chatId]
    if (!chat) throw '대화를 찾지 못했습니다'
    window.__sendCalls = (window.__sendCalls || []).concat([{ chatId, provider, model, effort, fast, skill: skill ?? null }])
    // 실제 chat_send(T33): 코덱스의 fast 는 그대로 전달(chat-done 의 fastState 는 unavailable).
    // ?fast=refusenow: 클로드 고속 모드를 보내기 전에 쓸 수 없다고 확인(말을 저장하지 않고 fast_unavailable).
    // ?fast=refuse: 보낸 뒤 실패(말은 저장되고 다시 보내기 표시, chat-error). 같은 글을 다시 보내면 그 말을 다시 쓴다.
    if (provider === 'claude' && fast && query.get('fast') === 'refusenow') return { status: 'fast_unavailable' }
    const last = chat.messages[chat.messages.length - 1]
    // retryMessageId(T52·T55): 답을 받지 못한 내 말을 그대로 다시 쓴다(새 말을 더하지 않음). 그런 말이 아니거나 ?retryfail 이면 retry_unavailable.
    const retried = retryMessageId ? chat.messages.find((m) => m.id === retryMessageId && m.role === 'user' && m.failed) : null
    if (retryMessageId && (!retried || query.has('retryfail'))) return { status: 'retry_unavailable' }
    window.__retryCalls = (window.__retryCalls || []).concat(retryMessageId ? [retryMessageId] : [])
    const reuse = Boolean(retried) || (last && last.role === 'user' && last.retryableFast && last.text === text)
    const userMessage = retried || (reuse ? last : { id: `${chatId}-m${chat.messages.length + 1}`, role: 'user', text, createdAt: nextStamp(), skill: skill ?? null })
    if (retried) { delete retried.failed; if (query.get('chaterror') && window.__retryOk) window.__failOff = true }
    else if (reuse) { delete last.retryableFast; last.skill = skill ?? null } else chat.messages.push(userMessage)
    chat.updatedAt = userMessage.createdAt
    if (!chat.title) chat.title = text.trim().slice(0, 40)
    if (provider === 'claude' && fast && query.get('fast') === 'refuse') {
      userMessage.retryableFast = true
      window.setTimeout(() => emit('chat-error', { chatId, status: 'fast_unavailable', reason: '', notHandedOver: 0 }), 300)
      return { status: 'started' }
    }
    const lastAnswer = [...chat.messages].reverse().find((m) => m.role === 'assistant')
    const switched = Boolean(lastAnswer && lastAnswer.provider && lastAnswer.provider !== provider)
    const pieces = ['받은 말: "', text, '"' + String.fromCharCode(10) + String.fromCharCode(10), '**굵은 ', '글씨**와 목록:' + String.fromCharCode(10), '- 하나' + String.fromCharCode(10) + '- ', '둘' + String.fromCharCode(10) + String.fromCharCode(10), '```' + String.fromCharCode(10) + '코드 ', '조각' + String.fromCharCode(10) + '```']
    let at = 0
    // ?chaterror=usageLimit 처럼: 답이 조금 오다가 G18 chat-error 로 끊긴다(그때까지 온 글은 답으로 저장). retryAt 은 ?retryin=분.
    const failKind = window.__failOff ? null : query.get('chaterror')
    const FAIL_ACTIONS = { usageLimit: ['wait', 'switchProvider'], rateLimited: ['wait', 'retry', 'switchProvider'], authentication: ['signIn', 'switchProvider'], billing: ['checkBilling', 'switchProvider'], modelUnavailable: ['switchModel', 'switchProvider'], connectionClosed: ['retry', 'switchProvider'], timeout: ['retry', 'switchProvider'], toolMissing: ['install', 'switchProvider'], serviceUnavailable: ['retry', 'wait', 'switchProvider'], contextTooLong: ['shorten', 'switchProvider'], unknown: [] }
    const timer = setInterval(() => {
      if (failKind && at >= 3) {
        clearInterval(timer)
        delete running[chatId]
        const partial = { id: `${chatId}-m${chat.messages.length + 1}`, role: 'assistant', text: pieces.slice(0, at).join(''), createdAt: nextStamp(), provider, model, effort, finishReason: 'error' }
        chat.messages.push(partial)
        const retryAt = query.get('retryin') ? Math.round(Date.now() / 1000) + Number(query.get('retryin')) * 60 : null
        userMessage.failed = true
        emit('chat-error', { chatId, messageId: partial.id, userMessageId: userMessage.id, reason: '', ...(failKind === 'authentication' ? { status: 'auth_failed' } : {}), error: { kind: failKind, retryAt, actions: FAIL_ACTIONS[failKind] || [], providerCode: null } })
        return
      }
      if (at < pieces.length) {
        emit('chat-stream', { chatId, delta: pieces[at] })
        at += 1
        return
      }
      clearInterval(timer)
      delete running[chatId]
      // 실제(T33): 답에 fast(확인된 true/false, 모르면 null)와 notHandedOver 를 저장한 뒤 chat-done 을 보낸다.
      // 클로드에 고속 모드로 보냈으면 fastState on, 코덱스는 확인되지 않아 unavailable.
      const fastState = fast ? (provider === 'claude' ? 'on' : 'unavailable') : null
      const omitted = switched ? 2 : 0
      const message = { id: `${chatId}-m${chat.messages.length + 1}`, role: 'assistant', text: pieces.join(''), createdAt: nextStamp(), provider, model, effort, fast: fastState === 'on' ? true : null, notHandedOver: omitted }
      chat.messages.push(message)
      chat.updatedAt = message.createdAt
      emit('chat-done', { chatId, messageId: message.id, notHandedOver: omitted, ...(fastState ? { fastState } : {}) })
    }, 250)
    running[chatId] = timer
    return { status: 'started', userMessageId: userMessage.id, handoffLimit: null }
  }

  // ---- 허락을 구하는 물음(화면 확인용 가짜). window.__approve('command'|'file_change'|'other', ['allow_once', ...]) 로 띄운다. ----
  const approvalsPending = {}
  let approvalCount = 0
  window.__approve = (kind, options, extra) => {
    approvalCount += 1
    const requestId = `approval-${approvalCount}`
    const description = kind === 'command'
      ? 'npm run build && npm test -- --watch=false'
      : kind === 'file_change'
        ? ['C:', 'example', 'archive', 'zets', 'zet_20261115_143052.md'].join(String.fromCharCode(92))
        : '인터넷에서 요가 동작 이름의 영어 표기를 찾아보려고 합니다'
    const request = { requestId, provider: 'claude', kind, description, reason: null, options, ...(extra || {}) }
    approvalsPending[requestId] = request
    emit('agent-approval-request', request)
    return requestId
  }
  // reason: responded / timeout / provider_cancelled / auto_declined(2026-11-17 계약). 없으면 옛 값.
  window.__closeApproval = (requestId, reason) => {
    delete approvalsPending[requestId]
    emit('agent-approval-closed', reason ? { requestId, reason } : { requestId })
  }
  // 스킬로 하는 연결의 물음(가짜). archive-backend-port-notes.md "2026-11-16 옮긴 것"의 C9 설정 승인 계약 모양:
  //   kind 'setup_change', setup { action:'create_and_connect', service, account, region, resourceName, visibility:'private', cost:'possible' },
  //   targets [{ path: 이름, change:'create' }], options allow_once·deny·cancel. window.__approveSetup({ service, account, ... }).
  // 화면은 "이번만 허락"과 "거절"만 보여야 한다(일부러 allow_session 도 넣는다).
  window.__approveSetup = (setup) => {
    const next = { action: 'create_and_connect', service: 'github', account: null, region: null, resourceName: null, visibility: 'private', cost: 'possible', ...(setup || {}) }
    return window.__approve('setup_change', ['allow_once', 'allow_session', 'deny', 'cancel'], {
      provider: 'claude',
      description: next.service === 'github' ? 'momso_github_create_and_connect' : 'momso_object_store_create_and_connect',
      targets: next.resourceName ? [{ path: next.resourceName, change: 'create' }] : [],
      change: 'create',
      setup: next,
    })
  }
  // 키 넣기(가짜). window.__askSecret({ service, account, resourceName, fields }). 넣은 값은 기록하지 않고 칸 이름과 값의 길이만 남긴다.
  let secretCount = 0
  window.__secretCalls = []
  window.__askSecret = (extra) => { // 계약: fields 는 칸 이름 글자의 목록(GCP 는 gcpOauthToken 도)
    secretCount += 1
    const requestId = `secret-${secretCount}`
    const chatId = Object.keys(chats).find((id) => chats[id].title.includes('어깨')) || null
    emit('agent-setup-secret-request', { requestId, chatId, service: 'cloudflare', account: '0123456789abcdef0123456789abcdef', resourceName: 'momso-teacher', fields: ['accessKeyId', 'secretAccessKey'], ...(extra || {}) })
    return requestId
  }
  // 키를 넣기 전에 뒤쪽이 요청을 닫은 것처럼(5분 지남). window.__closeSecret('secret-1')
  // ---- 패널 브라우저(가짜). 진짜 웹뷰 대신 받은 자리에 회색 상자를 그려 자리·숨김을 눈으로 본다. ?panelfail=1 이면 unavailable. ----
  //   window.__panelDownload() 는 내려받기 알림, window.__panelGoogle(siteUrl) 은 Google 로그인 알림, window.__panelState 로 받은 값을 본다.
  window.__panelState = { open: false, visible: false, bounds: null, url: null, history: [], calls: [] }
  function panelBox() {
    let box = document.getElementById('fake-panel-webview')
    if (!box) {
      box = document.createElement('div')
      box.id = 'fake-panel-webview'
      box.style.cssText = 'position:fixed;z-index:9999;background:#e9ecef;border:1px dashed #9aa5b1;display:flex;align-items:center;justify-content:center;flex-direction:column;gap:6px;font:12px sans-serif;color:#52606d;pointer-events:none'
      document.body.appendChild(box)
    }
    const s = window.__panelState
    box.style.display = s.open && s.visible && s.bounds ? 'flex' : 'none'
    if (s.bounds) Object.assign(box.style, { left: s.bounds.x + 'px', top: s.bounds.y + 'px', width: s.bounds.width + 'px', height: s.bounds.height + 'px' })
    box.textContent = '가짜 웹뷰 · ' + (s.url || '')
    return box
  }
  function panelGo(url) {
    const parsed = (() => { try { return new URL(url) } catch { return null } })()
    if (!parsed || !/^https?:$/.test(parsed.protocol) || /^(localhost|127\.|\[::1\])/.test(parsed.hostname) || parsed.username) throw 'invalid_url'
    if (parsed.hostname === 'accounts.google.com') { emit('panel-browser-google-login', { host: 'accounts.google.com', siteUrl: window.__panelState.url }); throw 'google_login_required' }
    const s = window.__panelState
    s.url = parsed.href
    s.history = s.history.slice(0, (s.at ?? -1) + 1).concat([parsed.href]); s.at = s.history.length - 1
    emit('panel-browser-page', { url: parsed.href, host: parsed.hostname, secure: parsed.protocol === 'https:', loading: true })
    setTimeout(() => {
      emit('panel-browser-page', { url: parsed.href, host: parsed.hostname, secure: parsed.protocol === 'https:', loading: false })
      emit('panel-browser-title', { title: parsed.hostname === 'example.com' ? 'Example Domain' : parsed.hostname + ' 가짜 제목' })
      emit('panel-browser-history', { canGoBack: s.at > 0, canGoForward: s.at < s.history.length - 1 })
      panelBox()
    }, 300)
  }
  function panelCommand(command, args) {
    const s = window.__panelState
    s.calls.push([command, JSON.stringify(args || {})])
    if (query.get('panelfail') === '1') throw 'unavailable'
    switch (command) {
      case 'panel_browser_open':
        if (s.open) throw 'already_open'
        if (!(args.width > 0 && args.height > 0)) throw 'invalid_bounds'
        panelGo(args.url)
        Object.assign(s, { open: true, visible: true, bounds: { x: args.x, y: args.y, width: args.width, height: args.height } })
        panelBox(); return null
      case 'panel_browser_close': Object.assign(s, { open: false, visible: false, url: null, history: [], at: -1 }); panelBox(); return null
      case 'panel_browser_visibility': if (!s.open) throw 'unavailable'; s.visible = args.visible; panelBox(); return null
      case 'panel_browser_bounds': if (!s.open) throw 'unavailable'; s.bounds = { x: args.x, y: args.y, width: args.width, height: args.height }; panelBox(); return null
      case 'panel_browser_navigate': if (!s.open) throw 'unavailable'; panelGo(args.url); return null
      case 'panel_browser_action': {
        if (!s.open) throw 'unavailable'
        const step = args.action === 'back' ? -1 : args.action === 'forward' ? 1 : 0
        if (step && s.history[s.at + step]) { s.at += step; const url = s.history[s.at]; s.history = s.history.slice(); s.url = url; const p = new URL(url); emit('panel-browser-page', { url, host: p.hostname, secure: p.protocol === 'https:', loading: false }); emit('panel-browser-title', { title: p.hostname + ' 가짜 제목' }); emit('panel-browser-history', { canGoBack: s.at > 0, canGoForward: s.at < s.history.length - 1 }); panelBox() }
        return null
      }
      case 'panel_browser_open_human': if (query.get('humanmissing') === '1') throw 'browser_missing'; s.human = args.url; return null
      case 'panel_browser_download_select': s.selected = args.path; return null
      case 'panel_download_reveal': s.revealed = args.path; return null
      case 'panel_browser_open_agent':
        if (s.open) throw 'already_open'
        Object.assign(s, { open: true, visible: true, url: null, agentOpened: true, bounds: { x: args.x, y: args.y, width: args.width, height: args.height } })
        panelBox(); return null
      case 'panel_agent_status': return { ownerChatId: s.agentOwner || null, humanControls: Boolean(s.agentHuman) }
      case 'panel_agent_take_over': s.agentHuman = true; emit('panel-agent-control-changed', { chatId: args.chatId, state: 'human' }); return null
      // 바깥 로그인(가짜, G75): 몸소 전용 창을 연 것으로 치고 opened 를 보낸다. 그 뒤 돌려주기는 external-continued started.
      case 'panel_agent_external_login_open':
        s.externalOpened = true
        emit('panel-agent-external-login-opened', { chatId: args.chatId, state: 'login', message: '' })
        return { state: 'login' }
      case 'panel_agent_resume':
        if (s.externalOpened) {
          s.externalOpened = false; s.agentLogin = false
          emit('panel-agent-external-continued', { chatId: args.chatId, state: 'started', message: '' })
          return { state: 'started', userMessageId: 'ext-1' }
        }
        if (s.agentLogin) {
          if (query.get('agentlogin') === 'notyet') throw 'human_required'
          s.agentLogin = false
          emit('panel-agent-control-changed', { chatId: args.chatId, state: 'started', reason: null, provider: 'codex', chatTitle: null })
          return null
        }
        s.agentHuman = false; emit('panel-agent-control-changed', { chatId: args.chatId, state: 'available' }); return null
    }
    return undefined
  }
  // 다운로드(새 꼴): window.__panelDownload('started'|'finished'|'blocked'|'failed', name)
  window.__panelDownload = (state = 'finished', name = 'class-plan.pdf') => emit('panel-browser-download', { state, name, path: state === 'finished' ? 'D:/Downloads/' + name : null, size: state === 'finished' ? 182400 : null })
  // AI 가 패널을 쓸 때(가짜): window.__panelAgentOpen(chatId), window.__panelAgent(chatId, 'started'|'ended'|'human'|'available'|'login', reason, chatTitle)
  //   panel_agent_resume 은 ?agentlogin=notyet 이면 human_required, 아니면 login 뒤에는 started.
  // 팝업(가짜): window.__panelPopup('opened'|'closed', host, openerHost, title)
  window.__panelPopup = (state, host, openerHost, title) => emit('panel-browser-popup', { state, host, openerHost: openerHost ?? null, title: title ?? null, reason: state === 'closed' ? 'user' : null })
  window.__panelAgentOpen = (chatId) => emit('panel-agent-open-request', { chatId })
  // 로그인 코드(가짜, 2026-11-18 넷째 줄 계약): 장치 코드 · 닫힘 · 클로드 기본 브라우저 안내.
  window.__deviceLogin = (provider = 'codex', seconds = 900) => emit('provider-login-device', { provider, url: provider === 'github' ? 'https://github.com/login/device' : 'https://auth.openai.com/codex/device', code: provider === 'github' ? 'A1B2-C3D4' : 'KQ7M-2XWP', expiresInSeconds: seconds })
  window.__deviceLoginClosed = (provider = 'codex') => emit('provider-login-device-closed', { provider })
  window.__claudeLogin = (provider = 'claude') => emit('provider-login-browser', { provider, mode: 'default_browser' })
  window.__browserLoginClosed = (provider = 'claude') => emit('provider-login-browser-closed', { provider })
  window.__mediaRegenerated = (chatId, status = 'recorded') => emit('chat-media-regeneration', { chatId, userMessageId: 'm', status })
  window.__chatItemsUpdated = (chatId) => emit('chat-items-updated', { chatId })
  window.__loginStorage = (action = 'removed') => emit('provider-login-insecure-storage', { provider: 'github', action })
  window.__resumeStarted = (chatId) => emit('panel-agent-resume-started', { chatId, userMessageId: 'resume-1', text: '로그인을 마쳤습니다. 이어서 해 주세요.', provider: 'claude', skill: null, originalMessageId: null })
  // 회사가 다시 시도하는 중(가짜, 2026-11-18 계약): 지금 답을 쓰는 채팅방마다 chat-provider-retrying.
  window.__chatRetrying = (kind = 'serviceUnavailable', attempt = 1, seconds = null) => Object.keys(running).forEach((chatId) => emit('chat-provider-retrying', { chatId, provider: 'gemini', attempt, kind, retryAfterSeconds: seconds }))
  window.__panelAgent = (chatId, state, reason, chatTitle, loginUrl) => {
    window.__panelState.agentOwner = chatId
    window.__panelState.agentLogin = state === 'login'
    emit('panel-agent-control-changed', { chatId, state, reason: reason ?? null, provider: 'codex', chatTitle: chatTitle ?? null, ...(loginUrl ? { loginUrl, siteUrl: 'https://smartplace.naver.com/' } : {}) })
  }
  window.__panelGoogle = (siteUrl) => emit('panel-browser-google-login', { host: 'accounts.google.com', siteUrl: siteUrl ?? null })
  window.__closeSecret = (requestId, reason) => emit('agent-setup-secret-closed', reason ? { requestId, chatId: null, reason } : { requestId, chatId: null })

  // ---- 미디어(화면 확인용 가짜). ?media=ok(기본) / none(명령 없음). 그림은 색 띠 SVG. ----
  const mediaMode = query.get('media') || 'ok'
  function fakePicture(hue, w, h, label) {
    const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='${w}' height='${h}'><defs><linearGradient id='g' x1='0' y1='0' x2='1' y2='1'><stop offset='0' stop-color='hsl(${hue},45%,72%)'/><stop offset='1' stop-color='hsl(${(hue + 40) % 360},40%,48%)'/></linearGradient></defs><rect width='100%' height='100%' fill='url(%23g)'/><text x='50%' y='52%' font-size='${Math.round(h / 9)}' font-family='sans-serif' fill='white' text-anchor='middle'>${label}</text></svg>`
    return 'data:image/svg+xml;utf8,' + svg.replace(/#/g, '%23')
  }
  // 사진처럼 보이는 가짜 그림: 하늘(새벽·낮·저녁), 해, 겹친 산과 물을 씨앗 값으로 그린다(남의 사진을 받아 쓰지 않음).
  const photoCache = {}
  function photoLike(seed, w, h) {
    const key = `${seed}-${w}-${h}`
    if (photoCache[key]) return photoCache[key]
    let x = seed * 9301 + 49297
    const rnd = () => { x = (x * 9301 + 49297) % 233280; return x / 233280 }
    const c = document.createElement('canvas'); c.width = w; c.height = h
    const g = c.getContext('2d')
    const palettes = [
      ['#f6c7a5', '#f39b7f', '#7a6f9b', ['#5b5f86', '#3f4366', '#2b2d45']],
      ['#9fd2f2', '#d8eefb', '#f7f3e6', ['#7fa38a', '#557a65', '#36533f']],
      ['#2d3a64', '#d9736b', '#f2c48d', ['#3c3550', '#2a2539', '#191624']],
      ['#c9e6f0', '#eef6f2', '#fdf6e8', ['#a6b7a1', '#7b9277', '#55684f']],
      ['#f4d9b5', '#e9b28c', '#c98a7a', ['#8b6f6a', '#6a5250', '#493837']],
    ]
    const pal = palettes[seed % palettes.length]
    const sky = g.createLinearGradient(0, 0, 0, h)
    sky.addColorStop(0, pal[0]); sky.addColorStop(0.55, pal[1]); sky.addColorStop(1, pal[2])
    g.fillStyle = sky; g.fillRect(0, 0, w, h)
    const sx = w * (0.2 + rnd() * 0.6), sy = h * (0.25 + rnd() * 0.25), sr = Math.min(w, h) * (0.05 + rnd() * 0.05)
    const glow = g.createRadialGradient(sx, sy, 0, sx, sy, sr * 5)
    glow.addColorStop(0, 'rgba(255,250,235,0.9)'); glow.addColorStop(0.2, 'rgba(255,240,210,0.5)'); glow.addColorStop(1, 'rgba(255,240,210,0)')
    g.fillStyle = glow; g.fillRect(0, 0, w, h)
    g.fillStyle = 'rgba(255,252,242,0.95)'; g.beginPath(); g.arc(sx, sy, sr, 0, Math.PI * 2); g.fill()
    pal[3].forEach((color, layer) => {
      const base = h * (0.5 + layer * 0.14 + rnd() * 0.05)
      g.fillStyle = color; g.beginPath(); g.moveTo(0, h)
      let y = base
      for (let px = 0; px <= w; px += Math.max(4, w / 60)) {
        y += (rnd() - 0.5) * h * 0.06; y = Math.min(h * 0.95, Math.max(h * 0.32, y)); g.lineTo(px, y)
      }
      g.lineTo(w, h); g.closePath(); g.fill()
    })
    if (seed % 3 === 0) {
      const water = g.createLinearGradient(0, h * 0.82, 0, h)
      water.addColorStop(0, 'rgba(255,255,255,0.18)'); water.addColorStop(1, 'rgba(255,255,255,0.04)')
      g.fillStyle = water; g.fillRect(0, h * 0.82, w, h * 0.18)
    }
    for (let i = 0; i < 1400; i += 1) { g.fillStyle = `rgba(0,0,0,${rnd() * 0.04})`; g.fillRect(rnd() * w, rnd() * h, 1, 1) }
    return (photoCache[key] = c.toDataURL('image/jpeg', 0.82))
  }
  const RATIOS = [[3, 2], [2, 3], [1, 1], [16, 9], [4, 5], [3, 4], [3, 2], [5, 4]]
  const MEDIA_TITLES = ['아침 빈야사 흐름', '산 자세 정렬', '저녁 하타 수업 마무리', '어깨 풀기 동작', '호흡 안내', '전사 자세 옆모습', '수업 공간 창가', '나무 자세 균형', '고양이·소 자세', '사바아사나', '블록 쓰는 법', '다음 주 수업 순서']
  const BY = [
    { kind: 'ai', provider: 'codex', model: 'GPT-6.1 Terra' },
    { kind: 'ai', provider: 'claude', model: 'Fable 5.2' },
    { kind: 'user' },
    { kind: 'ai', provider: 'codex', model: 'GPT-6-Luna' },
    { kind: 'ai', provider: 'gemini', model: null },
  ]
  function mediaItem(i, chatPick) {
    const day = (d, hh) => new Date(Date.parse('2026-11-15T00:00:00+09:00') - d * 86400000 + hh * 3600000).toISOString()
    const kind = i % 9 === 4 ? 'video' : i % 11 === 7 ? 'audio' : 'image'
    const [rw, rh] = kind === 'video' ? [16, 9] : RATIOS[i % RATIOS.length]
    const w = 480, h = Math.round(480 * rh / rw)
    const by = BY[i % BY.length]
    const chat = chatPick(i)
    const pic = kind === 'audio' ? null : photoLike(i + 1, w, h)
    return {
      id: `media-${i + 1}`, kind, title: MEDIA_TITLES[i % MEDIA_TITLES.length] + (i >= MEDIA_TITLES.length ? ` ${Math.floor(i / MEDIA_TITLES.length) + 1}` : ''),
      createdAt: day(Math.floor(i / 4), 21 - (i % 4) * 3), width: kind === 'audio' ? null : w, height: kind === 'audio' ? null : h,
      durationSec: kind === 'image' ? null : 20 + (i * 13) % 200, path: kind === 'image' ? pic : '', posterPath: kind === 'video' ? pic : null,
      chatId: chat ? chat.id : null, chats: chat ? [{ id: chat.id, title: chat.title }] : [], by, how: by.kind === 'user' ? 'attached' : 'created',
    }
  }
  function mediaList() {
    if (mediaMode === 'none') throw 'Command media_list not found'
    const list = chatList()
    const count = bigMode ? 300 : mediaMode === 'many' ? 120 : mediaMode === 'empty' ? 0 : 14
    return Array.from({ length: count }, (_, i) => mediaItem(i, (n) => list[n % Math.max(1, list.length)] || null)).filter((item) => !trashedSources.has(item.id))
  }
  // ---- 원자료 열기와 지우기(가짜, T59). 지운 원자료는 미디어 목록과 채팅방에서 다룬 것에서 빠진다. ----
  const trashedSources = new Set()
  const purgedSources = new Set()
  const retrySources = new Map()
  window.__sourceCalls = []
  if (query.get('copyfail')) setTimeout(() => emit('chat-archive-changed', { chatId: null, state: 'unavailable', reason: query.get('copyfail') }), 2500)
  function sourceCommand(command, args) {
    window.__sourceCalls.push([command, args])
    const id = args.assetId
    if (trashedSources.has(id) && command !== 'source_asset_restore_trashed') {
      if (command === 'source_asset_open') return { state: 'not_found', kind: null, path: null, content: null, nextCursor: null, totalBytes: null }
    }
    const files = chatItems(chatList()[0]?.id).files
    const fileTitle = Object.fromEntries(files.map((f) => [f.id, f.title]))[id]
    switch (command) {
      case 'source_asset_open':
        if (id === asset) return { state: 'ok', kind: 'audio', path: 'C:/fake/a.m4a', content: null, nextCursor: null, totalBytes: null }
        if (fileTitle) return /[.](txt|md)$/.test(fileTitle) ? { state: 'ok', kind: 'text', path: null, content: '...', nextCursor: null, totalBytes: 3 } : { state: 'not_text', kind: null, path: null, content: null, nextCursor: null, totalBytes: null }
        return { state: 'ok', kind: 'image', path: 'C:/fake/p.jpg', content: null, nextCursor: null, totalBytes: null }
      case 'source_asset_delete_preview': {
        // ?sourceused=none 이면 쓰는 곳 없음. 기본은 문서 둘·작업 내역 하나·채팅방 하나.
        if (query.get('sourcefail') === 'preview') throw 'references_unavailable'
        const none = query.get('sourceused') === 'none'
        const docIds = Object.keys(zets).slice(0, 2)
        return { assetId: id, sha256: 'x', originalFileName: fileTitle || null, state: 'active', documents: none ? [] : docIds, receipts: none ? [] : ['rcpt_1'], chats: none ? [] : [chatList()[0]?.id].filter(Boolean), recordVersions: [], localRemaining: null, remoteRemaining: null, planDigest: 'fake-digest' }
      }
      case 'source_asset_trash':
        if (query.get('sourcefail') === 'busy') throw 'chat_active'
        trashedSources.add(id); return { state: 'trashed', assetId: id, retainUntil: new Date(Date.now() + 30 * 864e5).toISOString() }
      case 'source_asset_restore_trashed':
        trashedSources.delete(id); return { state: 'active', assetId: id, retainUntil: null }
      case 'source_asset_deleted_list': {
        // ?deleted=many 면 미리 지운 것 45개(쪽 나눔 확인), 기본은 이 창에서 지운 것만.
        if (query.get('deleted') === 'many' && !window.__deletedSeeded) { window.__deletedSeeded = true; for (let i = 0; i < 45; i++) trashedSources.add('sha256:seed' + String(i).padStart(4, '0')) }
        // ?deleted=retry 면 다 지우지 못한 줄 둘(이 PC 의 것·원격의 사본)과 되돌릴 수 있는 줄 하나.
        if (query.get('deleted') === 'retry' && !window.__deletedSeeded) { window.__deletedSeeded = true; trashedSources.add('sha256:seed0000'); for (const [n, s] of [['0001', 'local_retry_pending'], ['0002', 'remote_retry_pending']]) { trashedSources.add('sha256:seed' + n); retrySources.set('sha256:seed' + n, s) } }
        const all = [...trashedSources].map((id, i) => ({ id, originalFileName: id.startsWith('sha256:seed') ? `수업 녹음 ${id.slice(-4)}.m4a` : (Object.fromEntries(files.map((f) => [f.id, f.title]))[id] || '수업 사진.jpg'), deletedAt: new Date(Date.now() - i * 3600e3).toISOString(), deletionState: retrySources.get(id) || 'trashed', canRestore: !retrySources.has(id), canPurge: true, referenceCount: i % 3 }))
        const start = args.before ? all.findIndex((x) => x.id === args.before.assetId) + 1 : 0
        const items = all.slice(start, start + 40)
        const more = start + items.length < all.length
        return { items, hasMore: more, nextCursor: more ? { deletedAt: items[items.length - 1].deletedAt, assetId: items[items.length - 1].id } : null }
      }
      case 'source_asset_purge':
        if (!args.confirmed) throw 'confirmation_required'
        // ?purgeretry=local|remote 면 다 지우지 못해 목록에 남는다(뒤쪽 local_retry_pending·remote_retry_pending).
        if (query.get('purgeretry')) { const s = query.get('purgeretry') + '_retry_pending'; retrySources.set(id, s); return { state: s, assetId: id, localRemaining: [], remoteRemaining: ['remote_access:network'], removedRemoteKeys: [] } }
        trashedSources.delete(id); retrySources.delete(id); purgedSources.add(id)
        return { state: 'partial', assetId: id, localRemaining: ['database_backups_may_contain_metadata'], remoteRemaining: ['object_versions_unverified', 'other_stores_unverified'], removedRemoteKeys: [] }
    }
    return undefined
  }
  // ?items=ok(기본) / none(명령 없음) · 첫 채팅방은 문서 셋과 미디어 여섯, 둘째는 문서 하나, 셋째는 아무것도 없음.
  const extraItems = {}
  function chatItems(chatId) {
    if ((query.get('items') || 'ok') === 'none') throw 'Command chat_items not found'
    const list = chatList()
    const at = list.findIndex((c) => c.id === chatId)
    const docs = listEntries()
    const when = (m) => new Date(Date.parse('2026-11-15T21:00:00+09:00') - m * 60000).toISOString()
    const pick = (entry, how, by, m) => ({ id: entry.id, title: entry.title, summary: entry.summary, kind: entry.kind, status: entry.status, updatedAt: entry.updatedAt, how, by, at: when(m) })
    let documents = []
    let media = []
    let files = []
    const file = (title, m) => ({ id: `sha256:${title.length.toString(16).padStart(4, '0')}`, title, createdAt: when(m), how: 'attached', by: { kind: 'user' }, at: when(m), path: '' })
    if (at === 0) {
      documents = [pick(docs[0], 'created', BY[1], 5), pick(docs[1] || docs[0], 'edited', BY[0], 40), pick(docs[2] || docs[0], 'read', BY[0], 90)]
      media = Array.from({ length: 6 }, (_, i) => ({ ...mediaItem(i * 3, () => list[0]), at: when(10 + i * 7) }))
      files = [file('저녁 하타 전사.txt', 15), file('수련생 메모.md', 60), file('수업 사진 원본.heic', 70)]
    } else if (at === 1) {
      documents = [pick(docs[0], 'edited', { kind: 'user' }, 120)]
    }
    const extra = extraItems[chatId] || []
    const kept = (item) => !(typeof trashedSources !== 'undefined' && trashedSources.has(item.id))
    return { documents: [...extra, ...documents], media: [...(attachedMedia[chatId] || []), ...media].filter(kept), files: [...(attachedFiles[chatId] || []), ...files].filter(kept) }
  }
  // ---- 클립으로 붙이기(가짜): plan/apply_source_asset_import → chat_attach_asset. 소리 파일은 미디어로 "붙임"(실제처럼). ----
  const attachedMedia = {}
  const attachedFiles = {}
  const importedAssets = {}
  window.__attachCalls = []
  function attachCommand(command, args) {
    window.__attachCalls.push(command)
    if (command === 'plan_source_asset_import') {
      const p = args.proposal
      const name = p.sourcePath.split(/[\\/]/).pop()
      return { planDigest: 'p'.repeat(64), originalFileName: name, assetKind: p.assetKind, contentSha256: (name.length.toString(16) + 'a'.repeat(64)).slice(0, 64), byteLength: 1234567, alreadyStored: false, mediaType: p.mediaType }
    }
    if (command === 'apply_source_asset_import') {
      const p = args.input.proposal
      const id = `asset-${Object.keys(importedAssets).length + 1}`
      importedAssets[id] = { name: p.sourcePath.split(/[\\/]/).pop(), kind: p.assetKind }
      return { id, assetKind: p.assetKind, contentSha256: 'a'.repeat(64), byteLength: 1234567, originalFileName: importedAssets[id].name, mediaType: p.mediaType, capturedBy: 'momso-desktop', createdAt: nextStamp(), integrityState: 'active', integrityIssue: null, quarantinedAt: null, quarantinedBy: null }
    }
    if (command === 'chat_attach_asset') {
      if (!chats[args.chatId]) throw '채팅방을 찾을 수 없습니다'
      const asset = importedAssets[args.assetId]
      if (!asset) throw '원자료를 찾을 수 없습니다'
      if (asset.kind === 'audio_recording') {
        const now = new Date().toISOString()
        attachedMedia[args.chatId] = [{ id: `m-${args.assetId}`, kind: 'audio', title: asset.name, createdAt: now, width: null, height: null, durationSec: 95, path: '', posterPath: null, chatId: args.chatId, chats: [{ id: args.chatId, title: chats[args.chatId].title }], by: { kind: 'user' }, how: 'attached', at: now }, ...(attachedMedia[args.chatId] || [])]
      } else {
        // T42: 미디어가 아닌 첨부(글 파일 등)는 files 로.
        const now = new Date().toISOString()
        attachedFiles[args.chatId] = [{ id: `sha256:${args.assetId}`, title: asset.name, createdAt: now, how: 'attached', by: { kind: 'user' }, at: now, path: '' }, ...(attachedFiles[args.chatId] || [])]
      }
      window.setTimeout(() => emit('chat-items-updated', { chatId: args.chatId }), 0)
      return 'rec_20261116_100000'
    }
    return undefined
  }
  window.__addChatItem = (chatId) => {
    const docs = listEntries()
    extraItems[chatId] = [{ id: docs[0].id, title: docs[0].title, summary: docs[0].summary, kind: docs[0].kind, status: docs[0].status, updatedAt: docs[0].updatedAt, how: 'created', by: BY[0], at: new Date().toISOString() }, ...(extraItems[chatId] || [])]
    emit('chat-items-updated', { chatId })
  }

  // ---- 스킬(가짜). ?skills=ok(기본) / empty(하나도 없음) / none(명령 없음). 이름은 "보기 스킬 …"(요가 스킬을 지어 넣지 않음). ----
  const skillStore = (query.get('skills') || 'ok') === 'empty' ? [] : [
    { name: 'skill-one', title: '보기 스킬 하나', description: '보기 설명: 처음부터 들어 있던 스킬.', enabled: true, builtin: true, body: '보기 내용 하나.', syncState: 'ok' },
    { name: 'skill-two', title: '보기 스킬 둘', description: '보기 설명: 내가 만든 스킬.', enabled: true, builtin: false, body: '보기 내용 둘.' + String.fromCharCode(10) + '둘째 줄.', syncState: 'ok' },
    { name: 'skill-three', title: '보기 스킬 셋', description: '보기 설명: 꺼 둔 스킬.', enabled: false, builtin: false, body: '보기 내용 셋.', syncState: 'disabled' },
    { name: 'skill-four', title: '보기 스킬 넷', description: '보기 설명: 다른 곳에서 고친 내용과 다른 스킬.', enabled: true, builtin: true, body: '보기 내용 넷.', syncState: 'conflict' },
    // 쓸 수 있는지(available·unavailableReason): 브라우저 스킬 둘
    { name: 'skill-five', title: '보기 스킬 다섯', description: '보기 설명: 브라우저 스킬, 기본 규칙을 고침.', enabled: true, builtin: true, body: '보기 내용 다섯.', syncState: 'ok', available: false, unavailableReason: 'policy_modified' },
    { name: 'skill-six', title: '보기 스킬 여섯', description: '보기 설명: 브라우저 스킬, 브라우저 없음.', enabled: true, builtin: true, body: '보기 내용 여섯.', syncState: 'ok', available: false, unavailableReason: 'browser_unavailable' },
  ].map((item) => ({
    available: item.syncState === 'conflict' || !item.enabled ? false : true,
    unavailableReason: item.syncState === 'conflict' ? 'skill_conflict' : !item.enabled ? 'disabled' : null,
    ...item, updatedAt: '2026-11-16T01:00:00+09:00', providers: ['codex', 'claude'], hash: 'h1',
  }))
  window.__skillCalls = []
  function skillCommand(command, args) {
    if ((query.get('skills') || 'ok') === 'none') throw `Command ${command} not found`
    const find = (name) => { const found = skillStore.find((item) => item.name === name); if (!found) throw 'not_found'; return found }
    const view = (item, withBody) => { const { hash, body, ...rest } = item; return withBody ? { ...rest, body, contentHash: hash } : rest }
    window.__skillCalls.push([command, args])
    switch (command) {
      case 'skill_list': return skillStore.map((item) => view(item, false))
      case 'skill_read': return view(find(args.name), true)
      case 'skill_create': {
        if (!/^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$/.test(args.name || '')) throw 'invalid_name'
        if (skillStore.some((item) => item.name === args.name)) throw 'already_exists'
        const item = { name: args.name, title: args.title, description: args.description, body: args.body, enabled: true, builtin: false, syncState: 'ok', updatedAt: new Date().toISOString(), providers: ['codex', 'claude'], hash: 'h1' }
        skillStore.push(item)
        return { status: 'ok' }
      }
      case 'skill_update': {
        const item = find(args.name)
        if (window.__skillConflict || args.expectedHash !== item.hash) { window.__skillConflict = false; item.hash = item.hash + 'x'; throw 'conflict' }
        Object.assign(item, args.changes || {}, { hash: item.hash + 'u', updatedAt: new Date().toISOString() })
        return { status: 'ok' }
      }
      case 'skill_set_enabled': { const item = find(args.name); item.enabled = Boolean(args.enabled); item.syncState = item.enabled ? 'ok' : 'disabled'; return { status: 'ok' } }
      case 'skill_remove': {
        const item = find(args.name)
        if (item.builtin) throw 'builtin'
        skillStore.splice(skillStore.indexOf(item), 1)
        return { status: 'ok' }
      }
    }
    return undefined
  }

  // ---- 첫 실행(가짜). ?firstrun=1 이면 첫 실행을 마치지 않은 컴퓨터로 본다(없으면 마친 것으로 적어 두어 다른 사진에 끼지 않는다).
  //      ?username=none 이면 이름이 없다, 그 밖의 값이면 그 이름. ?username 이 없으면 'teacher'. ?usernamecmd=none 이면 명령 없음. ----
  if (query.get('firstrun') === '1') { try { localStorage.removeItem('momso.firstRunDone') } catch {} }
  else { try { localStorage.setItem('momso.firstRunDone', '1') } catch {} }
  let fakeUsername = query.get('username') === 'none' ? null : (query.get('username') || 'teacher')
  window.__usernameCalls = []
  function usernameCommand(command, args) {
    if (query.get('usernamecmd') === 'none') throw `Command ${command} not found`
    window.__usernameCalls.push([command, args])
    if (command === 'momso_username_get') return { state: 'ok', username: fakeUsername }
    const name = (args && args.username) || ''
    const lower = name.toLowerCase()
    if (!/^[A-Za-z0-9._-]+$/.test(name) || name.length > 94 || lower.endsWith('.git') || lower.endsWith('.wiki')) return { state: 'invalid_username', username: null }
    fakeUsername = name
    return { state: 'ok', username: name }
  }

  // ---- AI 로그인(가짜). ?auth=signedout / account / keys / mixed, 없으면 명령 없음(예전 연결 칸). 계약대로 제미나이는 unavailable, methods 빈 배열.
  //      ?installed=none 이면 셋 다 도구가 없다, ?installed=codex,claude 처럼 적으면 그것만 있다(없으면 코덱스·클로드만 있다).
  //      설치는 ?installdelay=ms 뒤에 끝난다. ?installfail=1 이면 실패한다. ----
  const authMode = query.get('auth')
  const installedParam = query.get('installed')
  const installedSet = installedParam === null ? new Set(['codex', 'claude']) : new Set(installedParam === 'none' ? [] : installedParam.split(','))
  const authState = {}
  ;['codex', 'claude', 'gemini'].forEach((provider) => {
    const gem = provider === 'gemini'
    const installed = installedSet.has(provider)
    // T43: 제미나이는 methods ['apiKey'](계정 로그인 없음). 실제 앱처럼 키가 없으면 unavailable, ?auth=keys 면 apiKey.
    const state = !installed ? 'unavailable' : gem ? (authMode === 'keys' ? 'apiKey' : 'unavailable') : authMode === 'account' ? 'account' : authMode === 'keys' ? 'apiKey' : authMode === 'mixed' ? (provider === 'codex' ? 'account' : 'apiKey') : 'signedOut'
    authState[provider] = {
      provider, installed, state, methods: gem ? ['apiKey'] : ['account', 'apiKey'],
      accountLabel: state === 'account' ? 'teacher@example.com' : null, planLabel: state === 'account' ? (provider === 'codex' ? 'Plus' : 'Pro') : null,
      apiKeySaved: state === 'apiKey', active: state === 'signedOut' ? null : state,
      // 2026-11-17 키 확인(desktop-screen-backend-contract.md "AI 연결 확인"). ?key=verifying|rejected|unavailable|notSupported 로 저장된 키의 확인 결과를 바꾼다(기본 verified).
      runnable: installed, keyVerification: state === 'apiKey' ? (query.get('key') || 'verified') : 'notConfigured',
    }
  })
  // 뒤쪽 first_run_allowed 와 같은 규칙(provider_auth.rs): 도구가 실행되고, 계정이 있거나 저장된 키가 verified·unavailable·notSupported.
  function firstRunAllowed() {
    return Object.values(authState).some((x) => x.runnable && (x.state === 'account' || (x.apiKeySaved && ['verified', 'unavailable', 'notSupported'].includes(x.keyVerification))))
  }
  // 키를 저장하거나 다시 확인하면 verifying 을 알리고, 잠시 뒤 결과(?keyresult=rejected|unavailable|verified, 기본 verified)를 알린다.
  function checkKey(item, provider) {
    item.keyVerification = 'verifying'
    emit('provider-auth-changed', { provider, state: 'verifying' })
    setTimeout(() => { item.keyVerification = query.get('keyresult') || 'verified'; emit('provider-auth-changed', { provider, state: item.keyVerification }) }, Number(query.get('keydelay') || 800))
  }
  window.__authCalls = []
  function authCommand(command, args) {
    if (!authMode) throw `Command ${command} not found`
    window.__authCalls.push([command, command === 'provider_api_key_set' ? { provider: args.provider, apiKey: '(가림)' } : args])
    const item = args && authState[args.provider]
    switch (command) {
      case 'provider_auth_status': return Object.values(authState).map((x) => ({ ...x }))
      case 'install_external_provider': {
        // 계약(287~289줄): 끝날 때까지 기다린 뒤 installed / installFailed / unavailable / inProgress. 진행 이벤트 provider-install-progress.
        // ?installfail=1 이면 실패(?installreason=timeout 등, 없으면 installFailed). ?installbytes=1 이면 받은 양을 숫자로 준다.
        if (!item) return 'unavailable'
        const progress = (phase, extra) => emit('provider-install-progress', { provider: args.provider, phase, receivedBytes: null, totalBytes: null, canCancel: false, reason: null, ...(extra || {}) })
        progress('installing')
        const total = 40 * 1024 * 1024
        const delay = Number(query.get('installdelay') || 1200)
        if (query.get('installbytes') === '1') [0.25, 0.5, 0.75].forEach((part, i) => setTimeout(() => progress('installing', { receivedBytes: total * part, totalBytes: total }), (delay / 4) * (i + 1)))
        // 뒤쪽처럼 끝나기 전에 설치한 도구를 실행해 보는 단계(verifying, 실패 아님). ?installfail=once 면 처음 한 번만 실패(반쯤 깔린 뒤 다시 시도).
        setTimeout(() => progress('verifying'), delay * 0.8)
        window.__installTries = (window.__installTries || 0) + 1
        return new Promise((done) => setTimeout(() => {
          if (query.get('installfail') === '1' || (query.get('installfail') === 'once' && window.__installTries === 1)) {
            progress('failed', { reason: query.get('installreason') || 'installFailed' })
            done('installFailed'); return
          }
          Object.assign(item, { installed: true, state: 'signedOut' })
          progress('completed')
          emit('provider-auth-changed', { provider: args.provider, state: 'signedOut' })
          done('installed')
        }, delay))
      }
      case 'provider_login_start':
        if (!item || !item.methods.includes('account')) return 'unavailable'
        emit('provider-auth-changed', { provider: args.provider, state: 'inProgress' })
        // ?loginresult=unavailable 면 로그인이 끝나지 않은 것처럼(5분 지남·실패는 뒤쪽이 unavailable 로 알린다).
        // ?loginreason=timeout 이면 reason 도 붙인다(시간 초과).
        if (query.get('loginresult')) { window.__authTimer = setTimeout(() => emit('provider-auth-changed', { provider: args.provider, state: query.get('loginresult'), ...(query.get('loginreason') ? { reason: query.get('loginreason') } : {}) }), Number(query.get('logindelay') || 1500)) } else
        window.__authTimer = setTimeout(() => { Object.assign(item, { state: 'account', accountLabel: 'teacher@example.com', planLabel: 'Plus', active: 'account' }); emit('provider-auth-changed', { provider: args.provider, state: 'account' }) }, Number(query.get('logindelay') || 1500))
        return 'inProgress'
      case 'provider_login_cancel': clearTimeout(window.__authTimer); emit('provider-auth-changed', { provider: args.provider, state: 'cancelled' }); return 'cancelled'
      case 'provider_logout': Object.assign(item, { state: item.apiKeySaved ? 'apiKey' : 'signedOut', accountLabel: null, planLabel: null }); emit('provider-auth-changed', { provider: args.provider, state: 'signedOut' }); return 'signedOut'
      case 'provider_api_key_set':
        if (!item.methods.includes('apiKey')) return 'unavailable'
        if (!args.apiKey || /\s/.test(args.apiKey) || args.apiKey.length < 8) return 'invalidKey'
        item.apiKeySaved = true; if (item.state === 'signedOut' || item.state === 'unavailable') item.state = 'apiKey'
        checkKey(item, args.provider); return 'verifying'
      case 'provider_api_key_verify':
        if (!item || !item.apiKeySaved) return 'notConfigured'
        checkKey(item, args.provider); return 'verifying'
      case 'provider_first_run_eligible': return firstRunAllowed()
      case 'provider_api_key_clear':
        item.apiKeySaved = false; item.keyVerification = 'notConfigured'; if (item.state === 'apiKey') item.state = 'signedOut'
        emit('provider-auth-changed', { provider: args.provider, state: 'keyCleared' }); return 'keyCleared'
    }
    return undefined
  }

  // ---- 백업(가짜). ?remote=off(기본)/on/none · ?store=off(기본)/on/none · 복구 키는 ?key=confirmed 면 확인됨. ----
  const remoteMode = query.get('remote') || 'off'
  const storeMode = query.get('store') || 'off'
  const remote = { state: remoteMode === 'on' ? 'connected' : 'not_connected', reason: null, repo: remoteMode === 'on' ? 'teacher/momso-notes' : null, visibility: remoteMode === 'on' ? 'private' : null, lastPushedAt: remoteMode === 'on' ? '2026-11-15T21:00:00Z' : null, ahead: remoteMode === 'on' ? 2 : null, behind: remoteMode === 'on' ? 0 : null, signedInAs: 'teacher' }
  const store = { state: storeMode === 'on' ? 'connected' : 'not_connected', provider: storeMode === 'on' ? 'cloudflare' : null, bucket: storeMode === 'on' ? 'momso-raw' : null, lastBackupAt: storeMode === 'on' ? '2026-11-15T22:00:00Z' : null, pendingCount: storeMode === 'on' ? 3 : null, pendingBytes: storeMode === 'on' ? 18874368 : null }
  const recovery = { stored: query.get('key') === 'confirmed', confirmed: query.get('key') === 'confirmed' }
  window.__backupCalls = []
  function backupCommand(command, args) {
    window.__backupCalls.push([command, command === 'object_store_connect' ? { ...args, accessKeyId: '(가림)', secretAccessKey: '(가림)' } : args])
    // 자동 백업(가짜): ?autobackup=retry 면 연속 실패 2번 · 4분 뒤, pending·running 도. window.__auto 로 바꿀 수 있다. ?autoreason=<값> 이면 retry 의 까닭(두 자동 줄 함께).
      // 원자료 자동 업로드(가짜, 2026-11-18 D38): ?storeauto=retry|pending|running|off. window.__storeAuto 로 바꿀 수 있다.
      // 따로 넣은 파일 수(M17): ?extrafiles=N(기본 0), =fail 이면 unavailable.
      if (command === 'archive_additional_files_count') {
        if (query.get('extrafiles') === 'fail') throw 'unavailable'
        return { count: Number(query.get('extrafiles') || 0) }
      }
      if (command === 'object_store_auto_upload_get' || command === 'object_store_auto_upload_set') {
        if (!window.__storeAuto) {
          const mode = query.get('storeauto') || 'idle'
          window.__storeAuto = { enabled: mode !== 'off', state: mode === 'off' ? 'disabled' : mode === 'retry' ? 'retry_pending' : mode, consecutiveFailures: mode === 'retry' ? 3 : 0, reason: mode === 'retry' ? (query.get('autoreason') || 'remote_unavailable') : null, retryInSeconds: mode === 'retry' ? 480 : null }
        }
        if (command === 'object_store_auto_upload_set') Object.assign(window.__storeAuto, { enabled: args.enabled, state: args.enabled ? 'pending' : 'disabled' })
        return { ...window.__storeAuto }
      }
      if (command === 'archive_auto_backup_get' || command === 'archive_auto_backup_set') {
        if (!window.__auto) {
          const mode = query.get('autobackup') || 'idle'
          window.__auto = { enabled: mode !== 'off', state: mode === 'off' ? 'disabled' : mode === 'retry' ? 'retry_pending' : mode, consecutiveFailures: mode === 'retry' ? 2 : 0, reason: mode === 'retry' ? (query.get('autoreason') || 'connection_failed') : null, retryInSeconds: mode === 'retry' ? 240 : null }
        }
        if (command === 'archive_auto_backup_set') Object.assign(window.__auto, { enabled: args.enabled, state: args.enabled ? 'pending' : 'disabled' })
        return { ...window.__auto }
      }
    if (command.startsWith('archive_remote_')) {
      if (remoteMode === 'none') throw `Command ${command} not found`
      if (command === 'archive_remote_connect') {
        if (!/^[\w.-]+\/[\w.-]+$/.test(args.repo || '')) return { ...remote, reason: 'invalid_repository' }
        if (/public/.test(args.repo)) return { ...remote, reason: 'repository_not_private' }
        Object.assign(remote, { state: 'connected', repo: args.repo, visibility: 'private', ahead: 0, behind: 0 }); return { ...remote }
      }
      // ?git=missing: 이 PC 에 Git 이 없어 GitHub 쪽이 막힘(reason git_missing). 설치가 끝나면(gitInstalled) 풀린다.
      if (command === 'archive_remote_status' && query.get('git') === 'missing' && !window.__gitInstalled) return { ...remote, state: 'unavailable', reason: 'git_missing' }
      // 다시 읽은 상태에도 conflictPaths(2026-11-17). ?statusdiverged=3 이면 그 수만큼, =null 이면 수 없이.
      // ?remotereason=signed_out 등: 연결 전 상태가 unavailable + 그 까닭(2026-11-18). 다시 시도하면 window.__remoteFixed 가 true 일 때 연결 전으로.
      if (command === 'archive_remote_status' && query.get('remotereason') && !window.__remoteFixed) return { ...remote, state: 'unavailable', reason: query.get('remotereason'), repo: null, signedInAs: null }
      if (command === 'archive_remote_status' && query.get('statusdiverged')) return { ...remote, reason: 'diverged', conflictPaths: query.get('statusdiverged') === 'null' ? null : Array.from({ length: Number(query.get('statusdiverged')) }, (_, i) => `zettels/${i}.md`) }
      // ?history=profile / rules / other: 올리기가 이력 때문에 막힘(T34·T35). reason 은 unsafe_history, historyReason 은 그 까닭(그 밖은 null).
      const held = { profile: 'profile_in_history', rules: 'profile_in_rules_history', other: null }
      if (command === 'archive_remote_push' && query.get('history') in held) return { ...remote, reason: 'unsafe_history', historyReason: held[query.get('history')] }
      // 2026-11-17 합치기 결과와 못 올린 까닭. ?merge=auto|preserved|preserved2|conflicts|conflictsnull, ?pushreason=connection_failed 등.
      // ?firstreceive=1 이면 받기 동안 archive-first-receive-progress(세 단계)와 chat-archive-restore-progress 를 흉내 낸다.
      const merged = () => {
        const m = query.get('merge')
        const ids = Object.keys(zets)
        if (m === 'auto') return { autoMerged: true, preservedDocuments: [], conflictPaths: [] }
        if (m === 'preserved') return { autoMerged: true, preservedDocuments: [ids[0]], conflictPaths: [] }
        if (m === 'preserved2') return { autoMerged: true, preservedDocuments: ids.slice(0, 2), conflictPaths: [] }
        if (m === 'conflicts') return { autoMerged: false, preservedDocuments: [], conflictPaths: ['zettels/a.md', 'zettels/b.md', 'chats/c.json'], reason: 'diverged' }
        if (m === 'conflictsnull') return { autoMerged: false, preservedDocuments: [], conflictPaths: null, reason: 'diverged' }
        return { autoMerged: false, preservedDocuments: [], conflictPaths: [] }
      }
      if (command === 'archive_remote_push') {
        // ?pushredaction=1 이면 가리는 동안이라 미룸, ?keyleft=1 이면 올린 기록에 옛 키가 남음(2026-11-17).
        if (query.get('pushredaction') === '1') return { ...remote, state: 'redactionPending', reason: null }
        if (query.get('keyleft') === '1') { Object.assign(remote, { ahead: 0, lastPushedAt: new Date().toISOString() }); return { ...remote, uploadedKeyFound: true, expiredKeyWarning: false } }
        if (query.get('pushreason')) return { ...remote, reason: query.get('pushreason'), ...merged() }
        Object.assign(remote, { ahead: 0, lastPushedAt: new Date().toISOString() }); return { ...remote, ...merged() }
      }
      if (command === 'archive_remote_pull') {
        const done = () => ({ ...remote, reason: query.get('pull') === 'unsaved' ? 'unsaved_changes' : null, ...merged() })
        if (query.get('firstreceive') !== '1') return done()
        const steps = [['receiving', 0], ['indexing', 1], ['restoring', 2]]
        const gap = Number(query.get('receivegap') || 1500)
        steps.forEach(([state, completed], i) => setTimeout(() => emit('archive-first-receive-progress', { completed, total: 3, state, available: false }), gap * i))
        ;[0, 1, 2, 3, 4].forEach((n) => setTimeout(() => emit('chat-archive-restore-progress', { completed: n * 3, total: 12, state: 'restoring', available: false }), gap * 2 + n * (gap / 5)))
        return new Promise((resolve) => setTimeout(() => {
          emit('chat-archive-restore-progress', { completed: 12, total: 12, state: 'complete', available: true })
          emit('archive-first-receive-progress', { completed: 3, total: 3, state: 'complete', available: true })
          resolve(done())
        }, gap * 3 + 200))
      }
      if (command === 'archive_remote_disconnect') { Object.assign(remote, { state: 'not_connected', repo: null, ahead: null, behind: null }); return { ...remote } }
      // T37 저장소 만들기(가짜). ?create=created(기본) / exists_empty / exists_public / signed_out 등.
      if (command === 'archive_remote_create') {
        const result = query.get('create') || 'created'
        const repo = fakeUsername ? `teacher/momso-${fakeUsername}` : null
        if (!fakeUsername) return { state: 'username_missing', repo: null }
        if (result === 'created') Object.assign(remote, { state: 'connected', repo, visibility: 'private', ahead: 0, behind: 0 })
        return { state: result, repo: result.startsWith('exists') || result.startsWith('created') ? repo : null }
      }
      return { ...remote }
    }
    if (command.startsWith('object_store_') || command.startsWith('app_state_')) {
      if (storeMode === 'none') throw `Command ${command} not found`
      if (command === 'object_store_status') return { ...store }
      if (command === 'object_store_connect') {
        if (args.provider === 'ncp') throw 'write_safety_unverified'
        if (args.secretAccessKey === 'wrong') throw 'invalid_credentials'
        // ?storefail=bucket_create_unknown|created_not_connected 처럼 연결 실패 값을 흉내 낸다(2026-11-17).
        if (query.get('storefail')) throw query.get('storefail')
        // 버킷을 비우면 momso-<username> 을 만들어 연결(자동 버킷 계약). ?bucketauto=taken 이면 bucket_name_taken.
        let bucket = args.bucket
        if (!bucket || !bucket.trim() || bucket === 'auto') {
          if (args.provider === 'gcp') throw 'gcp_project_required'
          if (!fakeUsername) throw 'username_required'
          if (query.get('bucketauto') === 'taken') throw 'bucket_name_taken'
          bucket = `momso-${fakeUsername}`
        }
        Object.assign(store, { state: 'connected', provider: args.provider, bucket, pendingCount: 12, pendingBytes: 52428800 }); return { ...store }
      }
      if (command === 'object_store_disconnect') { Object.assign(store, { state: 'not_connected', provider: null, bucket: null }); return null }
      if (command === 'object_store_backup') {
        if (!recovery.confirmed) throw 'recovery_key_unconfirmed'
        const total = store.pendingBytes || 1
        return new Promise((done) => { let n = 0; const t = setInterval(() => { n += 1; emit('object-store-progress', { completedCount: n, completedBytes: total * n / 4, totalCount: 4, totalBytes: total }); if (n >= 4) { clearInterval(t); emit('object-store-done', { completedCount: 4, completedBytes: total, totalCount: 4, totalBytes: total }); Object.assign(store, { pendingCount: 0, pendingBytes: 0, lastBackupAt: new Date().toISOString() }); done({ completedCount: 4, completedBytes: total, totalCount: 4, totalBytes: total }) } }, 300) })
      }
      if (command === 'app_state_backup') return { state: 'complete', createdAt: new Date().toISOString(), size: 2048 }
      // ?restorefail=restore_failed|restore_rollback_failed 면 그 값으로 실패한다(뒤쪽은 오류 글로 준다).
      if (command === 'app_state_restore' && query.get('restorefail')) throw query.get('restorefail')
      if (command === 'app_state_restore') return { state: 'restored', snapshotAt: store.lastBackupAt, safetyBackupCreated: true }
      if (command === 'object_store_restore') return { restoredCount: 2, skippedCount: 10 }
    }
    if (command === 'user_owned_backup_overview') return { recoveryKeyStored: recovery.stored, recoveryKeyConfirmed: recovery.confirmed }
    if (command === 'export_backup_recovery_key') { recovery.stored = true; return 'AGE-SECRET-KEY-1FAKEFAKEFAKE' }
    if (command === 'confirm_backup_recovery_key') { recovery.confirmed = true; return null }
    if (command === 'import_backup_recovery_key') { recovery.stored = true; recovery.confirmed = true; return null }
    return undefined
  }

  function archive(command, args) {
    if (scenario === 'missing' && /^(archive_|zet_|receipt_)/.test(command)) throw `Command ${command} not found`
    switch (command) {
      case 'usage_status':
        return usageStatus()
      case 'setup_secret_submit':
        // 계약: setup_secret_submit({ requestId, chatId, input: { accessKeyId, secretAccessKey, gcpOauthToken? } }). 값은 남기지 않는다(칸 이름과 길이만).
        window.__secretCalls.push(['submit', args.requestId, args.chatId, Object.fromEntries(Object.entries(args.input || {}).map(([k, v]) => [k, String(v).length]))])
        if (query.get('secretfail') === '1') throw 'unavailable'
        emit('agent-setup-secret-closed', { requestId: args.requestId, chatId: args.chatId, reason: 'submitted' })
        return null
      case 'agent_setup_secret_cancel':
        // 계약(T48): agent_setup_secret_cancel({ requestId, chatId }) → 값 없음, 그 뒤 agent-setup-secret-closed.
        window.__secretCalls.push(['cancel', args.requestId, args.chatId])
        emit('agent-setup-secret-closed', { requestId: args.requestId, chatId: args.chatId })
        return null
      case 'agent_approval_respond': {
        window.__approvalCalls = (window.__approvalCalls || []).concat([{ requestId: args.requestId, choice: args.choice }])
        const pending = approvalsPending[args.requestId]
        if (!pending) throw '승인 요청을 찾을 수 없습니다'
        if (!pending.options.includes(args.choice)) throw '이 승인 요청에서 선택할 수 없는 응답입니다'
        delete approvalsPending[args.requestId]
        // 2026-11-17 뒤쪽: 답해서 끝나도 닫힘이 온다(reason responded). 답이 돌아오기 전에 오는 가장 나쁜 차례로.
        emit('agent-approval-closed', { requestId: args.requestId, reason: 'responded' })
        return null
      }
      case 'chat_group_list':
        return groupList()
      case 'chat_group_create': {
        if (groupsMode === 'none') throw 'Command chat_group_create not found'
        const g = addGroup(args.name)
        return { ...g, chatCount: 0 }
      }
      case 'chat_group_rename': {
        const g = chatGroups.find((x) => x.id === args.id); if (!g) throw '묶음을 찾을 수 없습니다'
        g.name = args.name; return null
      }
      case 'chat_group_reorder':
        args.ids.forEach((id, i) => { const g = chatGroups.find((x) => x.id === id); if (g) g.order = i })
        return null
      case 'chat_group_remove': {
        const at = chatGroups.findIndex((x) => x.id === args.id); if (at < 0) throw '묶음을 찾을 수 없습니다'
        chatGroups.splice(at, 1)
        Object.values(chats).forEach((c) => { if (c.groupId === args.id) c.groupId = null })
        window.__groupCalls = (window.__groupCalls || []).concat(['remove ' + args.id])
        return null
      }
      case 'chat_set_group': {
        const c = chats[args.chatId]; if (!c) throw '채팅방을 찾을 수 없습니다'
        c.groupId = args.groupId; window.__groupCalls = (window.__groupCalls || []).concat([`set ${args.chatId} ${args.groupId}`])
        return null
      }
      case 'user_profile_get':
        if (query.get('profile') === 'none') throw 'Command user_profile_get not found'
        return { ...(window.__profile || { name: '', about: '', style: '' }), updatedAt: null }
      case 'user_profile_set':
        window.__profileCalls = (window.__profileCalls || []).concat([args])
        window.__profile = { name: args.name, about: args.about, style: args.style }
        return null
      // ---- 버전·업데이트(가짜). ?update=latest(기본) / available / failed / none(명령 없음) ----
      case 'app_version':
        if ((query.get('update') || 'latest') === 'none') throw 'Command app_version not found'
        return { version: '0.1.0', buildDate: '2026-11-15T09:00:00+09:00' }
      case 'update_check': {
        const mode = query.get('update') || 'latest'
        if (mode === 'none') throw 'Command update_check not found'
        if (mode === 'failed') return { state: 'failed', currentVersion: '0.1.0', latestVersion: null, notes: null, publishedAt: null }
        // ?update=norelease: 공개된 릴리스가 없음(D40, 오류 아님).
        if (mode === 'norelease') return { state: 'no_release', currentVersion: '0.1.0', latestVersion: null, notes: null, publishedAt: null }
        if (mode === 'available') return { state: 'available', currentVersion: '0.1.0', latestVersion: '0.2.0', notes: ['보기 설명: 채팅방 목록을 읽기 쉽게 고쳤습니다.', '보기 설명: 검색 창을 더했습니다.'].join(String.fromCharCode(10)), publishedAt: '2026-11-16T09:00:00+09:00' }
        return { state: 'up_to_date', currentVersion: '0.1.0', latestVersion: '0.1.0', notes: null, publishedAt: null }
      }
      case 'update_install': {
        let p = 0
        const timer = setInterval(() => {
          p += 20
          if (query.get('installfail') && p >= 60) { clearInterval(timer); emit('update-failed', { state: 'failed' }); return }
          if (p >= 100) { clearInterval(timer); emit('update-done', { state: 'downloaded' }); return }
          emit('update-progress', { downloadedBytes: p * 1000, totalBytes: 100000 })
        }, 300)
        return null
      }
      case 'update_restart':
        window.__restartCalls = (window.__restartCalls || 0) + 1
        return null
      // ---- 몸소 폴더 위치(가짜). ?folder=ok(기본) / onedrive / none ----
      // 실제 계약(archive_location.rs): get → { path, isDefault, onCloudSyncFolder }, set → { state, path, previousPath, previousKept }(끝날 때까지 기다림).
      // 옮기는 동안 archive-location-progress { copiedBytes, totalBytes }, 끝에 archive-location-done / -failed.
      // ?folder=onedrive: 옮긴 곳이 자동 올리기 폴더. ?folder=notempty / invalid: 옮기지 못함. ?folder=nototal: 전체 양을 모름(totalBytes null).
      // ?movems=: 옮기는 데 걸리는 시간(기본 1200).
      case 'archive_location_get':
        if ((query.get('folder') || 'ok') === 'none') throw 'Command archive_location_get not found'
        return { path: window.__folderPath || ['C:', 'Users', 'teacher', 'Documents', 'momso'].join(String.fromCharCode(92)), isDefault: !window.__folderPath, onCloudSyncFolder: window.__folderPath && query.get('folder') === 'onedrive' ? true : false }
      case 'archive_location_open':
        window.__folderOpened = (window.__folderOpened || 0) + 1
        return null
      case 'archive_location_set': {
        const folder = query.get('folder')
        const ms = Number(query.get('movems') || 1200)
        const total = folder === 'nototal' ? null : 48000000
        const previous = window.__folderPath || ['C:', 'Users', 'teacher', 'Documents', 'momso'].join(String.fromCharCode(92))
        emit('archive-location-progress', { copiedBytes: 0, totalBytes: null })
        const finish = (result) => { emit(result.state === 'ok' || result.state === 'unchanged' ? 'archive-location-done' : 'archive-location-failed', result); return result }
        if (folder === 'notempty') return Promise.resolve(finish({ state: 'target_not_empty', path: null, previousPath: null, previousKept: null }))
        if (folder === 'invalid') return Promise.resolve(finish({ state: 'invalid', path: null, previousPath: null, previousKept: null }))
        return new Promise((done) => {
          const started = Date.now()
          const timer = window.setInterval(() => {
            const part = Math.min(1, (Date.now() - started) / ms)
            emit('archive-location-progress', { copiedBytes: Math.round(48000000 * part), totalBytes: total })
            if (part < 1) return
            window.clearInterval(timer)
            window.__folderPath = args.path
            done(finish({ state: 'ok', path: args.path, previousPath: previous, previousKept: true }))
          }, 150)
        })
      }
      // 뒤쪽이 여는 고르기 창(select_local_paths, 2026-11-17). purpose: conversation(내보낸 파일 하나) | source(첨부 여럿) | archive(폴더).
      case 'select_local_paths': {
        const bs = String.fromCharCode(92)
        if (args.purpose === 'conversation') return [['D:', '내려받기', 'MyActivity.json'].join(bs)]
        if (args.purpose === 'archive') return [['D:', 'momso-folder'].join(bs)]
        return [['D:', '수업', '저녁 하타 전사.txt'].join(bs)]
      }
      case 'plugin:dialog|open':
        // 폴더 고르기(몸소 폴더 옮기기)와 파일 고르기(클립, 여럿 고르기)를 가른다.
        // 대화 가져오기: 내보낸 파일 하나(zip·json 거르개).
        if (args && args.options && (args.options.filters || []).some((f) => (f.extensions || []).includes('zip'))) return ['D:', '내려받기', 'conversations.json'].join(String.fromCharCode(92))
        if (args && args.options && args.options.multiple) return [['D:', '수업', '2026-11-15 저녁 하타 수업.m4a'].join(String.fromCharCode(92)), ['D:', '수업', '저녁 하타 전사.txt'].join(String.fromCharCode(92))]
        return ['D:', 'momso-folder'].join(String.fromCharCode(92))
      case 'provider_auth_status': case 'provider_login_start': case 'provider_login_cancel': case 'provider_logout': case 'provider_api_key_set': case 'provider_api_key_clear': case 'install_external_provider':
      case 'provider_api_key_verify': case 'provider_first_run_eligible':
        return authCommand(command, args)
      case 'momso_username_get': case 'momso_username_set':
        return usernameCommand(command, args)
      // 받아쓰기(가짜). ?voice=unsupported 면 쓸 수 없음, ?voiceopen=not_focused 처럼 열기 결과를 바꿀 수 있다(기본 requested).
      case 'voice_typing_status':
        if (query.get('voice') === 'none') throw `Command ${command} not found`
        return { state: query.get('voice') === 'unsupported' ? 'unsupported' : 'unknown', microphone: 'legacy_input_detected', language: 'unknown', permission: 'unknown', network: 'unknown' }
      case 'voice_typing_open':
        window.__voiceCalls = (window.__voiceCalls || 0) + 1
        return { state: query.get('voiceopen') || 'requested' }
      case 'archive_auto_backup_get': case 'archive_auto_backup_set':
      case 'archive_remote_status': case 'archive_remote_connect': case 'archive_remote_push': case 'archive_remote_pull': case 'archive_remote_disconnect': case 'archive_remote_create':
      case 'object_store_auto_upload_get': case 'object_store_auto_upload_set':
      case 'archive_additional_files_count':
      case 'object_store_status': case 'object_store_connect': case 'object_store_disconnect': case 'object_store_backup': case 'object_store_restore': case 'app_state_backup': case 'app_state_restore':
      case 'user_owned_backup_overview': case 'export_backup_recovery_key': case 'confirm_backup_recovery_key': case 'import_backup_recovery_key':
        return backupCommand(command, args)
      case 'skill_list': case 'skill_read': case 'skill_create': case 'skill_update': case 'skill_set_enabled': case 'skill_remove':
        return skillCommand(command, args)
      // 채팅방 사본 쓰기 형편(CHAT-ARCHIVE-RETRY.md). ?copyfail=<까닭> 이면 대기 2개, 앱을 켠 뒤 그 까닭으로 chat-archive-changed 를 한 번 보낸다.
      case 'chat_archive_sync_status':
        // ?redaction=pending|upload|expired: 옛 키 다시 가리기(2026-11-17).
        if (query.get('redaction') === 'pending') return { state: 'redactionPending', pendingWrites: 0, pendingDeletions: 0, retiredKeysPending: 1, retiredKeysExpired: 0 }
        if (query.get('redaction') === 'upload') return { state: 'redactionUploadPending', pendingWrites: 0, pendingDeletions: 0, retiredKeysPending: 1, retiredKeysExpired: 0 }
        if (query.get('redaction') === 'expired') return { state: 'saved', pendingWrites: 0, pendingDeletions: 0, retiredKeysPending: 0, retiredKeysExpired: 1 }
        return query.get('copyfail') || query.get('copypending') ? { state: 'pending', pendingWrites: 2, pendingDeletions: 0 } : { state: 'saved', pendingWrites: 0, pendingDeletions: 0 }
      // 앱 시작 진행(2026-11-17). ?startup=migrating|backup|checking 이면 끝나지 않은 형편을 준다(그동안 목록은 ?slowcmd=chat_list_page 로 늦춘다).
      case 'runtime_startup_status': {
        const st = query.get('startup')
        if (st) return { state: st, completed: st === 'checking' ? 0 : st === 'backup' ? 1 : 2, total: 4, available: false }
        return { state: 'complete', completed: 4, total: 4, available: true }
      }
      // 채팅방 목록 쪽 나눔(2026-11-17). ?listpage=none 이면 명령이 없는 옛 뒤쪽.
      case 'chat_list_page': {
        if (query.get('listpage') === 'none') throw 'Command chat_list_page not found'
        window.__listPageCalls = (window.__listPageCalls || []).concat([args.offset ?? 0])
        const all = chatList()
        const start = Number(args.offset || 0)
        const items = all.slice(start, start + 80)
        const next = start + items.length
        return { items, hasMore: next < all.length, nextOffset: next < all.length ? next : null }
      }
      // 처음 받아 오기 지금 형편(2026-11-17). ?receivestatus=receiving|conflict 로 앱을 켤 때의 형편을 흉내 낸다.
      case 'archive_first_receive_status': {
        const st = query.get('receivestatus')
        if (st === 'receiving') return { completed: 1, total: 3, state: 'indexing', available: false, reason: null, conflictPaths: [] }
        if (st === 'conflict') return { completed: 0, total: 3, state: 'interrupted', available: false, reason: 'first_receive_conflict', conflictPaths: ['AGENTS.md', 'CLAUDE.md', 'zettels/2026-11-15-hatha.md', 'dictionary/people.json'] }
        return { completed: 0, total: 3, state: 'complete', available: true, reason: null, conflictPaths: [] }
      }
      case 'media_list':
        return mediaList()
      case 'source_asset_open': case 'source_asset_delete_preview': case 'source_asset_trash': case 'source_asset_restore_trashed': case 'source_asset_purge': case 'source_asset_deleted_list':
        return sourceCommand(command, args)
      case 'panel_browser_open': case 'panel_browser_close': case 'panel_browser_visibility': case 'panel_browser_bounds': case 'panel_browser_navigate': case 'panel_browser_action': case 'panel_browser_open_human': case 'panel_browser_download_select': case 'panel_download_reveal': case 'panel_browser_open_agent': case 'panel_agent_status': case 'panel_agent_take_over': case 'panel_agent_resume': case 'panel_agent_external_login_open':
        return panelCommand(command, args)
      case 'chat_items':
        return chatItems(args.chatId)
      // ---- Git 설치(가짜, B12). ?git=missing 이면 approvalRequired, ?gitfail=networkFailed 처럼 실패를 흉내 낸다. ----
      case 'git_install_status':
        if (window.__gitInstalling) return 'inProgress'
        return query.get('git') === 'missing' && !window.__gitInstalled ? 'approvalRequired' : 'installed'
      case 'git_install': {
        if (!args.approved) return 'approvalRequired'
        window.__gitInstalling = true
        let done = 0
        const total = 52000000
        const timer = setInterval(() => {
          done += 13000000
          if (query.get('gitfail') && done >= 26000000) { clearInterval(timer); window.__gitInstalling = false; emit('git-install-changed', { provider: 'git', state: 'installFailed', stage: query.get('gitfail'), completed: 0, total: null }); return }
          if (done >= total) { clearInterval(timer); window.__gitInstalling = false; window.__gitInstalled = true; emit('git-install-changed', { provider: 'git', state: 'installed', stage: 'complete', completed: 0, total: null }); return }
          emit('git-install-changed', { provider: 'git', state: 'inProgress', stage: 'downloading', completed: done, total })
        }, 500)
        return 'inProgress'
      }
      // ---- 원자료 찾기(가짜): source_search({ query }) → 파일 이름에 그 말이 든 원자료. ----
      case 'source_search': {
        const all = [
          { id: 'asset-1', originalFileName: '2026-11-15 저녁 하타 수업.m4a', mediaType: 'audio/mp4', size: 18200000, createdAt: '2026-11-15T21:10:00+09:00', linkedDocumentIds: [] },
          { id: 'asset-2', originalFileName: '저녁 하타 전사.txt', mediaType: 'text/plain', size: 4200, createdAt: '2026-11-15T21:30:00+09:00', linkedDocumentIds: [listEntries()[0]?.id].filter(Boolean) },
          { id: 'asset-3', originalFileName: '하타 수업 메모 초안.txt', mediaType: 'text/plain', size: 900, createdAt: '2026-11-14T20:00:00+09:00', linkedDocumentIds: [] },
        ]
        const q = String(args.query || '').trim().toLowerCase()
        return all.filter((item) => item.originalFileName.toLowerCase().includes(q))
      }
      case 'chat_import_export': {
        // 계약: chat_import_export({ path, provider }) → { imported, duplicate, skipped, omittedNonTextMessages, omittedAttachments, updated, addedMessages, conflicts }.
        // ?importfail=invalid_format 처럼 실패 값을 흉내 낸다. 제미나이는 unsupported_format.
        window.__importCalls = (window.__importCalls || []).concat([[args.provider, args.path]])
        // 2026-11-17 제미나이도 받는다(?geminifail=gemini_json_required|gemini_file_too_large).
        if (args.provider === 'gemini' && query.get('geminifail')) throw query.get('geminifail')
        if (query.get('importfail')) throw query.get('importfail')
        return new Promise((done) => setTimeout(() => done({ imported: 2, duplicate: 1, skipped: 0, omittedNonTextMessages: 0, omittedAttachments: 0, updated: 0, addedMessages: 0, conflicts: 0 }), 900))
      }
      case 'yoga_terms_list': {
        // 용어 사전(가짜). ?big=1 이면 용어 1,000개·사람 이름 300개, 아니면 용어 몇 개. ?dict=fail 이면 읽기 실패, ?dict=slow 면 3초 뒤.
        if (query.get('dict') === 'fail') throw 'dictionary_unavailable'
        const n = bigMode ? 1000 : 6
        const cats = ['자세', '호흡·정화', '반다·무드라', '철학·수련', '유파·수업', '시리즈', '빈야사 구령', '차크라', '나디·구나', '도구', '만트라·수업 말', '변형 자세']
        const terms = Array.from({ length: n }, (_, i) => ({ id: `yoga-${i + 1}`, enabled: true, sanskrit: `Asana${i + 1}`, roman: `Āsana ${i + 1}`, korean: [`아사나${i + 1}`], koreanNames: [`${i + 1}번 자세`], meaning: `${i + 1}번째 용어의 뜻입니다.`, category: cats[i % cats.length], variants: [{ text: `아사나 ${i + 1}`, origin: 'observed', recognitionOnly: false }], searchOnly: [{ text: `아 사나${i + 1}`, recognitionOnly: true }], origin: i % 50 === 0 ? 'user' : 'default' }))
        const people = query.get('dict') === 'empty' ? [] : Array.from({ length: bigMode ? 300 : 3 }, (_, i) => ({ id: `person-${i + 1}`, name: `수련생${i + 1}`, aliases: [`${i + 1}번 회원`], confirmed: i % 4 !== 0, origin: 'user' }))
        const value = { bundleVersion: 3, reviewStatus: '사람 검수 전', baseStatus: 'ok', baseConflicts: [], terms: query.get('dict') === 'empty' ? [] : terms, fragments: [], people, peopleStatus: 'ok', userRevision: 'r1', peopleRevision: 'p1' }
        return query.get('dict') === 'slow' ? new Promise((done) => setTimeout(() => done(value), 3000)) : value
      }
      case 'chat_archive_redaction_count':
        // 채팅방 GitHub 사본에서 가린 수. ?redacted=3 이면 3, 없으면 null(사본 없음).
        return query.get('redacted') ? Number(query.get('redacted')) : null
      case 'chat_import_export_preview': {
        // 계약: { conversations, alreadyImported, unreadable, oldest, newest }. ?importpreview=empty 이면 0개.
        // 2026-11-17 제미나이도 받는다(?geminifail=gemini_json_required|gemini_file_too_large).
        if (args.provider === 'gemini' && query.get('geminifail')) throw query.get('geminifail')
        if (query.get('importfail')) throw query.get('importfail')
        // 2026-11-18 제미나이: groupingNotice(30분 규칙), ?geminihtml=1 이면 guidance(JSON 으로 골라 달라)만.
        if (args.provider === 'gemini' && query.get('geminihtml') === '1') return { conversations: 0, alreadyImported: 0, unreadable: 0, oldest: null, newest: null, guidance: '구글 테이크아웃에서 ...' }
        const geminiExtra = args.provider === 'gemini' ? { groupingNotice: '제미나이 기록은 ...' } : {}
        const empty = query.get('importpreview') === 'empty'
        return new Promise((done) => setTimeout(() => done(args.provider === 'gemini' && !empty ? { conversations: 5, alreadyImported: 0, unreadable: 0, oldest: '2026-11-01T09:00:00+00:00', newest: '2026-11-10T12:00:00+00:00', ...geminiExtra } : empty
          ? { conversations: 0, alreadyImported: 0, unreadable: 0, oldest: null, newest: null }
          : { conversations: 40, alreadyImported: 3, unreadable: 1, oldest: '2024-03-02T09:00:00+00:00', newest: '2026-11-10T12:00:00+00:00' }), 400))
      }
      case 'chat_import_export_progress': {
        // 대화 하나마다 chat-import-progress { requestId, processed }. 그만두면 'cancelled'. ?importslow 이면 천천히(사진용).
        window.__importCalls = (window.__importCalls || []).concat([[args.provider, args.path, args.requestId]])
        const step = query.has('importslow') ? 400 : 40
        return new Promise((done, fail) => {
          let processed = 0
          window.__importCancel = window.__importCancel || {}
          const timer = setInterval(() => {
            if (window.__importCancel[args.requestId]) { clearInterval(timer); fail('cancelled'); return }
            processed += 1
            emit('chat-import-progress', { requestId: args.requestId, processed })
            if (processed >= 40) { clearInterval(timer); done({ ...(args.provider === 'gemini' ? { groupingNotice: '제미나이 기록은 ...' } : {}), imported: 36, duplicate: 3, skipped: 1, omittedNonTextMessages: 0, omittedAttachments: 0, updated: 2, addedMessages: 5, conflicts: 0 }) }
          }, step)
        })
      }
      case 'chat_import_export_cancel':
        window.__importCancel = { ...(window.__importCancel || {}), [args.requestId]: true }
        return null
      case 'attachment_text_read': {
        // 계약(T46·T48): { state, content, nextCursor, totalBytes }. 가짜는 2000자씩 잘라 이어 받기를 흉내 낸다.
        window.__textCalls = (window.__textCalls || []).concat([[args.assetId, args.cursor ?? null]])
        const byId = {}
        for (const f of chatItems(chatList()[0]?.id).files) byId[f.id] = f.title
        const title = byId[args.assetId]
        if (!title) return { state: 'not_found', content: null, nextCursor: null, totalBytes: null }
        if (!/[.](txt|md)$/.test(title)) return { state: 'not_text', content: null, nextCursor: null, totalBytes: null }
        const body = title.endsWith('.md')
          ? ['# 수련생 메모', '', '## 왼쪽 어깨', '- 팔을 올릴 때 걸린다고 함', '- 호흡을 길게 두고 높이를 먼저 확인', '', '## 다음 수업', '1. 고양이 자세로 풀기', '2. **아래를 보는 개** 자세는 무릎을 굽혀서'].join(String.fromCharCode(10))
          : Array.from({ length: query.get('filetext') === 'big' ? 400 : 6 }, (_, i) => `${i + 1}. 저녁 하타 수업 전사 줄입니다. 호흡을 길게 두고 천천히 움직입니다.`).join(String.fromCharCode(10))
        const start = Number(args.cursor || 0)
        if (start > body.length) return { state: 'invalid_cursor', content: null, nextCursor: null, totalBytes: body.length }
        const end = Math.min(body.length, start + 2000)
        return { state: 'ok', content: body.slice(start, end), nextCursor: end < body.length ? end : null, totalBytes: body.length }
      }
      case 'ai_models':
        // ?modelsdelay=2000: 모델 목록이 늦게 오는 때(앱을 연 직후)를 흉내 낸다.
        if (query.get('modelsdelay')) return new Promise((done) => window.setTimeout(() => done(aiModels()), Number(query.get('modelsdelay'))))
        return aiModels()
      case 'permission_mode_get':
        if (permMode === 'fail') throw 'Command permission_mode_get not found'
        return permMode === 'old' ? permissionMode : permissionByProvider[args.provider]
      case 'permission_mode_set':
        window.__permissionCalls = (window.__permissionCalls || []).concat([{ provider: args.provider, mode: args.mode }])
        if (permMode === 'old') permissionMode = args.mode
        else permissionByProvider[args.provider] = args.mode
        return null
      case 'permission_modes_available':
        return permissionModes(args.provider)
      case 'chat_send':
        return chatSend(args)
      case 'chat_cancel':
        if (sendMode === 'old') throw 'Command chat_cancel not found'
        if (running[args.chatId]) {
          clearInterval(running[args.chatId])
          delete running[args.chatId]
          emit('chat-error', { chatId: args.chatId, message: '응답을 중단했습니다' })
        }
        return null
      case 'chat_context': {
        if (contextMode === 'fail') throw 'Command chat_context not found'
        if (contextMode === 'none') return { state: 'unavailable', reason: '이 AI에서는 알 수 없습니다' }
        const chat = chats[args.chatId]
        const used = chat ? chat.messages.reduce((sum, m) => sum + m.text.length * 40, 0) : 0
        // 계약(cef940fa): 공통 값 usedTokens, limitTokens, usedPercent(모르면 비움). 줄이기 시작하는 때는 오지 않는다.
        return { state: 'ok', usedTokens: used, limitTokens: 200000, usedPercent: Math.round(used / 2000), provider: 'codex', model: null, updatedAt: new Date().toISOString() }
      }
      case 'chat_list':
        return chatList()
      case 'chat_pin': case 'chat_archive': case 'chat_archived_list': case 'chat_restore': case 'chat_delete':
        return keepCommand(command, args)
      case 'chat_create': {
        const createdAt = nextStamp()
        const id = chatIdFor(createdAt)
        chats[id] = { id, title: '', createdAt, updatedAt: createdAt, messages: [] }
        return chatEntry(chats[id])
      }
      // 긴 채팅방 페이지(2026-11-18 둘째 줄 B84 꼴): 최근부터 limit 개(기본 100, 1~200), 오래된 순, nextCursor 는 그 페이지 첫 말 id(더 없으면 null).
      case 'chat_read_page': {
        const chat = chats[args.id]
        if (!chat) throw '대화를 읽지 못했습니다'
        const limit = args.limit ?? 100
        if (limit < 1 || limit > 200) throw 'invalid_limit'
        let end = chat.messages.length
        if (args.before) { end = chat.messages.findIndex((m) => m.id === args.before); if (end < 0) throw 'invalid_cursor' }
        const start = Math.max(0, end - limit)
        const copy = JSON.parse(JSON.stringify({ ...chat, messages: chat.messages.slice(start, end) }))
        window.__pageCalls = (window.__pageCalls || 0) + 1
        return { ...copy, nextCursor: start > 0 ? chat.messages[start].id : null }
      }
      case 'chat_read': {
        const chat = chats[args.id]
        if (!chat) throw '대화를 읽지 못했습니다'
        // ?msgattach=1 이면 첫 내 말에 첨부를 붙여 준다(뒤쪽에 요청한 chat_read messages[].attachments 꼴).
        const copy = JSON.parse(JSON.stringify(chat))
        if (query.get('msgattach') === '1') { const first = copy.messages.find((m) => m.role === 'user'); if (first) first.attachments = [{ assetId: 'sha256:' + 'a'.repeat(64), name: '수업 사진.jpg', mediaType: 'image/jpeg', deleted: false }, { assetId: 'sha256:' + 'b'.repeat(64), name: '저녁 하타 수업 녹음.m4a', mediaType: 'audio/mp4', deleted: false }, { assetId: 'sha256:' + 'c'.repeat(64), name: '수업 계획.txt', mediaType: 'text/plain', deleted: true }] }
        return copy
      }
      case 'chat_append': {
        const chat = chats[args.id]
        if (!chat) throw '대화를 찾지 못했습니다'
        const message = { id: `${chat.id}-m${chat.messages.length + 1}`, role: args.role, text: args.text, createdAt: nextStamp() }
        chat.messages.push(message)
        chat.updatedAt = message.createdAt
        if (!chat.title && args.role === 'user') chat.title = args.text.trim().slice(0, 40)
        return message
      }
      case 'chat_rename': {
        const chat = chats[args.id]
        if (!chat) throw '대화를 찾지 못했습니다'
        chat.title = args.title
        return null
      }
      // 쪽 나눔(G33). ?searchpage=1 일 때만 있다(없으면 화면이 옛 chat_search 로 돌아가는지 본다). 한 쪽 80개.
      case 'chat_search_page': {
        if (query.get('searchpage') !== '1') throw 'Command chat_search_page not found'
        window.__pageCalls = (window.__pageCalls || []).concat([args.offset ?? 0])
        const all = archive('chat_search', { query: args.query })
        const start = Number(args.offset || 0)
        const items = all.slice(start, start + 80)
        const next = start + items.length
        return { items, hasMore: next < all.length, nextOffset: next < all.length ? next : null }
      }
      case 'chat_search': {
        const q = String(args.query || '').trim().toLowerCase()
        return chatList().flatMap((entry) => {
          const chat = chats[entry.id]
          if (chat.title.toLowerCase().includes(q)) return [{ ...entry, snippet: null }]
          const hit = chat.messages.find((m) => m.text.toLowerCase().includes(q))
          if (!hit) return []
          const at = hit.text.toLowerCase().indexOf(q)
          return [{ ...entry, snippet: (at > 20 ? '…' : '') + hit.text.slice(Math.max(0, at - 20), at + q.length + 40) }]
        })
      }
      case 'run_ai_assistant':
        return new Promise((resolve) => setTimeout(() => resolve({ text: `받은 말: "${args.request.prompt}"
이것은 화면 확인용 가짜 답이다.`, usage: null }), 600))
      case 'archive_status':
        return { path: ['C:', 'example', 'archive'].join(String.fromCharCode(92)), zetCount: 4, receiptCount: 1, hasUnsavedChanges: Boolean(dirtyMode), usesBundledGit: true }
      case 'zet_list':
        if (scenario === 'fail') throw '글 목록을 읽지 못했습니다'
        return scenario === 'empty' ? [] : listEntries()
      // 문서 폴더(가짜, 2026-11-18 뒤쪽 1568490 꼴): 폴더 목록·만들기·문서 옮기기·이름 바꾸기.
      case 'zet_folders': {
        const all = new Set(window.__zetFolders || [])
        Object.values(window.__zetFolderOf || {}).forEach((path) => { const parts = path.split('/').filter(Boolean); for (let i = 1; i <= parts.length; i += 1) all.add(parts.slice(0, i).join('/')) })
        return [...all]
      }
      case 'zet_create_folder': {
        if (!args.folder || args.folder.split('/').some((part) => !part || part.trim() !== part)) throw 'invalid_folder'
        const parts = args.folder.split('/'); window.__zetFolders = window.__zetFolders || []
        for (let i = 1; i <= parts.length; i += 1) { const path = parts.slice(0, i).join('/'); if (!window.__zetFolders.includes(path)) window.__zetFolders.push(path) }
        return null
      }
      case 'provider_login_browser_ack': window.__ackCalls = (window.__ackCalls || []).concat([args.provider]); return 'acknowledged'
      case 'zet_set_folder': window.__zetFolderOf = { ...(window.__zetFolderOf || {}), [args.id]: args.folder }; return null
      case 'zet_rename_folder': {
        const swap = (path) => (path === args.from ? args.to : path.startsWith(args.from + '/') ? args.to + path.slice(args.from.length) : path)
        window.__zetFolders = (window.__zetFolders || []).map(swap)
        window.__zetFolderOf = Object.fromEntries(Object.entries(window.__zetFolderOf || {}).map(([id, path]) => [id, swap(path)]))
        return null
      }
      case 'zet_read': {
        const zet = zets[args.id]
        if (!zet) throw '글을 읽지 못했습니다'
        return { ...JSON.parse(JSON.stringify(zet)), contentHash: hashOf(zet) }
      }
      case 'zet_save': {
        // 주소에 ?savefail=save 를 붙이면 이력에 남기기가 실패한 것처럼 답한다(글은 원래대로).
        window.__saveCalls = (window.__saveCalls || []).concat([{ command: 'zet_save', expectedHash: args.expectedHash === hashOf(zets[args.id]) ? 'matches' : 'stale' }])
        const zet = zets[args.id]
        if (!zet) throw '글 번호가 올바르지 않습니다'
        if (args.expectedHash && args.expectedHash !== hashOf(zet)) throw '이 글이 그사이 바뀌었습니다'
        if (saveFail === 'save') throw '이력 저장 실패: 보관함 저장 작업을 완료하지 못했습니다; 글을 원래대로 돌렸습니다'
        if (args.changes.title !== undefined) zet.head.title = args.changes.title
        if (args.changes.summary !== undefined) zet.head.summary = args.changes.summary
        if (args.changes.body !== undefined) zet.body = args.changes.body
        zet.head.updated_at = new Date().toISOString()
        history[args.id] = history[args.id] || []
        history[args.id].unshift({ revision: Math.random().toString(16).slice(2).padEnd(40, '0').slice(0, 40), time: zet.head.updated_at, title: zet.head.title, otherZets: 0 })
        return null
      }
      case 'zet_linked': {
        const zet = zets[args.id]
        if (!zet) throw '글 번호가 올바르지 않습니다'
        const incoming = []
        for (const other of Object.values(zets)) {
          for (const edge of other.head.edges) {
            if (edge.target === args.id) incoming.push(linkedZet(edge.type, other.head.id))
          }
        }
        return {
          assets: zet.head.assets.map((id, index) => ({ id, originalFileName: index === 0 ? '2026-11-15 저녁 하타 수업.m4a' : null })),
          outgoing: zet.head.edges.map((edge) => linkedZet(edge.type, edge.target)),
          incoming,
          receipts: Object.values(receipts)
            .filter((receipt) => receipt.head.wrote.includes(args.id))
            .map(({ head }) => ({ id: head.id, createdAt: head.created_at, tool: head.tool, model: head.model })),
        }
      }
      case 'zet_update': {
        // 주소에 ?savefail=update 를 붙이면 파일에 쓰기가, ?savefail=archive 를 붙이면 이력에 남기기가 실패한다.
        // ?savefail=archive-once 는 이력에 남기기가 처음 한 번만 실패한다.
        window.__saveCalls = (window.__saveCalls || []).concat(['zet_update'])
        const zet = zets[args.id]
        if (!zet) throw '글 번호가 올바르지 않습니다'
        if (saveFail === 'update') throw '글을 고치지 못했습니다'
        if (args.changes.title !== undefined && !args.changes.title.trim()) throw '제목을 입력해 주세요'
        Object.assign(zet.head, { ...(args.changes.title !== undefined ? { title: args.changes.title } : {}), ...(args.changes.summary !== undefined ? { summary: args.changes.summary } : {}) })
        if (args.changes.body !== undefined) zet.body = args.changes.body
        zet.head.updated_at = new Date().toISOString()
        unsaved[args.id] = true
        return null
      }
      case 'archive_save': {
        window.__saveCalls = (window.__saveCalls || []).concat(['archive_save'])
        if (saveFail === 'archive' || (saveFail === 'archive-once' && !window.__failedOnce)) {
          window.__failedOnce = true
          throw '보관함 검사를 먼저 확인해 주세요'
        }
        const changed = Object.keys(unsaved).filter((id) => unsaved[id])
        if (changed.length === 0) throw '저장할 변경 사항이 없습니다'
        for (const id of changed) {
          unsaved[id] = false
          history[id] = history[id] || []
          history[id].unshift({ revision: Math.random().toString(16).slice(2).padEnd(40, '0').slice(0, 40), time: new Date().toISOString(), title: zets[id].head.title, otherZets: changed.length - 1 })
        }
        return null
      }
      case 'zet_history':
        return history[args.id] || []
      case 'zet_read_version': {
        if (!zets[args.id]) throw '글 번호 또는 이력이 올바르지 않습니다'
        return versionOf(args.id, args.revision)
      }
      case 'zet_restore_version': {
        const zet = zets[args.id]
        if (!zet) throw '글 번호 또는 이력이 올바르지 않습니다'
        window.__restoreCalls = (window.__restoreCalls || []).concat([{ revision: args.revision[0], discardUnsaved: args.discardUnsaved }])
        if (unsaved[args.id] && !args.discardUnsaved) throw '저장하지 않은 내용이 있습니다'
        const version = versionOf(args.id, args.revision)
        if (!version.head) throw '글 머리가 올바르지 않습니다'
        unsaved[args.id] = false
        zet.body = version.body
        zet.head = { ...version.head, updated_at: new Date().toISOString() }
        history[args.id].unshift({ revision: 'c'.repeat(40), time: zet.head.updated_at, title: zet.head.title, otherZets: 0 })
        return null
      }
      // 작업 내역 지우기(2026-11-17). ?receiptfail=1 이면 실패.
      case 'receipt_delete':
        if (query.get('receiptfail') === '1') throw 'unavailable'
        if (!receipts[args.id]) throw 'not_found'
        delete receipts[args.id]
        return { state: 'deleted', id: args.id, chatId: null }
      case 'receipt_read': {
        const receipt = receipts[args.id]
        if (!receipt) throw '작업 기록을 읽지 못했습니다'
        const reference = (id) => {
          if (id.startsWith('sha256:')) return { type: 'asset', id, originalFileName: id === asset ? '2026-11-15 저녁 하타 수업.m4a' : null }
          const { type, ...zet } = linkedZet('references', id)
          return { type: 'zet', ...zet }
        }
        return { head: { ...receipt.head, read: receipt.head.read.map(reference), wrote: receipt.head.wrote.map(reference) }, body: receipt.body }
      }
      case 'plugin:window|destroy':
      case 'plugin:window|close':
        window.__windowCalls = (window.__windowCalls || []).concat([command.split('|')[1]])
        return null
      case 'plugin:event|listen':
        // 창 닫기 요청을 흉내 내려고 듣는 쪽을 적어 둔다. window.__emitCloseRequested() 로 부른다.
        listeners[args.event] = (listeners[args.event] || []).concat([args.handler])
        return args.handler
      case 'plugin:event|unlisten':
        for (const name of Object.keys(listeners)) listeners[name] = listeners[name].filter((handler) => handler !== args.eventId)
        return null
      case 'plugin:opener|open_url':
        window.__openedUrls = (window.__openedUrls || []).concat([args.url])
        return null
      default:
        return undefined
    }
  }

  function app(command, args) {
    switch (command) {
      case 'runtime_info':
        return {
          appVersion: '0.1.0',
          platform: 'windows',
          dataRoot: 'C:\\example',
          databasePath: 'C:\\example\\momso.db',
          objectsPath: 'C:\\example\\objects',
          exportsPath: 'C:\\example\\exports',
          backupsPath: 'C:\\example\\backups',
          logsPath: 'C:\\example\\logs',
          schemaVersion: 1,
          databaseHealth: 'ok',
          womAdapterStatus: 'waiting_for_stable_contract',
        }
      case 'plan_source_asset_import': case 'apply_source_asset_import': case 'chat_attach_asset':
        return attachCommand(command, args)
      case 'list_source_assets':
        return [{ contentSha256: asset.slice(7), originalFileName: '2026-11-15 저녁 하타 수업.m4a' }]
      case 'plugin:window|is_maximized':
        return false
      case 'plugin:event|listen':
        return 1
      default:
        return null
    }
  }

  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} }

  const listeners = {}
  const callbacks = {}
  window.__emitCloseRequested = () => {
    for (const handler of listeners['tauri://close-requested'] || []) callbacks[handler]?.({ event: 'tauri://close-requested', id: handler, payload: null })
  }

  let callbackId = 0
  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: 'main' },
      currentWebview: { windowLabel: 'main', label: 'main' },
    },
    transformCallback(callback) {
      callbackId += 1
      callbacks[callbackId] = callback
      return callbackId
    },
    invoke(command, args) {
      // 2026-11-17 화면 품질 점검: ?failcmd=명령1,명령2(또는 all) 이면 그 명령이 영어 상태 값 'storage_error' 로 실패,
      // ?slowcmd=명령1,명령2(또는 all) 이면 그 명령이 10초 뒤에 답한다(기다리는 동안의 화면 보기). 창·이벤트 명령은 빼고.
      const pick = (name) => { const v = query.get(name); if (!v || command.startsWith('plugin:')) return false; return (v === 'all' && command !== 'runtime_info') || v.split(',').includes(command) }
      const failNow = pick('failcmd')
      const delay = pick('slowcmd') ? 10000 : 120
      return new Promise((resolve, reject) => {
        window.setTimeout(() => {
          try {
            if (failNow) throw 'storage_error'
            const result = archive(command, args || {})
            resolve(result === undefined ? app(command, args || {}) : result)
          } catch (error) {
            reject(error)
          }
        }, delay)
      })
    },
  }
})()
