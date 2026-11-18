---
name: conversation-import
title: 다른 AI의 채팅방 가져오기
description: 다른 AI에서 내보낸 파일을 선택하면 왼쪽 목록에 채팅방으로 생깁니다.
---

어느 서비스에서 내보냈는지 묻고 `momso_import_conversations`에 `chatgpt`, `claude`, `gemini` 중 하나만 전달한다. 몸소가 파일 선택과 미리 보기, 가져오기 확인을 직접 보여 준다. 파일 경로나 대화 내용을 묻거나 읽거나 채팅에 붙이지 않는다. 결과로 받은 개수와 상태, `groupingNotice` 또는 `guidance`가 있으면 그 내용도 알려 준다. 제미나이는 “구글 테이크아웃에서 '내 활동'의 형식을 JSON으로 골라 내보내 주세요.”라고 안내하고 내 활동의 Gemini 앱을 선택하도록 돕는다. 영어로 내보낸 일부 HTML도 읽지만 한국어 HTML 형식은 아직 확인되지 않았다. 활동 기록은 원래 채팅방의 경계를 보장하지 않고, 내보낸 답변이 빠지거나 잘릴 수 있다.
