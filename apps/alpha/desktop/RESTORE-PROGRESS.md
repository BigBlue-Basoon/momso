# 채팅 사본 복원 진행 계약

GitHub에서 받은 채팅 사본과 앱 상태 복원에 `chat-archive-restore-progress` 이벤트를 사용한다. 값은 `{ completed, total, state, available }`이다. `state`는 `restoring`, `complete`, `failed` 중 하나다. `completed`는 검사하고 데이터베이스에 넣어 본 사본 수이며 `total`은 전체 사본 수다. 진행 중인 변경은 하나의 트랜잭션에 묶이므로 `available`은 `false`이고, 복원이 끝나야 새로 받은 채팅방을 사용할 수 있다. 실패하면 트랜잭션을 되돌리고 `failed`와 `available: false`를 보낸다. 앱 상태 복원에서는 추가 무결성 검사까지 성공한 후 `complete`를 보낸다. 이미 로컬에 있던 채팅방은 원래대로 유지한다.

화면은 진행 중에 채팅방을 조작하는 요청을 보내지 않고 완료 또는 실패를 기다린다. 이 값은 백엔드 준비나 Git 다운로드 전체의 진행률이 아니라 채팅 사본 복원 단계의 진행률이다.
