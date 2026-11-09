//! 응시 앱 진입점(Dioxus 데스크톱). 화면은 얇다 — 상태는 [`Session`], 부수 효과는 [`exam`]·[`LocalModel`].
//!
//! 첫 화면: 「내 시험」(로그인 → 배정된 시험·문항) 또는 「개인 연습」. 응시 전 점검(Docker·웹캠·동의) 뒤 응시 화면은
//! 문제 | Monaco 편집기(실행·예제 테스트는 이 PC Docker) | AI 패널 자리(#98)다. 시험 문항은 남은 시간을 보이고, 시간이
//! 다 되면 지금 코드를 바로 낸다(서버도 제한을 강제한다).
//!
//! 시험 문항은 과정 로그(#9)를 남긴다: 문항 시작·언어 고름·코드 스냅숏(5초)·로컬 실행·붙여넣기 막음·제출을 서명해
//! 앱 데이터 폴더에 쌓고, 5초마다 서버로 올린다(오프라인이면 쌓아 두었다가 재연결 뒤 — [`promptlab_app::proclog`]).
//!
//! 시험 문항을 푸는 동안은 응시 모드로 잠근다(#12, [`promptlab_app::lockdown`]): 창을 전체화면으로 두고 빠져나가면
//! 다시 돌린다. 막지 못하는 포커스 이탈(Alt+Tab 등)·전체화면 이탈은 과정 로그 사건으로 남기고, 밖에서 온 붙여넣기는
//! 막고 남긴다(앱 안 복사는 앱 안 버퍼만 — [`promptlab_app::editor::Clipboard`]).
//!
//! 설정: `PROMPTLAB_SERVER_URL`·`PROMPTLAB_ACCESS_TOKEN`(선택 — 없으면 브라우저로 sandcorp-auth 로그인)·`PROMPTLAB_PROBLEM_ID`,
//! 모델 파일은 `PROMPTLAB_MODEL_PATH`(없으면 기본 캐시 경로 — 시험 문항 시작 때 sha256을 신고한다).

// 릴리스 Windows 빌드는 콘솔 창 없이 뜬다(화면 없는 같은 흐름은 promptlab-app-cli).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod camera;
mod ui;

use dioxus::desktop::LogicalSize;
use promptlab_app::workspace;

fn main() {
    // 창 제목을 「PromptLab」으로(기본은 크레이트 이름). 기본 메뉴 막대(Window·Edit·Help)는 끈다 —
    // Fluent 화면 위에 남아 보이고, Windows·Linux에서 복사·붙여넣기 등은 웹뷰 기본 단축키가 처리한다.
    let window = dioxus::desktop::WindowBuilder::new()
        .with_title(ui::APP_NAME)
        // 창 최소·기본 크기(#117): 접힘 모드 막대 + 편집기 최소 + AI 최소가 들어가야 도구 막대가 안 가려진다.
        .with_inner_size(LogicalSize::new(
            f64::from(workspace::DEFAULT_WINDOW_WIDTH),
            f64::from(workspace::DEFAULT_WINDOW_HEIGHT),
        ))
        .with_min_inner_size(LogicalSize::new(
            f64::from(workspace::MIN_WINDOW_WIDTH),
            f64::from(workspace::MIN_WINDOW_HEIGHT),
        ));
    let mut cfg = dioxus::desktop::Config::new()
        .with_window(window)
        .with_menu(None)
        // 웹뷰는 앱 자산·dioxus 로컬 연결만(외부 요청 없음, Monaco 자산도 구워 내준다 — #97).
        .with_custom_head(ui::editor_bridge::csp_head());
    // 쓸 수 없는 웹뷰 데이터 폴더는 Edge가 오류 대화상자를 띄운다 — 쓰기를 시험해 폴더를 명시한다(#114, Windows).
    if let Some(dir) = cfg!(windows)
        .then(promptlab_app::webview_data::choose_here)
        .flatten()
    {
        cfg = cfg.with_data_directory(dir);
    }
    dioxus::LaunchBuilder::desktop()
        .with_cfg(cfg)
        .launch(ui::App);
}
