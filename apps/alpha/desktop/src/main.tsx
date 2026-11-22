import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ServerGate } from "./components/ServerGate";
import { isNetworkRuntime } from "./host";
import { isOverlayWebview } from "./panel-overlay";
import { PanelOverlayRoot } from "./components/PanelOverlayRoot";
import { StartScreen } from "./start/StartScreen";

/** 네트워크 길(momso server, 같은 출처)이면 문(서버 확인·로그인이 풀림·닿지 못함) 안에서 화면을 연다. 데스크톱·미리 보기는 예전 그대로 App. */
// 휴대폰 홈 화면 앱(네트워크 길): 화면 높이를 키보드를 뺀 보이는 높이로(아이폰은 키보드가 올라와도 화면 크기를 줄이지 않는다).
if (isNetworkRuntime()) {
  document.documentElement.classList.add("is-network-app");
  const fit = () => {
    const height = window.visualViewport?.height ?? window.innerHeight;
    document.documentElement.style.setProperty("--app-height", `${Math.round(height)}px`);
    // 아이폰이 입력 칸을 보이게 하려고 문서를 밀어 올린 것을 되돌린다(화면은 이미 보이는 높이에 맞춤).
    if (window.scrollY) window.scrollTo(0, 0);
  };
  fit();
  window.visualViewport?.addEventListener("resize", fit);
  window.visualViewport?.addEventListener("scroll", fit);
  window.addEventListener("resize", fit);
}

/** 휴대폰에서 시작하기 페이지(줄2 S26): VITE_MOMSO_START=1 로 빌드한 화면은 공개 시작 페이지로만 쓴다(채팅 앱을 열지 않는다). */
const startPage = import.meta.env.VITE_MOMSO_START === "1";
if (startPage) document.documentElement.classList.add("is-start-page");

function Root() {
  if (startPage) return <StartScreen />;
  // 패널 위에 띄운 앱 창(웹뷰 panel-overlay, S13)이면 메인 화면 전체를 그리지 않고 그 창의 내용만.
  if (isOverlayWebview()) return <PanelOverlayRoot />;
  return isNetworkRuntime() ? <ServerGate><App /></ServerGate> : <App />;
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
