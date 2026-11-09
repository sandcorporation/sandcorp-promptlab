//! 응시 중 화면(#97). ≥1440px이면 문제 380 | 편집기 가변 | AI 패널 360, 미만이면 문제 칸이 48px 막대로 접힌다(눌러 펼친다).
//! 가운데 칸 = 도구 막대(언어·실행·예제 테스트·제출) + Monaco 편집기 + 아래 실행 칸(입력 한 번·결과 표).
//! 오른쪽 = AI 도우미 칸(#98, `ai_pane.rs`). 머리: 문항 탭 ✓ · 잠금 · 웹캠 · 남은 시간.
//! 키보드: Ctrl+Enter 예제 테스트 · Ctrl+Shift+Enter 실행(편집기 안은 Monaco 명령, 입력 칸은 여기서).
//! 개인 연습은 같은 화면에서 잠금·웹캠 표시만 뺀다.

use std::time::Instant;

use dioxus::html::{KeyboardEvent, Modifiers};
use dioxus::prelude::*;
use promptlab_app::localrun::{self, CaseLine, RunReport};
use promptlab_app::proclog;
use promptlab_app::workspace::{
    self, KEY_STEP, KeyInput, LAYOUT_FILE, LayoutPrefs, MIN_AI_WIDTH, MIN_PROBLEM_WIDTH,
    PaneLayout, PaneWidths, SHORTCUT_HINT, Shortcut, shortcut,
};
use promptlab_shared::api::ItemState;
use promptlab_shared::coding::{Language, PublicCodingProblem};
use promptlab_shared::log::LocalRunKind;

use super::editor_bridge::EditorHost;
use super::{RunView, Ui, mm_ss, submit};

const SPLITTER_JS: &str = include_str!("splitter.js");

/// 칸 폭 설정 파일 자리(앱 데이터 폴더).
fn layout_path() -> std::path::PathBuf {
    proclog::data_dir().join(LAYOUT_FILE)
}

/// 손잡이가 보낸 메시지 하나 → 새 폭. 끌기가 끝났으면(`end`) 두 번째 값이 `true`.
fn apply_splitter(
    msg: &serde_json::Value,
    window_width: u32,
    layout: PaneLayout,
    cur: PaneWidths,
) -> Option<(PaneWidths, bool)> {
    let h = msg.get("h")?.as_str()?;
    let total = msg
        .get("total")
        .and_then(serde_json::Value::as_u64)
        .map_or(window_width, |t| t as u32);
    let cur = workspace::fit(total, layout, cur);
    let next = if let Some(x) = msg.get("x").and_then(serde_json::Value::as_i64) {
        match h {
            "problem" => workspace::drag_problem(total, layout, cur, x),
            _ => workspace::drag_ai(total, layout, cur, i64::from(total) - x),
        }
    } else {
        let step = msg.get("step").and_then(serde_json::Value::as_i64)? * i64::from(KEY_STEP);
        match h {
            "problem" => workspace::drag_problem(total, layout, cur, i64::from(cur.problem) + step),
            // AI 칸은 오른쪽 끝에 붙어 있어 오른쪽 화살표 = 경계가 오른쪽으로 = AI가 좁아진다.
            _ => workspace::drag_ai(total, layout, cur, i64::from(cur.ai) - step),
        }
    };
    let end = msg
        .get("end")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    Some((next, end))
}

#[component]
pub fn ExamScreen() -> Element {
    let ui = use_context::<Ui>();
    let layout = ui.layout();
    let is_exam = ui.session.read().exam_item().is_some();
    // 기억한 칸 폭(#117): 처음 한 번 읽고, 끌기가 끝났을 때만 쓴다. 과정 로그·서버와 무관하다.
    let mut prefs = use_signal(|| LayoutPrefs::load(&layout_path()));
    {
        let ui = ui.clone();
        use_future(move || {
            let ui = ui.clone();
            async move {
                let mut eval = document::eval(SPLITTER_JS);
                while let Ok(msg) = eval.recv::<serde_json::Value>().await {
                    let layout = ui.layout();
                    let cur = prefs.read().widths(layout);
                    let Some((next, end)) = apply_splitter(&msg, (ui.width)(), layout, cur) else {
                        continue;
                    };
                    if next != cur {
                        prefs.write().set(layout, next);
                    }
                    if end {
                        let saved = prefs.read().clone();
                        if let Err(e) = saved.save(&layout_path()) {
                            eprintln!("칸 폭 설정을 쓰지 못했다: {e}");
                        }
                    }
                }
            }
        });
    }
    use_drop(|| {
        document::eval("if (window.__plSplitterOff) window.__plSplitterOff();");
    });
    let total = (ui.width)();
    let w = workspace::fit(total, layout, prefs.read().widths(layout));
    let three = layout == PaneLayout::Three;
    let style = format!("--w-problem: {}px; --w-ai: {}px;", w.problem, w.ai);
    rsx! {
        div { class: "exam", "data-layout": if three { "three" } else { "folded" },
            ExamHead { is_exam }
            div { class: "panes", style: "{style}",
                ProblemPane {}
                WorkPane {}
                super::ai_pane::AiPane {}
                if three {
                    div { class: "splitter", style: "left: {w.problem}px;", "data-h": "problem", role: "separator", tabindex: "0",
                        "aria-orientation": "vertical", "aria-label": "문제 칸 폭 조절",
                        "aria-valuemin": "{MIN_PROBLEM_WIDTH}", "aria-valuenow": "{w.problem}", "aria-valuemax": "{total}" }
                }
                div { class: "splitter", style: "right: {w.ai}px;", "data-h": "ai", role: "separator", tabindex: "0",
                    "aria-orientation": "vertical", "aria-label": "AI 칸 폭 조절",
                    "aria-valuemin": "{MIN_AI_WIDTH}", "aria-valuenow": "{w.ai}", "aria-valuemax": "{total}" }
            }
            if (ui.dialog)() {
                submit::SubmitDialog {}
            }
        }
    }
}

#[component]
fn ExamHead(is_exam: bool) -> Element {
    let ui = use_context::<Ui>();
    // 1초마다 다시 그린다(남은 시간·미전송 로그 수).
    let _ = (ui.tick)();
    let s = ui.session.read();
    let item = s.exam_item().cloned();
    let time_left = s.remaining_secs(Instant::now());
    let problem_id = s.problem().map(|p| p.id.clone()).unwrap_or_default();
    drop(s);
    let assignment = item.as_ref().and_then(|i| {
        ui.assignments
            .read()
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|list| list.iter().find(|a| a.id == i.assignment_id))
            .cloned()
    });
    let webcam_on = assignment
        .as_ref()
        .and_then(|a| a.webcam.as_ref())
        .is_none_or(|t| t.required)
        && is_exam;
    let title = assignment.as_ref().map_or_else(
        || format!("개인 연습 · {problem_id}"),
        |a| a.exam_title.clone(),
    );
    let locked = ui.lock.read().is_active();
    let unsent = ui.book.as_ref().map(|b| b.pending()).unwrap_or(0);
    rsx! {
        header { class: "exam-head",
            span { class: "brand",
                svg { "viewBox": "0 0 16 16", "aria-hidden": "true",
                    rect { class: "logo-bg", x: "1", y: "1", width: "14", height: "14", rx: "3.5" }
                    path { class: "logo-fg", d: "M5.5 11.5V4.5h3a2.2 2.2 0 0 1 0 4.4h-3" }
                }
            }
            h1 { "{title}" }
            nav { class: "htabs", "aria-label": "문항",
                if let Some(a) = &assignment {
                    for it in a.items.clone() {
                        {
                            let cur = item.as_ref().is_some_and(|i| i.position == it.position);
                            let done = matches!(it.state, ItemState::Submitted { .. });
                            rsx! {
                                span {
                                    class: if cur { "htab cur" } else { "htab" },
                                    title: "문항은 차례로 제출한다",
                                    "aria-current": if cur { "step" } else { "false" },
                                    if done { span { class: "check", "aria-label": "제출함", "✓" } }
                                    "문항 {it.position + 1}"
                                }
                            }
                        }
                    }
                }
            }
            div { class: "head-right",
                if locked {
                    span { class: "pill lock", title: "응시 중에는 창이 전체화면으로 잠긴다. 빠져나가면 다시 돌리고 과정 로그에 남는다",
                        svg { "viewBox": "0 0 16 16", "aria-hidden": "true", rect { x: "3", y: "7", width: "10", height: "7", rx: "1.5" } path { d: "M5.5 7V5a2.5 2.5 0 0 1 5 0v2" } }
                        span { class: "pill-label", "잠금" }
                    }
                }
                if webcam_on {
                    span { class: "pill", title: "웹캠으로 감독 중 — 분석은 이 PC 안에서만 한다",
                        svg { "viewBox": "0 0 16 16", "aria-hidden": "true", rect { x: "1.5", y: "4", width: "9", height: "8", rx: "1.5" } path { d: "M10.5 7l4-2v6l-4-2" } }
                        span { class: "pill-label", "웹캠" }
                    }
                }
                if (ui.watched)() {
                    span { class: "badge live", title: "감독관이 웹캠 영상을 실시간으로 보고 있다(녹화·음성 없음)", "● 감독관이 실시간으로 보는 중" }
                }
                if unsent > 0 {
                    span { class: "badge", title: "네트워크가 돌아오면 자동으로 올린다", "과정 로그 미전송 {unsent}건" }
                }
                if let Some(left) = time_left {
                    span { class: if left < 60 { "pill late" } else { "pill" },
                        strong { "{mm_ss(left)}" }
                        span { class: "pill-label", "남음" }
                    }
                }
            }
        }
    }
}

#[component]
fn ProblemPane() -> Element {
    let ui = use_context::<Ui>();
    let layout = ui.layout();
    let mut open = ui.problem_open;
    let (problem, position) = {
        let s = ui.session.read();
        (s.problem().cloned(), s.exam_item().map(|i| i.position))
    };
    let Some(p) = problem else { return rsx! {} };
    let folded = layout == PaneLayout::Folded;
    let shown = !folded || open();
    let heading = match position {
        Some(pos) => format!("문항 {}", pos + 1),
        None => "개인 연습".to_owned(),
    };
    rsx! {
        if folded {
            div { class: "rail",
                button { class: "rail-btn", "aria-expanded": "{open()}", onclick: move |_| open.set(!open()),
                    span { if open() { "문제 칸 접기" } else { "문제 보기" } }
                }
            }
        }
        if shown {
            section { class: if folded { "pane problem drawer" } else { "pane problem" }, "aria-label": "문제",
                div { class: "ph", span { class: "sub", "{heading}" } }
                div { class: "pbody",
                    ProblemBody { problem: p }
                }
            }
        }
    }
}

#[component]
fn ProblemBody(problem: PublicCodingProblem) -> Element {
    let p = problem;
    let params: Vec<String> = p
        .signature
        .params
        .iter()
        .map(|a| format!("{}: {}", a.name, a.ty))
        .collect();
    let signature = format!(
        "{}({}) -> {}",
        p.signature.name,
        params.join(", "),
        p.signature.returns
    );
    rsx! {
        h3 { "{p.title}" }
        pre { class: "desc", "{p.description}" }
        h3 { "함수 서명" }
        pre { class: "mono", "{signature}" }
        h3 { "예제 {p.examples.len()}개" }
        for (i, example) in p.examples.iter().enumerate() {
            div { class: "example",
                span { class: "sub", "예제 {i + 1} 인자" }
                pre { class: "mono", "{json(&example.args)}" }
                span { class: "sub", "기대 반환값" }
                pre { class: "mono", "{json(&example.expected)}" }
            }
        }
        p { class: "sub", "케이스당 {p.limits.case_time_ms}ms" }
    }
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_default()
}
/// 단축키 → 실행 종류.
fn run_kind(s: Shortcut) -> LocalRunKind {
    match s {
        Shortcut::RunExamples => LocalRunKind::Examples,
        Shortcut::RunInput => LocalRunKind::Run,
    }
}

/// 가운데 칸: 도구 막대 + 편집기 + 실행 칸.
#[component]
fn WorkPane() -> Element {
    let ui = use_context::<Ui>();
    let Some((language, allowed)) = ui
        .editor
        .read()
        .as_ref()
        .map(|e| (e.language(), e.allowed().to_vec()))
    else {
        return rsx! {};
    };
    let signature = ui.session.read().problem().map(|p| p.signature.clone());
    let placeholder = signature
        .as_ref()
        .map(|s| format!("인자 JSON 배열, 예: {}", localrun::input_example(s)))
        .unwrap_or_default();
    let running = matches!(*ui.run_view.read(), RunView::Running { .. });
    let busy = (ui.busy)();
    let mut run_input = ui.run_input;
    let mut dialog = ui.dialog;
    let (ui_lang, ui_run, ui_ex, ui_key) = (ui.clone(), ui.clone(), ui.clone(), ui.clone());
    rsx! {
        section { class: "pane work", "aria-label": "코드",
            div { class: "toolbar",
                label { class: "sr-only", r#for: "pl-lang", "언어" }
                select { id: "pl-lang", class: "lang",
                    disabled: allowed.len() < 2 || running,
                    onchange: move |e| {
                        let v = e.value();
                        if let Some(l) = Language::ALL.iter().copied().find(|l| l.as_str() == v) {
                            ui_lang.choose_language(l);
                        }
                    },
                    for l in allowed.iter().copied() {
                        option { value: "{l.as_str()}", selected: l == language, "{l.display_name()}" }
                    }
                }
                span { class: "sub hint", "{SHORTCUT_HINT}" }
                div { class: "tools",
                    button { disabled: running, title: "아래 입력 칸의 인자로 한 번 실행(Ctrl+Shift+Enter)",
                        onclick: move |_| ui_run.run(LocalRunKind::Run), "실행" }
                    button { disabled: running, title: "공개 예제로 시험(Ctrl+Enter)",
                        onclick: move |_| ui_ex.run(LocalRunKind::Examples), "예제 테스트" }
                    button { class: "accent", disabled: busy, onclick: move |_| dialog.set(true), "제출" }
                }
            }
            EditorHost {}
            div { class: "console", "aria-label": "실행",
                div { class: "run-input",
                    label { class: "sub", r#for: "pl-run-input", "실행 입력" }
                    textarea { id: "pl-run-input", rows: "2", spellcheck: "false",
                        placeholder: "{placeholder}",
                        initial_value: "{run_input}",
                        oninput: move |e| run_input.set(e.value()),
                        onkeydown: move |e: KeyboardEvent| {
                            let m = e.modifiers();
                            let key = e.key().to_string();
                            let input = KeyInput {
                                key: &key,
                                ctrl: m.contains(Modifiers::CONTROL),
                                shift: m.contains(Modifiers::SHIFT),
                                alt: m.contains(Modifiers::ALT),
                                meta: m.contains(Modifiers::META),
                            };
                            if let Some(s) = shortcut(input) {
                                e.prevent_default();
                                ui_key.run(run_kind(s));
                            }
                        },
                    }
                }
                RunResult {}
            }
        }
    }
}

/// 실행 칸의 결과.
#[component]
fn RunResult() -> Element {
    let ui = use_context::<Ui>();
    let view = ui.run_view.read().clone();
    match view {
        RunView::Idle => rsx! {
            p { class: "sub", "「실행」은 입력 칸의 인자로 한 번, 「예제 테스트」는 공개 예제 전부를 이 PC의 Docker에서 돌립니다. 숨은 테스트는 제출 뒤 서버가 돌립니다." }
        },
        RunView::Running { note, .. } => rsx! {
            p { class: "sub running", role: "status", "{note}" }
        },
        RunView::Done(Err(e)) => rsx! {
            div { class: "infobar error", role: "alert", "{e}" }
        },
        RunView::Done(Ok(r)) => rsx! { ReportView { report: r } },
    }
}

#[component]
fn ReportView(report: RunReport) -> Element {
    let r = report;
    let examples = r.kind == LocalRunKind::Examples;
    let all_ok = r.compile_error.is_none() && (!examples || r.passed() == r.total());
    rsx! {
        div { class: "run-head",
            strong { class: if all_ok { "pass" } else { "fail" }, "{r.summary()}" }
            if r.wall_killed {
                span { class: "badge warn", "전체 시간 상한으로 멈춤" }
            }
            span { class: "sub", "{r.language.display_name()}" }
        }
        if let Some(msg) = &r.compile_error {
            pre { class: "mono out", "{msg}" }
        }
        if !r.cases.is_empty() {
            div { class: "tscroll",
                table { class: "cases",
                    thead {
                        tr {
                            th { "#" }
                            th { "인자" }
                            if examples { th { "기대" } }
                            th { "반환" }
                            th { "상태" }
                            th { class: "num", "시간" }
                        }
                    }
                    tbody {
                        for (i, c) in r.cases.iter().enumerate() {
                            CaseRow { index: i + 1, case: c.clone(), examples }
                        }
                    }
                }
            }
        }
        if !r.stdout.is_empty() {
            span { class: "sub", "표준 출력" }
            pre { class: "mono out", "{r.stdout}" }
        }
        if !r.stderr.is_empty() {
            span { class: "sub", "표준 오류" }
            pre { class: "mono out", "{r.stderr}" }
        }
    }
}

#[component]
fn CaseRow(index: usize, case: CaseLine, examples: bool) -> Element {
    let c = case;
    let (mark, class) = match c.ok {
        Some(true) => ("✓", "mark ok"),
        Some(false) => ("×", "mark bad"),
        None => ("", "mark"),
    };
    let status = c
        .message
        .clone()
        .unwrap_or_else(|| localrun::status_text(c.status).to_owned());
    rsx! {
        tr {
            td { span { class: "{class}", "{mark}" } "{index}" }
            td { code { "{c.args}" } }
            if examples { td { code { "{c.expected.clone().unwrap_or_default()}" } } }
            td { code { "{c.got.clone().unwrap_or_default()}" } }
            td { "{status}" }
            td { class: "num", "{c.ms}ms" }
        }
    }
}
