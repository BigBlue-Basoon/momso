# 처음 Google Cloud를 쓰는 사람의 시작 경로 (S112)

확인일: 2026-11-24. 실제 새 Google 계정은 시험하지 않았다. 아래는 공식 문서와 가짜 응답으로 확인한 범위이며, 확인하지 못한 오류는 추측하지 않는다.

| 단계 | 제품이 할 일 | 사람이 할 일 | 현재 한계 |
|---|---|---|---|
| Google 계정과 OAuth | `openid`, `email`, `cloud-platform` 권한을 Google에 요청하고 접근 토큰은 메모리에만 둔다. | Google 계정 로그인과 권한 동의. | 동의 거절은 다시 승인해야 한다. `cloud-platform`은 넓은 권한이며 Google OAuth 앱의 심사·시험 사용자 설정은 별도다. [OAuth](https://docs.cloud.google.com/resource-manager/docs/authorizing) |
| Cloud 약관 | 사용자에게 Google Cloud 콘솔 진입을 제안하고, 돌아오면 같은 API 작업을 다시 확인한다. | 약관을 읽고 동의할지 결정한다. | `projects.create`의 약관 미동의 전용 오류는 [공식 API 문서](https://docs.cloud.google.com/resource-manager/reference/rest/v3/projects/create)에서 확인하지 못했다. 403을 약관 미동의로 단정하지 않는다. `terms_not_accepted`는 만들지 않았다. |
| 프로젝트 | Resource Manager API로 사용자가 고른 기존 프로젝트를 확인하거나 새 프로젝트를 만든다. [프로젝트 생성](https://docs.cloud.google.com/resource-manager/docs/creating-managing-projects) | 새 프로젝트 생성이 금지되거나 할당량이 없으면 Google 화면에서 권한·할당량 문제를 해결한다. | 프로젝트 생성·결제 계정 생성은 별개다. 403은 여러 원인이 가능하므로 `permission_denied`, HTTP 429는 `quota_exceeded`; 모든 할당량 오류가 429라는 보장은 없다. [할당량](https://docs.cloud.google.com/resource-manager/docs/limits) |
| 결제 계정 | 접근 가능한 활성 결제 계정을 목록으로 읽고 선택한 계정을 프로젝트에 연결한 뒤 다시 확인한다. 여러 개면 자동으로 고르지 않는다. | 계정이 없으면 Google에서 만들거나 필요한 권한을 받는다. | 목록이 비어 있으면 `billing_account_missing`이다. 이는 **접근 가능한 활성 계정이 없음**이지 실제 계정 자체가 없다는 확정이 아니다. `billing_inactive`, `billing_account_unavailable`, `billing_selection_required`, `billing_not_ready`는 기존 계약이다. [결제 계정 관리](https://docs.cloud.google.com/billing/docs/how-to/manage-billing-account) |
| 무료 체험·카드·본인 확인 | Google의 무료 체험 안내로 이동할 수 있게 한다. | 자격을 확인하고 본인이 결제수단과 요구되는 신원 확인을 제공한다. | 신규 프로젝트마다 체험이 보장되는 것은 아니다. 카드가 반드시 필요한지 API 응답만으로 확인하지 못했으므로 `card_required`라고 단정하지 않는다. 요금과 체험 조건은 Google이 결정한다. [무료 프로그램](https://docs.cloud.google.com/free/docs/free-cloud-features) |
| API 켜기와 설치 | 허용된 프로젝트에서 필요한 서비스 API를 켜고 VM·입구 등을 설치하며 재시도한다. | Google이 요구하는 동의나 결제 설정을 마친다. | 회사 조직 정책·권한·할당량에 따라 자동 설치가 거부될 수 있다. 가짜 응답 시험만 했으며 실제 신규 계정 수용 여부는 미확인이다. |

Google의 약관 수락이나 카드·신원 입력을 대신하지 않는다. Google이 콘솔을 끝낸 뒤 `/start/`로 자동 리디렉션한다는 보장은 확인하지 못했다. 사용자가 브라우저 뒤로 가기로 시작 화면에 돌아와야 할 수 있다.

## 화면 담당 계약

- 시작 화면에서 실패한 동작은 기존 `retryLastAction`의 세션 기록에 남는다. `billing_account_missing`에는 접근 가능한 활성 계정이 없다는 안내와 결제 설정 열기를 제공한다. 카드가 필요하다고 단정하지 않는다.
- `openGoogleSetup({destination})`은 누름 동작에서만 호출한다. 허용 값은 `cloud`, `billing`, `trial`, `quota`이며 URL은 코드에 고정돼 있다. `cloud`는 Cloud 콘솔, `billing`은 결제 콘솔, `trial`은 무료 체험 안내, `quota`는 프로젝트 생성 화면이다. 임의 URL은 거절한다.
- 화면이 돌아왔을 때 기존대로 `resumeAfterRedirect({clientId, release, onProgress, ...})`를 한 번 부른다. 브라우저가 페이지를 복원하는 경우에도 동작하도록 `pageshow` 복원 시 재호출하고 중복 호출은 막는다. Google 화면을 열기 전에 저장한 작업이 있으면 이를 재확인하고 `{kind, ...}`를 반환한다. 새 페이지에서 토큰이 사라졌거나 만료됐다면 먼저 Google OAuth를 다시 거쳐 `{kind: 'redirecting'}`이 된다. 새 OAuth 동의·로그인이 필요할 수 있다.
- Google 콘솔이 성공을 보증하지 않으므로 뒤로 돌아온 뒤에도 기존 `permission_denied`, `quota_exceeded`, `billing_inactive` 등을 그대로 보여 주고 재시도할 수 있게 한다. 403을 `terms_not_accepted` 또는 `card_required`로 바꾸지 않는다.
- 처음부터 프로젝트나 결제 목록을 읽기 전 Google 화면을 열려면 실패한 작업의 재시도 기록이 먼저 필요하다. 기록이 없을 때는 `checkpoint_unavailable`이다.
- 텍스트와 시각 디자인은 화면 담당이 정한다. 실제 계정·카드 정보는 몸소에 입력하지 않는다.

## 본부의 실제 신규 계정 시험

1. 사용자의 허락을 받은 새 Google 계정으로 `/start/`에서 로그인·권한 동의를 한다. 비밀번호와 인증번호는 사용자가 직접 입력한다.
2. Cloud 약관이 미동의라면 Google의 실제 단계와 오류 코드를 기록하되 토큰·개인정보는 기록하지 않는다. 동의 후 브라우저 뒤로 가기로 돌아가 이어지는지 본다.
3. 활성 결제 계정이 없는 경우 목록과 `billing_account_missing` 여부를 확인한다. Google에서 체험·결제 가입 여부는 사용자가 결정한다. 필요한 카드·본인 확인은 사용자가 Google에 직접 한다.
4. 돌아온 뒤 결제 계정이 다시 조회되고, 고른 계정·프로젝트가 보존되며 설치가 이어지는지 본다. OAuth를 다시 요구한 경우도 기록한다.
5. 다수 결제 계정, 프로젝트 권한 거부, 프로젝트 할당량, 동의 거절을 각각 시험하고, 실제 API의 상태·구조화된 오류 이유만 개인정보 없이 기록한다. 예상과 다르면 코드에서 명시적으로 구분한다.

## 충실도 확인

사람의 약관·카드·신원 확인은 자동 처리하지 않으며 사용자 결정을 요구한다. 프로젝트·API 설치는 기존 자동 경로를 유지한다. 새 계정 오류의 종류와 콘솔에서의 자동 복귀 가능 여부는 문서에서 확인되지 않아 미확인으로 남겼다. 원 요청과 의미가 달라진 결정·수치: 없음.
