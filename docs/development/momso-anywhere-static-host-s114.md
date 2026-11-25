# 공통 로그인·시작 사이트 정적 이전 (S114)

확인일 2026-11-24. 이 문서는 준비 절차다. 배포·도메인·OAuth 설정을 변경하지 않았고 실제 호스팅에서 아직 시험하지 않았다.

## 선택과 비용

| 항목 | Cloudflare Pages Free | GitHub Pages 공개 저장소 |
|---|---|---|
| 정적 요청·전송 | 공식 [요금 안내](https://pages.cloudflare.com/)에서 Free 정적 요청과 대역폭을 무제한으로 표시한다. Functions는 별도 Workers 한도이므로 쓰지 않는다. | [사이트 1GB, 대역폭 월 100GB 소프트 한도](https://docs.github.com/en/pages/getting-started-with-github-pages/github-pages-limits). 초과 시 사이트 제공이 제한될 수 있다. 확인한 문서에는 초과분 자동 청구가 명시되지 않는다. |
| 빌드·한도 | [월 500회, 동시 1회, 20분 제한](https://developers.cloudflare.com/pages/platform/limits/). 직접 업로드를 사용한다. 확인한 문서에는 한도 초과 시 정확한 처리·자동 청구 여부가 명시되지 않아 단정하지 않는다. | 기본 빌드 시간당 10회 소프트 한도, 별도 Actions 빌드에는 해당 한도 예외. 공개 저장소 표준 Actions 실행기는 무료지만 유료 실행기 등은 별도다. |
| 도메인·HTTPS | [서브도메인은 CNAME](https://developers.cloudflare.com/pages/configuration/custom-domains/)으로 Pages에 연결하고 Pages에서 도메인 추가·인증서를 확인한다. | [사용자 지정 도메인과 HTTPS](https://docs.github.com/en/pages/getting-started-with-github-pages/securing-your-github-pages-site-with-https)를 지원한다. |
| 보안 헤더 | 정적 응답에 [`_headers`](https://developers.cloudflare.com/pages/configuration/headers/)를 지원한다. | GitHub.com Pages에서 임의 응답 헤더 설정을 공식 문서로 확인하지 못했다. |
| 배포 권한 | [계정의 Cloudflare Pages Edit API 토큰](https://developers.cloudflare.com/pages/how-to/use-direct-upload-with-continuous-integration/)과 GitHub 보호된 환경의 승인자가 배포할 수 있다. 토큰은 계정의 Pages에 영향을 줄 수 있으므로 가능한 범위를 줄이고 별도 계정을 검토한다. | 저장소 게시 설정과 Pages 배포 권한을 가진 사람. 사용자 지정 Actions는 `pages: write`, `id-token: write`가 필요하다. |

**권고: Cloudflare Pages Free의 정적 전용 Direct Upload.** 응답 보안 헤더를 직접 설정할 수 있고 정적 요청·전송에 표시된 사용량 한도가 없다. 다만 어느 공급자도 정책 변경·서비스 제한·별도 유료 상품·도메인 비용까지 포함해 **모든 상황에서 청구서가 절대 없다고 보장할 수는 없다.** Free 플랜과 정적 파일만 사용하고 Workers/유료 기능·유료 실행기를 켜지 말며, 계정 결제 설정과 최신 약관을 사람이 확인한다. 한도 초과의 불명확한 결과는 실제 공급자에 확인한다. 공급자 정책이 바뀌면 게시를 중지할 수 있도록 한다.

## 정적 묶음

실제 nginx 설정은 `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin`, `try_files` 404와 디렉터리 index를 사용한다. 생성기는 동일한 캐시·헤더에 CSP와 프레임 금지를 더하고 `_redirects`에 `/start /start/ 308`을 넣는다. 최상위 `404.html`은 없는 경로가 로그인 화면으로 바뀌는 [Pages SPA fallback](https://developers.cloudflare.com/pages/configuration/serving-pages/)을 막는다. `_headers`와 `_redirects`는 정적 응답에만 적용되므로 Functions·`_worker.js`를 사용하지 않는다. `/`, `/start/`는 OAuth redirect 경로 그대로다. `/start` 308과 캐시·CSP·404는 **실제 Pages 배포 후 직접 확인해야 한다**. 설정된 CSP의 `connect-src https:`는 임의의 HTTPS 대상까지 허용하므로 엄격한 목적지 제한이 아니다. 화면·Google·서명 목록의 실제 연결을 검증한 뒤 별도 보안 검토로 좁힐 수 있다.

`scripts/prepare_momso_anywhere_static_site.py`는 기존 로그인·시작 생성기를 결합하고 화면 원본을 바꾸지 않는다. 시작 쪽은 기존 생산 공개키 검증을 재사용하며 개발 키로 생산 묶음을 만들 수 없다. 출력은 새 경로여야 하며 중간 실패 시 최종 출력은 생기지 않는다. Google Client ID와 공개키는 비밀이 아니지만 변경되면 보안에 영향을 주므로 검토·보호해야 한다. 서명 개인키는 이 workflow에 넣지 않는다.

`.github/workflows/anywhere-static-pages.yml`은 공개 저장소의 `main`에서 사람이 수동 실행하며 `contents: read`만 가진다. GitHub `anywhere-production` 환경에 승인자·배포 브랜치 제한을 설정하고, Cloudflare Pages Edit 토큰은 `CLOUDFLARE_PAGES_API_TOKEN` 환경 비밀로 둔다. 계정 ID, OAuth Client ID, 서명 목록 URL은 환경 변수 `CLOUDFLARE_ACCOUNT_ID`, `MOMSO_OAUTH_CLIENT_ID`, `MOMSO_RELEASE_MANIFEST_URL`로 넣는다. `MOMSO_TRUSTED_GATEWAY_IMAGE`에는 이전 입구 찾기에 필요한 검증된 옛 이미지 지문을 필요할 때 넣는다. 여러 옛 지문이 필요하면 현재 workflow의 단일 변수로 부족하므로 배포 전에 확장·시험해야 한다. Pages 프로젝트 이름은 `momso-anywhere`로 고정돼 있으며 미리 Direct Upload 프로젝트로 만들어야 한다. Direct Upload 프로젝트는 나중에 Git 연동 방식으로 직접 전환할 수 없다는 [제약](https://developers.cloudflare.com/pages/get-started/direct-upload/)이 있다.

**현재 준비가 더 필요한 입력:** 이 통합 나무에는 운영 공개키 `scripts/momso_release_public_key.pem`, 화면 담당의 `apps/alpha/desktop/login-page/`, `apps/alpha/desktop/src/start/`가 없다. 승인된 공개키와 화면 원본을 공개 저장소에 검토해 넣기 전에는 workflow가 의도적으로 멈춘다. 화면 파일을 여기서 지어내거나 예전 화면으로 대신하지 않았다. 공개키 및 화면의 정본과 빌드된 결과가 맞는지 본부가 확인해야 한다. 실제 workflow와 Cloudflare 응답은 미검증이다.

시작 모듈의 서명 목록 fetch는 `redirect: 'error'`와 CORS를 사용한다. 따라서 `MOMSO_RELEASE_MANIFEST_URL`과 `.sig`가 브라우저에서 인증 없이 직접 200을 반환하고 시작 사이트 출처의 CORS를 허용하는지 확인해야 한다. GitHub Release 다운로드 URL처럼 다른 주소로 리디렉션되는 경로는 현재 코드에서 동작한다고 가정할 수 없다. 서버·화면 묶음과 입구 이미지의 실제 공개 경로·서명 일치도 별도로 확인한다.

## 본부와 개발자가 할 일

1. Free 플랜과 정적 전용 사용 조건을 확인하고, 별도의 Pages Direct Upload 프로젝트 `momso-anywhere`를 만든다. 위에 적은 승인된 화면 파일·운영 공개키·GitHub 환경 설정과 최소 권한 토큰을 준비한다. 토큰 값은 공개 저장소에 적지 않는다.
2. 수동 workflow를 승인된 `main`에서 실행한다. 먼저 `pages.dev` 시험 출처로 `/`, `/start`, `/start/`, 없는 파일, `config.json`, 아이콘과 헤더를 확인한다. 시험 출처의 OAuth 출처·redirect를 별도로 등록하지 않았다면 실제 Google 로그인은 성공하지 않는다. 운영 설정의 `loginOrigin`은 운영 주소로 고정돼 있으므로 시험 주소에서 실제 왕복을 하려면 **별도의 시험 설정·OAuth 등록과 검토가 필요하다**. 화면이 깨지거나 CSP가 호출을 막으면 옮기지 않는다.
3. Pages에 사용자 지정 도메인 `anywhere.momso.yoga`를 추가하고 인증서 준비를 확인한다. 기존 DNS 공급자에서 `CNAME anywhere → <Pages가 제공한 프로젝트 주소>.pages.dev` 한 줄을 연결한다. 실제 대상 이름은 Pages가 발급한 것을 확인해 쓴다. DNS 전환 전에 기존 레코드와 TTL을 기록한다. 루트 `momso.yoga` 소개 사이트는 바꾸지 않는다.
4. Google OAuth에 승인된 JavaScript 출처 `https://anywhere.momso.yoga`, redirect URI `https://anywhere.momso.yoga/`와 `https://anywhere.momso.yoga/start/`를 등록한다. 실제 반영 지연과 로그인·시작의 왕복을 확인한다. 기존 Cloud Run 출처의 OAuth 등록은 기존 고객이 쓰는 동안 유지한다.
5. **기존 고객 입구의 `MOMSO_LOGIN_ORIGIN`이 옛 Cloud Run 주소면 DNS만 바꿔도 새 주소를 쓰지 않는다.** 각 입구의 안전한 갱신 경로로 로그인 출처를 새 주소로 바꾸고, 직접 주소·홈 화면에서 로그인과 기존 세션을 시험한다. 모든 기존 입구의 이전 여부와 고객 영향을 확인하기 전에는 옛 Cloud Run 사이트·OAuth 출처를 끄거나 지우지 않는다. 옛 주소를 그대로 유지하면 그 사이트에 대한 GCP 비용 가능성도 남는다.
6. 문제 시 기존 OAuth 출처·옛 Cloud Run을 유지하고 기록한 DNS를 되돌리며, 이전한 고객 입구의 로그인 출처도 이전 값으로 되돌린다. DNS 전파·기존 세션에 시간이 걸릴 수 있으므로 전환 전 실제 복구 시험이 필요하다. 완료와 롤백 가능성이 확인되고 본부가 승인한 뒤에만 옛 서비스를 중지한다.

## 충실도 확인

개발자가 정한 운영 주소와 소개 사이트 분리, 실제 배포·DNS·OAuth 변경 보류를 유지했다. 0원 보장은 공식 문서로 확인하지 못해 조건부로 남겼다. 원 요청과 뜻이 달라진 결정·수치: 없음.
