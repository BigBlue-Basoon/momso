# 데스크톱 업데이트 계약 (S69)

## 화면이 부르는 명령과 받는 값

- `app_version()` → `{version, buildDate?}`.
- `update_check()` → `{state, currentVersion, latestVersion?, notes?, publishedAt?, reason?}`. `state`는 `available`, `up_to_date`, `no_release`, `failed`, `unavailable`이다. 설치된 판은 현재 `0.1.1`이다. 자동 확인에서 새 판을 찾으면 같은 값을 `update-available` 이벤트로 보낸다. 확인 결과는 최대 6시간 캐시된다.
- `update_install()`은 다운로드와 서명 확인을 한다. `{state: "downloaded"}`이면 설치할 수 있고 `busy`, `failed`, `unavailable`도 나온다. 진행은 `update-progress {downloadedBytes, totalBytes?}`, 결과는 `update-done` 또는 `update-failed {state, reason?}`이다.
- `update_restart()`는 서명 확인이 끝난 파일을 설치한다. 진행 중인 채팅이 있으면 `{state: "busy", reason: "active_turn"}`을 주고 설치하지 않는다. 화면은 일을 마친 뒤 다시 시도하게 안내한다. 설치 시작까지 새 채팅 턴이 끼어들지 않는다. 다른 업데이트 작업 중의 `busy`는 `reason`이 없다. 데이터 백업이 실패하면 `backup_failed`, 복구 프로세스를 준비하지 못하면 `installation_failed`, 비동기 작업 실패는 `worker_failed`다. 정상 설치 시작 시 앱이 종료되므로 이벤트가 화면에 도착한다고 보장할 수 없다.
- `signature_invalid`, `connection_failed`, `timeout`, `download_http_error` 등 실패 `reason`은 `app_updates.rs`의 값이다. 서명이 틀리거나 다운로드가 중단되면 설치하지 않는다.

## 설치 중 실패와 복구

Windows에서는 데이터베이스와 이전 설치 파일을 먼저 별도로 저장하고 각 파일의 해시를 확인한다. 이전 앱의 복사본에서 실행한 별도 복구 프로세스가 원래 앱 종료를 기다린 뒤, 서명 확인이 끝난 NSIS 설치기를 Windows 셸로 실행하고 종료를 기다린다. 정상 종료면 새 앱을 열고, 실패 또는 설치기 강제 종료면 사본의 원래 경로·해시를 확인한 후 이전 파일을 복원하고 이전 앱을 연다. 설치 프로세스를 확인할 수 없는 경우에는 쓰는 중일 수 있으므로 무작정 복원하지 않고 사본을 남긴다.

복원은 이전에 존재한 파일을 되돌리고 이전 판에 없던 설치 파일은 삭제한다. 몸소 폴더와 앱 자료·기록 폴더는 복원과 삭제 대상에서 제외한다. 복구 사본은 앱 데이터의 `backups/update-recovery`에 잠시 남는다. 복원한 판 또는 새 판의 앱 준비가 정상으로 끝나면 그 사본을 정리하고, 잠겨 있어 정리하지 못하면 다음 시작 때 다시 시도한다.

S81에서는 업데이트 전에 Windows의 현재 사용자 `Run` 항목에 사본 안 복구 도우미를 등록한다. 컴퓨터가 꺼졌거나 도우미가 종료되면 다음 사용자 로그인 때 도우미가 실행될 수 있다. 앱이 먼저 시작되면 앱이 미완료 기록을 확인하고 도우미를 실행한다. 도우미는 이전 설치기나 앱이 아직 실행 중이면 파일을 건드리지 않는다. 새 앱이 정상 준비를 끝내면 등록을 제거한다. Windows가 로그인 시 실행을 지연할 수 있으며 로그인 자체가 없으면 실행되지 않는다. 복구 과정이 실패하면 사본과 등록이 남는다. Windows의 시작 명령 길이 제한을 넘는 경로에서는 업데이트 준비를 실패로 돌려 기존 앱을 유지한다. [Microsoft의 Run 키 설명](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys).

이 방식은 업데이트를 시작하는 **이전 판 자체에 S81 도우미가 들어 있어야** 작동한다. 이전 판에 이 기능이 없던 사용자가 S81을 처음 받는 한 번의 설치에는 그 이전 판의 복구 기능만 적용된다.

## 격리 설치 시험

Windows 빌드 잠금을 잡고 `MOMSO_S69_TEST_INTERRUPT=1` 환경 변수를 설정한 뒤 `python scripts/test-desktop-updater-s69.py`를 실행한다. 시험 전용 고유 제품명·실행 파일·설치 폴더·자료 폴더·임시 서명 키를 사용한다. 루프백 HTTP는 시험 전용 빌드에서만 허용한다. 보통 제품은 HTTPS만 허용한다. 시험용 개인 키는 세 파일에 서명한 직후 지운다.

시험은 배포 절차와 같은 Node 22.23.5 압축 파일의 SHA-256과 Microsoft 서명 CRT DLL을 확인하고 설치된 Node·npm·DLL의 해시를 대조한다. 그 뒤 실제 Tauri 시험 앱에서 0.1.1 → 0.1.2 다운로드·서명 확인·설치·재시작, 잘못된 서명과 잘못된 설치 형식 거절을 확인한다. 마지막으로 시험용 서명 설치기가 이전 실행 파일을 일부 덮은 지점에서 그 설치기만 강제 종료하고, 복구한 이전 실행 파일의 해시와 이전 앱 재시작을 확인한다. 실제 계정과 실제 릴리스 서명 키는 사용하지 않는다. 시스템이 임시 파일이나 시험 산출물을 붙잡아 정리하지 못하면 경로를 출력한다.

S81의 같은 시험은 정상 시작 뒤 사본이 사라지는지 확인한다. 별도 `power` 시나리오는 시험 설치기와 복구 도우미를 종료하고, 도우미를 다시 실행해 이전 실행 파일의 해시와 재시작, 새 설치 파일 삭제, 시험용 시작 등록 삭제를 확인한다. 설치기가 아직 실행 중일 때는 복원하지 않는지도 본다. 실제 전원을 끄거나 실제 사용자 시작 항목을 변경하는 시험은 아니다.

## 공개할 때

현재 공개 주소는 `https://github.com/BigBlue-Basoon/momso/releases/latest/download/latest.json`이다. 기존에 정해진 GitHub Releases를 그대로 쓴다. Tauri는 고정 JSON과 서명된 설치 파일을 지원하므로 별도 배포 서버가 필요하지 않다. 서명 키는 개발자만 가진다. 한 번 해야 하는 일은 [릴리스 자동화 안내](../../apps/alpha/desktop/RELEASE-AUTOMATION.md)에 적힌 두 GitHub Actions 비밀값을 직접 넣는 것이다. 이후 작업 흐름 파일의 `.disabled`를 제거하고 검토·반영해야 태그나 수동 실행이 가능하다. 지금은 꺼져 있다. 로컬 서명 방법은 [릴리스 안내](../../apps/alpha/desktop/RELEASE.md)에 있다. 개인 키를 AI에 전달하지 않는다.

공식 근거: [Tauri updater](https://v2.tauri.app/plugin/updater/), [GitHub 릴리스의 최신 파일 주소](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases). NSIS의 설치 실패 콜백만으로 강제 종료나 전원 손실을 보장하지 않는 점은 설치된 NSIS 도움말(`NSIS.chm`, `Section4.7.html`)과 별도로 실행되는 복구 프로세스의 실제 강제 종료 시험으로 확인했다.

## 첫 정식 릴리스 뒤 확인 순서

서명 파일과 `latest.json`이 준비되면 먼저 앱에 포함된 공개키로 서명을 검증한다. 정식 `v0.1.1` 릴리스에는 `momso_0.1.1_x64-setup.exe`, 같은 이름의 `.exe.sig`, `latest.json`을 둔다. 서명 도구가 만드는 목록의 설치 파일 주소는 `releases/download/v0.1.1/momso_0.1.1_x64-setup.exe`이며 릴리스 검사 도구의 태그·파일명 규칙과 같다. 공개 후 `python scripts/momso-release-channels.py verify-desktop --tag v0.1.1`로 최신 릴리스의 파일을 확인하고, 실제 `releases/latest/download/latest.json`과 목록의 설치 파일 주소를 GitHub 리다이렉트를 따라 확인한다. 이후 별도 시험 환경에서 이전 판에서 새 판을 찾아 서명을 검증하고 설치·재시작하는지 확인한다. 공개와 실제 사용자 업데이트는 별도 승인된 릴리스 작업이다.

격리 시험의 로컬 HTTP 주소와 가짜 공개키는 실제 GitHub HTTPS 주소·개발자의 공개키와 다르다. 시험 앱은 별도 제품 이름·식별자·자료 폴더와 루프백 허용 및 자동 검사용 `g61-test-probe` 기능을 사용한다. 이 차이와 로컬 서버가 GitHub의 실제 리다이렉트·네트워크를 재현하지 않는 한계를 공개 후 점검에 반영한다. 시험용 열쇠와 설치 파일은 공개하지 않는다.

2026-11-25 S137: 공개 커밋 `f94a534739cbfe44ea3f7ea8f610a20ef15b7896`에서 추출한 파일을 커밋과 대조한 뒤, 제품 소스는 그대로 두고 시험 도구 두 파일을 보강했다. 가짜 열쇠와 로컬 서버, 별도 제품 이름·자료 폴더·Windows 데스크톱을 사용했다. 시험용 0.1.1 앱이 서명된 0.1.2를 내려받아 설치하고 재시작했으며 정상 시작 후 복구 사본이 지워졌다. 잘못된 서명, 낮은 버전, 잘못된 JSON, 서명된 잘못된 설치 파일은 거절됐고 이전 실행 파일이 유지됐다. 낮은 버전과 잘못된 JSON은 설치 파일 요청도 없었다. 시험 앱은 디버그 빌드와 `g61-test-probe`를 사용하므로 실제 배포용 릴리스 바이너리 자체의 검증은 아니다. 시험 열쇠와 시험 설치는 제거됐고, 실제 서명 열쇠와 실제 계정은 사용하지 않았다.
