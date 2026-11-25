# 웹 요청층 연결 및 오류 계약 — S171

기준: 화면 스냅샷 be899df8312bb4e4c7e2dea6c3993ffd7e505e37. 화면 구성 요소는 수정하지 않았다.

## 변경 전 확인

| 응답 또는 상황 | 기존 요청층 동작 |
| --- | --- |
| 503 `waking` | 자동 재시도와 연결 상태 알림 없음. 일반 요청은 `waking` 오류, 연결 확인은 `unreachable`. |
| 401 | 응답의 세부 까닭을 버리고 `signed_out`. |
| 403 | JSON이면 `forbidden`, JSON이 아니면 `signed_out`; 세부 까닭은 사라짐. |
| `browser_memory_low` | HTTP 오류라면 일반 요청의 코드로 전달. 브라우저 도구 결과는 AI의 도구 결과로 전달하며, 종료 이벤트의 표시 종류는 `unknown`일 수 있음. |
| `disk_low`, `backup_disk_low`, `restore_failed`, `proxy_not_running` | HTTP JSON 오류는 일반 요청에서 코드로 전달되지만 구조화된 재시도 여부·상세는 없음. 일부 개별 명령은 기존 화면 계약에 맞게 상태를 변환하거나 연결 확인에서 `unreachable`로 묶음. |
| 답변 스트림 연결 끊김 | 받은 바이트 위치로 재연결, 실패 시 턴 조회. 끝내 확인하지 못하면 실제 끊김 대신 `cli_timeout`. |

## 요청층 계약

- 503 `waking`: GET·HEAD만 `retryAfterSeconds` 뒤 재시도한다. 최대 30회, 전체 대기 3분. 상한이면 `waking_timeout`. POST·PUT·PATCH·DELETE는 재전송하지 않고 `waking` 오류를 준다. 기존 대용량 첨부의 이어 올리기는 별도로 서버의 받은 위치를 확인해 재개한다.
- `getServerConnectionState()`와 `onServerConnectionChange(listener)`는 `{ state: 'unknown' | 'waking' | 'ready' | 'unavailable', retryAfterSeconds: number | null }`를 제공한다. 같은 값은 중복 이벤트를 내지 않는다. 브라우저 이벤트 이름은 `momso:server-connection`이고 같은 값이 `detail`에 실린다. 기존 연결 화면과 중복 UI를 추가하지 않았다. `ready`는 깨어나는 응답 뒤 실제 성공 응답이 돌아온 경우다.
- `ServerError`는 `{ code, status, retryable, detail }`을 가진다. `retryable`은 다시 시도할 여지가 있다는 뜻이며 쓰기를 자동으로 다시 보낸다는 뜻이 아니다. 401·403 JSON의 `error`와 `detail`을 보존한다. 명령별로 정상 응답에 담긴 기존 `status`나 `state`의 변환은 유지한다. 연결 확인은 기존 화면 호환을 위해 `ready`·`signed_out`·`unreachable`을 유지한다.
- 답변 스트림은 기존처럼 마지막 바이트 위치를 이용해 재연결하고, 안 되면 턴을 조회한다. 더 이상 복구하지 못하면 `chat-error`의 `error.code`는 `stream_interrupted`, `retryable`은 `true`다. 깨우기 제한을 넘으면 `detail.cause`는 `waking_timeout`이다. 이전에 받은 답을 처음부터 다시 내보내지 않는다.

## 화면에서 정할 것

- 깨우는 중·다시 준비됨·제한 초과를 어느 화면에서 어떻게 알릴지, 기존 연결 상태 표시와 어떻게 합칠지.
- 각 까닭 코드에 어떤 글과 사용자의 다음 행동을 보여 줄지. UI와 문구는 이번 범위 밖이다.
