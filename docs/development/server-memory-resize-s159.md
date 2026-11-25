# 서버 메모리 진단과 VM 크기 변경 (S159)

## 공개 사이트와 로그인 전 CLI

별도 Debian 12 e2-small VM, 20GB 디스크, 스왑 없음, 로그인 정보가 없는 새 브라우저 프로필에서 2026-11-25에 측정했다. 사이트를 연 뒤 10초 기다렸고 누르기·입력·동의는 하지 않았다. 각 수치는 브라우저 프로세스 트리 PSS와 남은 메모리이며 단위는 MiB다. 이 측정에는 서버 프로세스가 포함되지 않았다.

| 공개 사이트 | 주소 | 브라우저 PSS | 남은 메모리 | 상태 |
|---|---|---:|---:|---|
| 네이버 지도 | https://map.naver.com/ | 547 | 1,162 | 지도 페이지 열림 |
| 카카오맵 | https://map.kakao.com/ | 569 | 1,205 | 지도 페이지 열림 |
| Booking.com | https://www.booking.com/ | 620 | 1,146 | 예약 페이지 열림 |
| Time and Date 달력 | https://www.timeanddate.com/calendar/ | 688 | 1,059 | 달력이 열린 뒤 추가 확인 요청 |
| OpenStreetMap | https://www.openstreetmap.org/ | 측정 못 함 | 측정 못 함 | 브라우저 도구의 이동이 `unavailable`; 같은 VM의 HTTPS HEAD 요청은 200 |

브라우저가 사이트에 따라 로딩하는 내용과 광고가 달라 수치는 매번 달라진다. 사이트를 실제로 조작하거나 로그인한 뒤의 메모리는 측정하지 않았다.

| CLI | 설치와 버전 | 로그인 전 상태 | PSS |
|---|---|---|---:|
| Codex | 제품과 같은 공식 Linux 단독 실행 파일 0.169.0 | `bubblewrap` 설치 후 제품의 `app-server`는 로그인 없이 대기. 일반 대화 실행은 패키지 오류로 종료했으나 `--no-daemon`은 로그인 방법 선택에서 대기 | 약 74MiB (`app-server`), 약 108MiB (`--no-daemon`) |
| Claude | 공식 npm 패키지 2.1.325 | 표시 방식 선택 뒤 로그인 방법 선택에서 대기 | 약 165MiB |
| Gemini | 공식 npm 패키지 0.71.1 | 시험용 빈 폴더 신뢰 선택 뒤 인증 방법 선택에서 대기 | 약 370MiB |

서버 제품은 Codex 실행 파일을 설치하고 Claude·Gemini는 CLI가 아니라 API로 호출한다. 따라서 Claude·Gemini CLI 수치는 제품의 API 방식 비용이 아니라 별도 비교 수치다. 세 CLI는 인증 전 화면에서 측정했으므로 실제 답변 생성이나 브라우저 도구 호출 중의 최고 메모리가 아니다.

새 Debian 12 시험 VM에서 단독 실행 파일을 인자 없이 터미널에서 실행했을 때 오류 원문은 다음과 같다.

```text
Error: this CLI has no complete local package; install a packaged Codex CLI or use the standalone installer
To work without the background server, rerun the same command with --no-daemon (including resume or fork and its arguments).
```

이 실행은 서버가 사용하는 로그인·대화 실행과 다르다. 서버 로그인은 `codex login --device-auth`, 대화는 `codex app-server`를 사용한다. 설치기가 지정한 SHA-256을 확인한 실제 Codex 0.169.0과 새 Debian 12 e2-small VM에서 서버를 실행해 시험했다. 서버의 연결 시작은 `not_connected`에서 기기 로그인 `waiting`까지 진행됐고, 인증번호는 사용하지 않고 취소했다. 서버와 같은 `CODEX_HOME`과 작업 폴더를 쓰는 실제 `app-server`에 초기화와 `thread/start`를 직접 요청한 결과 둘 다 성공했다. 서버가 넣는 동적 도구를 포함한 실제 턴과 로그인 후 답변은 시험하지 않았으므로 그 단계의 정상 동작까지 확인한 것은 아니다. 시험은 `scripts/s159_real_codex_prelogin.py`에 남겼다.

별도 측정값을 단순히 더하면 네이버 지도 한 방의 브라우저와 로그인 전 CLI는 Codex `app-server` 약 621MiB, Claude 약 712MiB, Gemini 약 917MiB다. 서로 다른 시점의 PSS를 더한 추정치이므로 공유 메모리와 실행 중 증가분은 반영하지 못한다. S155의 서버 PSS 약 20MiB도 별도로 필요하다. e2-small에서 세 경우 모두 **한 방만 보수적으로 예상**하며, Gemini CLI 조합은 여유가 특히 작다. 두 방 이상이나 실제 AI 답변 중의 안정성은 실측하지 않았다. 기본 VM은 본부 결정대로 e2-small을 유지한다.

## 소유자 메모리 진단

`GET /diagnostics/memory`는 기존 서버 인증을 요구하며 `memTotalKiB`, `memAvailableKiB`, `minAvailableKiB`, `browsers`, `evictionCount`, `lastEvictionAt`, `memoryLowCount`를 준다. `browsers` 항목은 `chatId`, `state`(`open` 또는 `busy`), `pssKiB`다. 사용 중인 방도 브라우저 프로세스를 확인하며, 프로세스가 사라지거나 측정 권한이 없으면 PSS는 null이다. 카운터와 마지막 시각은 서버가 시작된 뒤의 값이다. 주소·페이지 내용·로그인 값은 포함하지 않는다.

브라우저를 쓰는 방 수는 제한하지 않는다. 새 브라우저를 열 때 사용 가능한 메모리가 기준보다 작으면 오래 사용하지 않은 유휴 방부터 닫는다. 사용 중인 방은 닫지 않으며 그래도 메모리가 부족하면 `browser_memory_low`를 돌려준다.

개발 VM에서 실제 Chrome과 가짜 사이트·가짜 AI를 이용해 세 채팅방의 브라우저가 동시에 열려 있고 강제 종료가 없는 것을 확인했다. 메모리가 부족해졌을 때 유휴 방만 정리하는 것은 가짜 메모리 값을 사용하는 단위 시험으로 확인했다.

화면에서 정할 것: 메모리가 모자랄 때 무엇을 보여 줄지와 VM 크기를 키우도록 권할지.

## 크기 변경 명령

`scripts/momso_anywhere_resize.py`는 설치 상태 파일과 계정·프로젝트를 대조하고, 소유한 VM과 입구를 확인한 뒤 유지보수 문을 닫고 유휴·백업 준비 상태를 확인한다. 그다음 VM 중지 → 크기 변경 → 시작 → 입구의 인증된 `/health` 확인 → 유지보수 해제 순서로 진행한다. 실패 시 원래 크기로 복구를 시도한다. 복구나 잠금 해제를 확인할 수 없으면 성공으로 표시하지 않는다.

**본부 결정:** 이번 범위는 PC 명령까지다. 휴대폰에서 크기 바꾸기는 입구 권한을 넓혀야 해서 보류하고 화면을 볼 때 정한다. PC 명령의 입구 `/health` 확인도 소유자 브라우저 세션을 요구하지만 설치 상태 파일과 Google 계정만으로는 이 HttpOnly 세션을 얻을 수 없다. 현재 명령은 `--owner-session-file`로 이미 확보된 소유자 세션을 필요로 한다. 그 세션이 없으면 VM을 멈추기 전에 `health_not_ready`로 종료한다. 실제 고객 VM에는 이 명령을 실행하지 않았다.

입구 세션과 VM 밖의 제어·복구 경로에서 남은 사항은 `server-resize-owner-session-s163.md`에 적었다.

Debian 시험 VM에서 실제 유지보수 잠금 → e2-small 중지 → e2-medium 변경 → 시작 → 시험 서버의 상태 확인 → 잠금 해제를 확인했다. 시험 VM에는 실제 입구가 없어 소유 VM 확인과 공개 입구 상태 확인은 시험용 함수로 대체했다. 따라서 이 결과는 실제 입구를 통한 전체 경로의 성공을 뜻하지 않는다. 가짜 클라우드 시험은 실패와 복구를 포함해 8개 통과했다.
