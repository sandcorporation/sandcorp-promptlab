//! 응시 앱 화면 v2(#74, Windows 11 Fluent). 화면마다 모듈로 나눴다 — 상태·계산은 lib(`promptlab_app::workspace`·`session`),
//! 여기는 그리기와 이벤트 연결만 한다.
//!
//! - [`home`]: 내 시험·개인 연습 목록+상세 2칸, 설정
//! - [`check`]: 응시 전 점검·동의 별도 페이지
//! - [`exam_screen`]: 응시 중(문제 | Monaco 편집기·실행 | AI 패널 자리 3칸, 좁으면 문제 칸이 막대로 접힘)
//! - [`submit`]: 제출 ContentDialog
//! - [`result`]: 결과 3칸(채용 시험은 「응시 완료」만)
//! - [`background`]: 잠금·과정 로그·실시간 감독·웹캠 감시·남은 시간

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dioxus::core::spawn_forever;
use dioxus::desktop::window;
use dioxus::prelude::*;
use promptlab_app::client::{ApiClient, Config};
use promptlab_app::editor::{
    Clipboard, EditorState, PASTE_BLOCKED_NOTICE, PasteOutcome, SnapshotClock,
};
use promptlab_app::exam;
use promptlab_app::localrun::{DockerError, LocalDocker, RunReport, failed_event};
use promptlab_app::lockdown::Lockdown;
use promptlab_app::model_pick::{ModelPick, ModelPrefs};
use promptlab_app::proclog::{self, LogBook, SharedLog};
use promptlab_app::session::{ExamItemRef, Phase, Session, SessionEvent};
use promptlab_app::webcam::Photo;
use promptlab_app::workspace::PaneLayout;
use promptlab_shared::api::AssignmentView;
use promptlab_shared::coding::{Language, PublicCodingProblem};
use promptlab_shared::log::{Attempt, LocalRunKind, LogEvent};
use promptlab_shared::model_choice::Machine;
use promptlab_shared::webcam::{SnapshotReason, WebcamTerms};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::camera::SharedAnalyzer;

mod ai_pane;
mod background;
mod check;
pub mod editor_bridge;
mod exam_screen;
mod home;
mod result;
mod submit;

pub const MAX_PENDING_SNAPSHOTS: usize = 20;

/// 올리지 못한 웹캠 스냅샷(다음 주기에 다시 올린다).
#[derive(Clone)]
pub struct PendingSnapshot {
    pub item: ExamItemRef,
    pub client_ref: String,
    pub reason: SnapshotReason,
    pub at_ms: i64,
    pub photo: Photo,
}

const FLUENT_CSS: &str = include_str!("../fluent.css");
const STYLE: &str = include_str!("../style.css");

/// 표시 이름(창 제목·머리띠).
pub const APP_NAME: &str = "PromptLab";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 고른 AI 모델 받기 상태(#131).
#[derive(Debug, Clone, PartialEq)]
pub enum FetchState {
    Idle,
    Running {
        model: &'static str,
        progress: Option<promptlab_app::model_pick::FetchProgress>,
    },
    Failed(String),
}

/// 이 PC의 사양·받아 둔 모델·설정 파일을 읽는다(백엔드를 올리고 장치를 본다 — 모델은 올리지 않는다).
pub fn detect_model_pick() -> ModelPick {
    ModelPick {
        machine: promptlab_inference::machine().unwrap_or(Machine {
            gpu: None,
            free_ram: 0,
        }),
        downloaded: promptlab_inference::downloaded_models(),
        prefs: ModelPrefs::load(&model_prefs_path()),
    }
}

/// 모델 크기 설정 파일(앱 데이터 폴더).
pub fn model_prefs_path() -> std::path::PathBuf {
    promptlab_app::proclog::data_dir().join(promptlab_app::model_pick::MODEL_PREFS_FILE)
}

/// 창 안쪽 폭(논리 px).
pub fn window_width() -> u32 {
    let w = window();
    let logical = f64::from(w.inner_size().width) / w.scale_factor().max(0.5);
    logical.max(0.0) as u32
}

/// 시험 문항의 과정 로그에 사건을 남긴다(연습이면 로그가 없다). 디스크에 못 쓰면 표준 오류에만 알린다
/// — 로그 때문에 응시를 막지 않는다(못 올라간 로그는 다시보기에 신호로 뜬다).
pub fn note(plog: Signal<Option<SharedLog>>, event: LogEvent) {
    if let Some(log) = plog.read().as_ref()
        && let Err(e) = log.lock().expect("과정 로그 잠금").record_now(event)
    {
        eprintln!("과정 로그를 쓰지 못했다: {e}");
    }
}

pub fn mm_ss(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// 왼쪽 NavigationView에서 고른 화면.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Exams,
    Practice,
    Settings,
}

/// 응시를 시작할 곳.
#[derive(Clone, PartialEq)]
pub enum Target {
    /// 배정된 시험 문항. 웹캠 동의·기준 사진이 필요하면 `terms`.
    Exam {
        item: ExamItemRef,
        title: String,
        terms: Option<WebcamTerms>,
        /// 시험의 허용 언어(응시 전 점검이 이 언어 이미지를 받는다).
        languages: Vec<Language>,
    },
    /// 개인 연습 문제.
    Practice { problem: PublicCodingProblem },
}

/// 시작 화면(`Phase::NotStarted`)에서 어느 페이지인가.
#[derive(Clone, PartialEq)]
pub enum Route {
    Home,
    /// 응시 전 점검 별도 페이지.
    Check(Target),
}

/// 로컬 실행 칸의 상태.
#[derive(Clone, PartialEq)]
pub enum RunView {
    Idle,
    /// 도는 중(이미지 받기 진행 등 한 줄).
    Running {
        kind: LocalRunKind,
        note: String,
    },
    Done(Result<RunReport, DockerError>),
}

/// 화면들이 같이 쓰는 상태(컨텍스트로 나눈다).
#[derive(Clone)]
pub struct Ui {
    pub api: ApiClient,
    pub book: Result<Arc<LogBook>, String>,
    pub analyzer: SharedAnalyzer,
    pub session: Signal<Session>,
    /// 제출을 보내는 중(시간 끝 자동 제출이 겹치지 않게).
    pub busy: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// 응시 중 편집기 내용(언어·언어별 초안). 응시 중이 아니면 `None`.
    pub editor: Signal<Option<EditorState>>,
    pub clock: Signal<SnapshotClock>,
    pub clip: Signal<Clipboard>,
    /// Monaco 다리로 보내는 말(편집기 칸이 떠 있을 때만).
    pub editor_tx: Signal<Option<UnboundedSender<Value>>>,
    /// 「실행」 입력 칸.
    pub run_input: Signal<String>,
    pub run_view: Signal<RunView>,
    /// 마지막 예제 테스트(제출 확인 상자).
    pub last_examples: Signal<Option<RunView>>,
    pub run_count: Signal<u32>,
    /// 잠깐 보이는 알림(막은 붙여넣기 등).
    pub toast: Signal<Option<String>>,
    pub assignments: Signal<Option<Result<Vec<AssignmentView>, String>>>,
    pub practice_list: Signal<Option<Result<Vec<PublicCodingProblem>, String>>>,
    pub route: Signal<Route>,
    pub tab: Signal<Tab>,
    /// 내 시험 목록에서 고른 줄.
    pub selected: Signal<usize>,
    /// 개인 연습 목록에서 고른 문제 id(비었으면 설정의 기본 문제).
    pub practice_pick: Signal<String>,
    pub plog: Signal<Option<SharedLog>>,
    pub lock: Signal<Lockdown>,
    pub watched: Signal<bool>,
    pub width: Signal<u32>,
    /// 접힌 문제 칸을 펼쳤나.
    pub problem_open: Signal<bool>,
    /// 제출 ContentDialog가 떠 있나.
    pub dialog: Signal<bool>,
    /// 1초마다 올려 남은 시간을 다시 그린다.
    pub tick: Signal<u64>,
    /// 로컬 AI 모델(앱 실행 동안 한 번 올린다, #98 — 크기를 바꾸면 내리고 다시 올린다, #131).
    pub model: Signal<ai_pane::ModelSlot>,
    /// 모델 크기 추천·고른 크기(#131).
    pub pick: Signal<ModelPick>,
    /// 고른 모델 받기.
    pub fetch: Signal<FetchState>,
    /// 문제마다 대화 하나(열쇠 = [`Ui::chat_key`]).
    pub chats: Signal<HashMap<String, promptlab_app::assistant::Chat>>,
    /// AI가 답하는 중.
    pub ai_busy: Signal<bool>,
    /// 답하는 차례의 취소 깃발(「멈추기」, #116).
    pub ai_cancel: Signal<Option<promptlab_app::local::Cancel>>,
}

impl Ui {
    /// 쓸 모델 파일 경로: `PROMPTLAB_MODEL_PATH`가 있으면 그것(개발), 아니면 고른(또는 추천) 모델의 캐시 경로.
    pub fn model_path(&self) -> std::path::PathBuf {
        std::env::var_os("PROMPTLAB_MODEL_PATH")
            .map(Into::into)
            .unwrap_or_else(|| promptlab_inference::model_path(self.pick.read().chosen().model))
    }

    /// 쓸 모델 파일이 받아져 있나(크기만 — sha256은 올릴 때·문항 시작 때 잰다).
    pub fn model_ready(&self) -> bool {
        let path = self.model_path();
        match promptlab_inference::model_for_path(&path) {
            Some(m) => std::fs::metadata(&path).is_ok_and(|md| md.len() == m.size),
            None => path.is_file(),
        }
    }

    /// 모델 크기를 고른다(`None` = 자동). 설정에 기억하고, 쓸 모델이 바뀌면 올린 모델을 내리며, 받지 않았으면 받기 시작.
    /// 응시 전 점검·설정에서만 부른다(문항을 푸는 동안에는 바꾸지 않는다).
    pub fn choose_model(&self, id: Option<&str>) {
        let before = self.pick.read().chosen().model.id;
        let mut pick = self.pick;
        {
            let mut p = pick.write();
            p.prefs.model = id.map(str::to_owned);
            p.downloaded = promptlab_inference::downloaded_models();
            if let Err(e) = p.prefs.save(&model_prefs_path()) {
                eprintln!("모델 설정을 쓰지 못했다: {e}");
            }
        }
        if pick.read().chosen().model.id != before {
            let mut slot = self.model;
            slot.set(ai_pane::ModelSlot::Idle);
        }
        if !self.model_ready() {
            self.start_fetch();
        }
    }

    /// 고른 모델을 받는다(이미 받는 중이면 그대로).
    pub fn start_fetch(&self) {
        let mut fetch = self.fetch;
        if matches!(*fetch.read(), FetchState::Running { .. }) {
            return;
        }
        let model = self.pick.read().chosen().model;
        fetch.set(FetchState::Running {
            model: model.id,
            progress: None,
        });
        let mut pick = self.pick;
        spawn_forever(async move {
            let http = reqwest::Client::new();
            let result = promptlab_app::model_pick::fetch(
                &http,
                model,
                &model.url(),
                &promptlab_inference::model_dir(),
                |p| {
                    fetch.set(FetchState::Running {
                        model: model.id,
                        progress: Some(p),
                    })
                },
            )
            .await;
            pick.write().downloaded = promptlab_inference::downloaded_models();
            fetch.set(match result {
                Ok(_) => FetchState::Idle,
                Err(e) => FetchState::Failed(e.to_string()),
            });
        });
    }

    pub fn layout(&self) -> PaneLayout {
        PaneLayout::for_width((self.width)())
    }

    pub fn load_assignments(&self) {
        let (api, mut assignments) = (self.api.clone(), self.assignments);
        spawn_forever(async move {
            assignments.set(Some(api.assignments().await.map_err(|e| e.to_string())));
        });
    }

    pub fn load_practice(&self) {
        let (api, mut list) = (self.api.clone(), self.practice_list);
        spawn_forever(async move {
            list.set(Some(
                api.practice_problems().await.map_err(|e| e.to_string()),
            ));
        });
    }

    /// 점검(연습이면 고른 문제를 받고, 시험 문항이면 서버에 문항 시작)을 돌려 응시 중까지 간다.
    /// 응시 전 점검 페이지에서 이미 동의했으므로 점검이 끝나면 바로 동의로 넘어간다.
    pub fn begin(&self, target: &Target) {
        let ui = self.clone();
        let target = target.clone();
        let mut session = self.session;
        let mut error = self.error;
        let mut plog = self.plog;
        let (mut editor, mut clock, mut clip) = (self.editor, self.clock, self.clip);
        let (mut run_view, mut last_examples, mut run_count, mut toast) = (
            self.run_view,
            self.last_examples,
            self.run_count,
            self.toast,
        );
        session.set(Session::new());
        error.set(None);
        plog.set(None);
        editor.set(None);
        clock.set(SnapshotClock::new());
        clip.set(Clipboard::new());
        run_view.set(RunView::Idle);
        last_examples.set(None);
        run_count.set(0);
        toast.set(None);
        let _ = session.write().apply(SessionEvent::Start);
        spawn_forever(async move {
            let event = match &target {
                Target::Exam { item, .. } => {
                    exam::check_item(&ui.api, ui.model_path(), item.clone()).await
                }
                Target::Practice { problem } => exam::check(&ui.api, &problem.id).await,
            };
            // 시험 문항이 시작됐으면 과정 로그를 연다(앱을 다시 켰으면 이어 쓴다).
            if let SessionEvent::ExamReady { item, .. } = &event {
                let attempt = Attempt {
                    assignment_id: item.assignment_id.clone(),
                    position: item.position,
                };
                match ui
                    .book
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|b| b.attempt(attempt).map_err(|e| e.to_string()))
                {
                    Ok(log) => {
                        plog.set(Some(log));
                        note(plog, LogEvent::Started);
                    }
                    Err(e) => eprintln!("과정 로그를 열지 못했다: {e}"),
                }
            }
            if let Err(e) = session.write().apply(event) {
                error.set(Some(e.to_string()));
            }
            // 점검 페이지의 동의로 응시를 시작한다 — 편집기를 그 언어의 틀로 연다.
            if matches!(session.read().phase(), Phase::AwaitingConsent) {
                let _ = session
                    .write()
                    .apply(SessionEvent::ConsentGiven { at: Instant::now() });
                let opened = {
                    let s = session.read();
                    s.problem()
                        .map(|p| EditorState::new(p, &s.allowed_languages()))
                };
                if let Some(state) = opened {
                    note(plog, state.opened());
                    editor.set(Some(state));
                }
            }
        });
    }

    /// 응시 중인가(편집기가 떠 있다).
    fn in_progress(&self) -> bool {
        matches!(self.session.read().phase(), Phase::InProgress)
    }

    /// 시험 문항이면 클립보드를 앱 안으로 가둔다(개인 연습은 막지 않는다).
    pub fn guarded(&self) -> bool {
        self.session.read().exam_item().is_some()
    }

    /// Monaco 다리로 말 하나.
    pub fn to_editor(&self, message: Value) {
        if let Some(tx) = self.editor_tx.read().as_ref() {
            let _ = tx.send(message);
        }
    }

    /// 코드가 바뀌었으면 스냅숏을 과정 로그에 남긴다(`force`가 아니면 5초 주기).
    pub fn snapshot(&self, force: bool) {
        if !self.in_progress() || self.plog.read().is_none() {
            return;
        }
        let code = match self.editor.read().as_ref() {
            Some(e) => e.code().to_owned(),
            None => return,
        };
        let mut clock = self.clock;
        let now = Instant::now();
        let event = if force {
            clock.write().force(now, &code)
        } else {
            clock.write().tick(now, &code)
        };
        if let Some(e) = event {
            note(self.plog, e);
        }
    }

    /// Monaco 다리가 보낸 말 하나를 처리한다.
    pub fn on_editor(&self, message: Value) {
        let kind = message["t"].as_str().unwrap_or_default();
        let text = |k: &str| message[k].as_str().unwrap_or_default().to_owned();
        let (mut editor, mut clip, mut toast, mut error) =
            (self.editor, self.clip, self.toast, self.error);
        match kind {
            "change" => {
                if let Some(e) = editor.write().as_mut() {
                    e.edit(text("code"));
                }
            }
            "key" => {
                if let Some(e) = editor.write().as_mut() {
                    e.edit(text("code"));
                }
                match text("name").as_str() {
                    "examples" => self.run(LocalRunKind::Examples),
                    "run" => self.run(LocalRunKind::Run),
                    _ => {}
                }
            }
            "copy" if self.guarded() => clip.write().copied(&text("text")),
            "paste" if self.guarded() => {
                let outcome = clip.read().paste(&text("ext"));
                if let Some(e) = outcome.event() {
                    note(self.plog, e);
                    toast.set(Some(PASTE_BLOCKED_NOTICE.to_owned()));
                }
                let target = text("target");
                if let (PasteOutcome::Insert(t), false) = (outcome, target.is_empty()) {
                    self.to_editor(json!({"t": "insert", "target": target, "text": t}));
                }
            }
            "decide" => {
                if let Some(id) = message["id"].as_u64() {
                    self.decide(id as u32, message["accept"].as_bool().unwrap_or(false));
                }
            }
            "error" => error.set(Some(text("message"))),
            _ => {}
        }
    }

    /// 언어를 바꾼다(그 언어의 초안 또는 틀로).
    pub fn choose_language(&self, language: Language) {
        let mut editor = self.editor;
        let chosen = editor.write().as_mut().map(|e| e.choose(language));
        match chosen {
            Some(Ok(Some(event))) => {
                note(self.plog, event);
                let code = editor
                    .read()
                    .as_ref()
                    .map(|e| e.code().to_owned())
                    .unwrap_or_default();
                self.to_editor(json!({"t": "set", "language": language.as_str(), "code": code}));
            }
            Some(Err(e)) => {
                let mut error = self.error;
                error.set(Some(e.to_string()));
            }
            _ => {}
        }
    }

    /// 로컬 실행(「실행」·「예제 테스트」). 이미지가 없으면 받고 돈다. 결과는 화면과 과정 로그(`LocalRun`)에.
    pub fn run(&self, kind: LocalRunKind) {
        let mut run_view = self.run_view;
        if matches!(*run_view.read(), RunView::Running { .. }) || !self.in_progress() {
            return;
        }
        let Some((language, code)) = self
            .editor
            .read()
            .as_ref()
            .map(|e| (e.language(), e.code().to_owned()))
        else {
            return;
        };
        let Some(problem) = self.session.read().problem().cloned() else {
            return;
        };
        // 실행한 코드가 로그에 있게 바로 전 스냅숏.
        self.snapshot(true);
        let input = self.run_input.read().clone();
        let (plog, mut last_examples, mut run_count, mut toast) =
            (self.plog, self.last_examples, self.run_count, self.toast);
        toast.set(None);
        run_view.set(RunView::Running {
            kind,
            note: "Docker에서 실행하는 중…".to_owned(),
        });
        spawn_forever(async move {
            let result = async {
                let docker = LocalDocker::detect()?;
                if !docker.has_image(language).await? {
                    run_view.set(RunView::Running {
                        kind,
                        note: format!("{} 실행 이미지를 받는 중…", language.display_name()),
                    });
                    docker
                        .pull(language, |p| {
                            run_view.set(RunView::Running {
                                kind,
                                note: format!(
                                    "{} 실행 이미지를 받는 중… {}",
                                    language.display_name(),
                                    p.text()
                                ),
                            })
                        })
                        .await?;
                    run_view.set(RunView::Running {
                        kind,
                        note: "Docker에서 실행하는 중…".to_owned(),
                    });
                }
                match kind {
                    LocalRunKind::Examples => docker.run_examples(&problem, language, &code).await,
                    LocalRunKind::Run => docker.run_input(&problem, language, &code, &input).await,
                }
            }
            .await;
            note(
                plog,
                match &result {
                    Ok(r) => r.event(),
                    Err(e) => failed_event(kind, e),
                },
            );
            run_count += 1;
            let view = RunView::Done(result);
            if kind == LocalRunKind::Examples {
                last_examples.set(Some(view.clone()));
            }
            run_view.set(view);
        });
    }

    /// 지금 편집기의 언어·코드를 내고, 채점(또는 응시 완료)이 끝날 때까지 확인한다.
    /// 시험이면 `exam::submit`이 과정 로그에 `Submitted`(언어·코드 sha256)를 남기고 로그를 올린 뒤 낸다(409면 다시).
    pub fn submit_now(&self) {
        let (api, mut session, mut error, plog, mut dialog, mut busy) = (
            self.api.clone(),
            self.session,
            self.error,
            self.plog,
            self.dialog,
            self.busy,
        );
        error.set(None);
        dialog.set(false);
        if *busy.read() {
            return;
        }
        let Some((language, code)) = self
            .editor
            .read()
            .as_ref()
            .map(|e| (e.language(), e.code().to_owned()))
        else {
            return;
        };
        self.snapshot(true);
        if let Err(e) = session.write().apply(SessionEvent::Submit {
            at: Instant::now(),
            language,
            code,
        }) {
            error.set(Some(e.to_string()));
            return;
        }
        busy.set(true);
        spawn_forever(async move {
            let snapshot = session.read().clone();
            let log = plog.read().clone();
            if let Some(event) = exam::submit(&api, &snapshot, log.as_ref()).await
                && let Err(e) = session.write().apply(event)
            {
                error.set(Some(e.to_string()));
            }
            busy.set(false);
            // 채점이 끝난 뒤에도 점수(대화 평가, #99)가 늦게 끝날 수 있다 — 끝나거나 15분이 지날 때까지 확인한다.
            let until = Instant::now() + Duration::from_secs(15 * 60);
            while promptlab_app::workspace::should_poll(session.read().phase())
                && Instant::now() < until
            {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let snapshot = session.read().clone();
                if let Some(event) = exam::poll(&api, &snapshot).await {
                    let _ = session.write().apply(event);
                }
            }
        });
    }

    /// 시작 화면으로 돌아간다.
    pub fn go_home(&self) {
        let (mut session, mut route, mut assignments, mut error) =
            (self.session, self.route, self.assignments, self.error);
        session.set(Session::new());
        route.set(Route::Home);
        assignments.set(None);
        error.set(None);
        let mut sel = self.selected;
        sel.set(0);
        self.load_assignments();
    }
}

pub fn masthead(meta: Element) -> Element {
    rsx! {
        header { class: "titlebar",
            span { class: "brand",
                svg { "viewBox": "0 0 16 16", "aria-hidden": "true",
                    rect { class: "logo-bg", x: "1", y: "1", width: "14", height: "14", rx: "3.5" }
                    path { class: "logo-fg", d: "M5.5 11.5V4.5h3a2.2 2.2 0 0 1 0 4.4h-3" }
                }
                "{APP_NAME}"
            }
            {meta}
        }
    }
}

#[component]
pub fn App() -> Element {
    let api = use_hook(|| {
        Config::from_env()
            .and_then(ApiClient::new)
            .map_err(|e| e.to_string())
    });
    let book = use_hook(|| {
        LogBook::open(&proclog::data_dir())
            .map(Arc::new)
            .map_err(|e| e.to_string())
    });
    let analyzer: SharedAnalyzer = use_hook(Default::default);
    let session = use_signal(Session::new);
    let busy = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let editor = use_signal(|| None::<EditorState>);
    let clock = use_signal(SnapshotClock::new);
    let clip = use_signal(Clipboard::new);
    let editor_tx = use_signal(|| None::<UnboundedSender<Value>>);
    let run_input = use_signal(String::new);
    let run_view = use_signal(|| RunView::Idle);
    let last_examples = use_signal(|| None::<RunView>);
    let run_count = use_signal(|| 0u32);
    let toast = use_signal(|| None::<String>);
    let assignments = use_signal(|| None::<Result<Vec<AssignmentView>, String>>);
    let practice_list = use_signal(|| None::<Result<Vec<PublicCodingProblem>, String>>);
    let route = use_signal(|| Route::Home);
    let tab = use_signal(|| Tab::Exams);
    let selected = use_signal(|| 0usize);
    let practice_pick = use_signal(String::new);
    let plog = use_signal(|| None::<SharedLog>);
    let lock = use_signal(Lockdown::new);
    let watched = use_signal(|| false);
    let width = use_signal(window_width);
    let problem_open = use_signal(|| false);
    let dialog = use_signal(|| false);
    let tick = use_signal(|| 0u64);

    let Ok(api) = api else {
        let message = api.err().unwrap_or_default();
        return rsx! {
            style { {FLUENT_CSS} }
            style { {STYLE} }
            div { class: "app",
                {masthead(rsx! {})}
                div { class: "shell no-nav",
                    main { class: "content",
                        div { class: "page",
                            h1 { class: "sr-only", "{APP_NAME}" }
                            div { class: "infobar error", role: "alert", "설정 오류: {message}" }
                        }
                    }
                }
            }
        };
    };

    let ui = Ui {
        api,
        book,
        analyzer,
        session,
        busy,
        error,
        editor,
        clock,
        clip,
        editor_tx,
        run_input,
        run_view,
        last_examples,
        run_count,
        toast,
        assignments,
        practice_list,
        route,
        tab,
        selected,
        practice_pick,
        plog,
        lock,
        watched,
        width,
        problem_open,
        dialog,
        tick,
        model: use_signal(|| ai_pane::ModelSlot::Idle),
        pick: use_signal(detect_model_pick),
        fetch: use_signal(|| FetchState::Idle),
        chats: use_signal(HashMap::new),
        ai_busy: use_signal(|| false),
        ai_cancel: use_signal(|| None),
    };
    use_context_provider(|| ui.clone());
    background::use_background(&ui);
    // 앱에 구운 Monaco 자산을 웹뷰 경로 /monaco/…로 내준다(외부 요청 없음).
    editor_bridge::use_monaco_assets();

    let phase = ui.session.read().phase().clone();
    let _ = (ui.tick)();
    let screen = match &phase {
        Phase::NotStarted | Phase::Checking | Phase::AwaitingConsent => {
            return rsx! {
                style { {FLUENT_CSS} }
                style { {STYLE} }
                home::Shell {}
            };
        }
        Phase::InProgress => rsx! { exam_screen::ExamScreen {} },
        Phase::Submitting { .. } | Phase::Grading { .. } => {
            rsx! { result::Waiting {} }
        }
        Phase::Graded { .. } => rsx! { result::ResultScreen {} },
        Phase::GradingFailed { .. } | Phase::Completed => rsx! { result::Done {} },
    };
    rsx! {
        style { {FLUENT_CSS} }
        style { {STYLE} }
        {screen}
    }
}
