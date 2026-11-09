//! 문제 편집 화면(#75 B → 코딩 문제 #96): 코딩 문제 원본을 칸으로 나눠 고친다 — 제목·설명, **서명 편집기**(함수 이름·
//! 인자 줄 더하기/빼기·타입 고르기·반환 타입), 예제·히든 테스트 JSON(서명에 맞는지 shared 검증으로 바로 보인다),
//! 모범 답안(언어 + 텍스트 칸), **4개 언어 틀 미리보기**(shared 생성기). 오른쪽 점검 패널에서 발행 전 점검(모범 답안이
//! 예제 + 히든 테스트를 모두 통과 + 4개 언어 틀 컴파일)을 돌린다. 순서는 저장 → 점검 → 발행. 입력 상태·검증은
//! [`crate::problem_draft`].
//!
//! - 점검은 **저장된 문제의 지금 원본**으로만 돈다(서버가 그렇게 한다). 고치고 저장하지 않았거나(`dirty`) 고친 뒤
//!   점검이 낡았으면(`current: false`) 「재검사 필요」다. 발행은 지금 원본의 마지막 점검이 모두 통과일 때만 켜진다.
//! - 점검 결과에는 통과 수·언어별 틀 컴파일 결과·발행 가능·짧은 이유만 있다(케이스 입력·출력·컴파일 메시지 없음).
//! - 운영자 문항(`EditTarget::Operator`)은 `/operator/problem-checks`·`/operator/problems/{id}/publish`를 쓴다.

use dioxus::prelude::*;
use promptlab_shared::authoring::{
    OrgProblem, PreviewRequest, PreviewState, PreviewView, Publication,
};

use promptlab_shared::coding::{Language, Param, ValueType};

use crate::problem_draft::{Draft, parse_type};
use crate::problems::{EditTarget, blank_source};
use crate::shell::PageHead;
use crate::{api, notice};

/// 점검 폴링 간격.
const POLL_MS: u32 = 2000;

fn running(view: &PreviewView) -> bool {
    matches!(view.state, PreviewState::Queued | PreviewState::Running)
}

/// 점검을 큐에 넣고 끝날 때까지 2초마다 읽는다(상태가 바뀔 때마다 `preview`에 담는다).
async fn run_check(
    target: &EditTarget,
    problem_id: String,
    mut preview: Signal<Option<PreviewView>>,
) -> Result<(), String> {
    let request = PreviewRequest { problem_id };
    let mut view = match target {
        EditTarget::Org(org) => api::start_preview(org, &request).await,
        EditTarget::Operator { .. } => api::start_operator_check(&request).await,
    }
    .map_err(|e| e.to_string())?;
    preview.set(Some(view.clone()));
    while running(&view) {
        api::sleep_ms(POLL_MS).await;
        view = match target {
            EditTarget::Org(org) => api::preview(org, &view.id).await,
            EditTarget::Operator { .. } => api::operator_check(&view.id).await,
        }
        .map_err(|e| e.to_string())?;
        preview.set(Some(view.clone()));
    }
    Ok(())
}

#[component]
pub fn ProblemEditor(
    target: EditTarget,
    initial: Option<OrgProblem>,
    on_saved: EventHandler<OrgProblem>,
    on_close: EventHandler<()>,
) -> Element {
    let mut problem_id = use_signal(|| initial.as_ref().map(|p| p.id.clone()));
    let start = Draft::from_source(
        &initial
            .as_ref()
            .map_or_else(blank_source, |p| p.source.clone()),
    );
    let mut draft = use_signal({
        let d = start.clone();
        move || d.clone()
    });
    let mut saved_draft = use_signal(move || start.clone());
    let mut publication = use_signal(|| initial.as_ref().map(|p| p.publication.clone()));
    let mut preview = use_signal(|| {
        initial
            .as_ref()
            .and_then(|p| p.publication.last_check.clone())
    });
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);
    let mut checking = use_signal(|| false);
    let mut tpl_lang = use_signal(|| Language::Python);

    let dirty = *draft.read() != *saved_draft.read();
    let issues = draft.read().issues();

    let save = {
        let target = target.clone();
        move |_| {
            let current = draft.read().clone();
            let source = match current.to_source() {
                Ok(s) => s,
                Err(e) => return error.set(Some(e.join(" / "))),
            };
            let target = target.clone();
            let id = problem_id.read().clone();
            busy.set(true);
            spawn(async move {
                let saved = match &target {
                    EditTarget::Org(org_id) => {
                        api::save_org_problem(org_id, id.as_deref(), &source).await
                    }
                    EditTarget::Operator { practice } => {
                        api::save_operator_problem(id.as_deref(), *practice, &source)
                            .await
                            .map(|b| OrgProblem {
                                id: b.id,
                                source: b.source,
                                updated_at: b.updated_at,
                                publication: b.publication,
                            })
                    }
                };
                match saved {
                    Ok(saved) => {
                        error.set(None);
                        saved_draft.set(current);
                        problem_id.set(Some(saved.id.clone()));
                        publication.set(Some(saved.publication.clone()));
                        preview.set(saved.publication.last_check.clone());
                        on_saved.call(saved);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };

    let start_check = {
        let target = target.clone();
        move |_| {
            let Some(id) = problem_id.read().clone() else {
                return error.set(Some("먼저 저장해야 점검을 돌릴 수 있다".into()));
            };
            let target = target.clone();
            error.set(None);
            checking.set(true);
            spawn(async move {
                if let Err(e) = run_check(&target, id, preview).await {
                    error.set(Some(e));
                }
                checking.set(false);
            });
        }
    };

    let publish = {
        let target = target.clone();
        move |_| {
            let Some(id) = problem_id.read().clone() else {
                return;
            };
            let target = target.clone();
            busy.set(true);
            spawn(async move {
                let done: Result<Publication, _> = match &target {
                    EditTarget::Org(org) => api::publish_org_problem(org, &id).await,
                    EditTarget::Operator { .. } => api::publish_operator_problem(&id).await,
                };
                match done {
                    Ok(p) => {
                        error.set(None);
                        publication.set(Some(p.clone()));
                        // 목록을 새로 읽게 알린다(원본은 그대로).
                        if let Ok(source) = saved_draft.read().to_source() {
                            on_saved.call(OrgProblem {
                                id,
                                source,
                                updated_at: 0,
                                publication: p,
                            });
                        }
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };

    let view = preview.read().clone();
    let published = publication.read().as_ref().is_some_and(|p| p.published);
    let stale = view.as_ref().is_some_and(|v| !v.current);
    let usable = view
        .as_ref()
        .filter(|v| v.current && !dirty && v.state == PreviewState::Done)
        .cloned();
    let can_publish = !dirty
        && !published
        && problem_id.read().is_some()
        && usable.as_ref().is_some_and(|v| v.publishable);
    let is_running = view.as_ref().is_some_and(running) || *checking.read();

    let title = if problem_id.read().is_none() {
        "새 문제".to_owned()
    } else {
        crate::problems::short(&saved_draft.read().title, 40)
    };
    let badge = if dirty {
        rsx! { span { class: "sc-badge sc-badge--warning", "저장 안 됨" } }
    } else if stale {
        rsx! { span { class: "sc-badge sc-badge--warning", "재검사 필요" } }
    } else if published {
        rsx! { span { class: "sc-badge sc-badge--success", "발행됨" } }
    } else {
        rsx! { span { class: "sc-badge sc-badge--neutral", "초안" } }
    };
    let publish_hint = if published {
        "이미 발행했습니다"
    } else if dirty || problem_id.read().is_none() {
        "저장한 뒤 점검을 돌려야 발행할 수 있습니다"
    } else {
        "점검을 모두 통과해야 발행할 수 있습니다"
    };
    let sub = match &target {
        EditTarget::Operator { practice: true } => {
            Some("연습 문항(연습에만 쓴다 — 만든 뒤 바꿀 수 없다)".to_owned())
        }
        EditTarget::Operator { practice: false } => {
            Some("시험 문항(시험에만 쓴다 — 만든 뒤 바꿀 수 없다)".to_owned())
        }
        EditTarget::Org(_) => None,
    };

    let d = draft.read().clone();
    let (example_count, hidden_count) = (d.example_count(), d.hidden_count());
    let count_text =
        |n: Option<usize>| n.map_or_else(|| "읽지 못함".to_owned(), |n| format!("{n}개"));
    let example_text = count_text(example_count);
    let hidden_text = count_text(hidden_count);
    let template = d.template(*tpl_lang.read());
    let param_count = d.params.len();

    rsx! {
        PageHead {
            title,
            sub,
            badges: badge,
            actions: rsx! {
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", r#type: "button", onclick: move |_| on_close.call(()), "목록으로" }
                button {
                    class: "sc-btn sc-btn--secondary sc-btn--sm",
                    r#type: "button",
                    disabled: *busy.read() || !dirty || !issues.is_empty(),
                    onclick: save,
                    "저장"
                }
                button {
                    class: "sc-btn sc-btn--accent sc-btn--sm",
                    r#type: "button",
                    disabled: !can_publish || *busy.read(),
                    title: "{publish_hint}",
                    onclick: publish,
                    "발행"
                }
            },
        }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        div { class: "sc-split",
            div { class: "editor-main",
                if !issues.is_empty() {
                    div { class: "sc-alert sc-alert--warning", role: "status",
                        b { "저장하기 전에 고칠 곳 {issues.len()}개" }
                        ul { class: "issue-list",
                            for (i, issue) in issues.iter().enumerate() {
                                li { key: "{i}", "{issue}" }
                            }
                        }
                    }
                }
                section { class: "sc-panel", "aria-label": "기본 정보",
                    div { class: "sc-panel-section",
                        h2 { class: "sc-panel-title", "기본 정보" }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-title", "제목" }
                            input {
                                id: "p-title",
                                class: "sc-input",
                                value: "{d.title}",
                                oninput: move |e| draft.write().title = e.value(),
                            }
                        }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-desc", "설명(마크다운, 응시자에게 보인다)" }
                            textarea {
                                id: "p-desc",
                                class: "sc-input",
                                rows: 8,
                                value: "{d.description}",
                                oninput: move |e| draft.write().description = e.value(),
                            }
                        }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-time", "케이스당 시간(밀리초, 500~10000)" }
                            input {
                                id: "p-time",
                                class: "sc-input sc-num",
                                inputmode: "numeric",
                                value: "{d.case_time_ms}",
                                oninput: move |e| draft.write().case_time_ms = e.value(),
                            }
                        }
                    }
                }
                section { class: "sc-panel", "aria-label": "함수 서명",
                    div { class: "sc-panel-section",
                        h2 { class: "sc-panel-title", "함수 서명" }
                        p { class: "muted small", "함수 이름·인자·반환 타입에서 4개 언어의 빈 틀과 실행 하네스를 자동으로 만듭니다. 타입은 int(±2^53−1 안)·double·bool·string과 그 1차원 배열, int[][]·string[][]만 씁니다." }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-fn", "함수 이름" }
                            input {
                                id: "p-fn",
                                class: "sc-input case-code",
                                spellcheck: false,
                                value: "{d.fn_name}",
                                oninput: move |e| draft.write().fn_name = e.value(),
                            }
                        }
                        div { class: "sc-table-scroll",
                            table { class: "sc-table",
                                thead {
                                    tr {
                                        th { scope: "col", "인자 이름" }
                                        th { scope: "col", "타입" }
                                        th { scope: "col", class: "sc-num", span { class: "sc-visually-hidden", "빼기" } }
                                    }
                                }
                                tbody {
                                    for (i, p) in d.params.iter().cloned().enumerate() {
                                        tr { key: "param{i}",
                                            td {
                                                input {
                                                    class: "sc-input case-code",
                                                    "aria-label": "인자 {i + 1} 이름",
                                                    spellcheck: false,
                                                    value: "{p.name}",
                                                    oninput: move |e| draft.write().params[i].name = e.value(),
                                                }
                                            }
                                            td {
                                                select {
                                                    class: "sc-select",
                                                    "aria-label": "인자 {i + 1} 타입",
                                                    onchange: move |e| {
                                                        if let Some(t) = parse_type(&e.value()) {
                                                            draft.write().params[i].ty = t;
                                                        }
                                                    },
                                                    for t in ValueType::ALL {
                                                        option { value: t.as_str(), selected: t == p.ty, "{t.as_str()}" }
                                                    }
                                                }
                                            }
                                            td { class: "sc-num",
                                                button {
                                                    class: "sc-btn sc-btn--secondary sc-btn--sm",
                                                    r#type: "button",
                                                    "aria-label": "인자 {i + 1} 빼기",
                                                    onclick: move |_| { draft.write().params.remove(i); },
                                                    "빼기"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "sc-toolbar",
                            span { class: "muted small", "인자 {param_count} / {promptlab_shared::coding::MAX_PARAMS}" }
                            button {
                                class: "sc-btn sc-btn--secondary sc-btn--sm",
                                r#type: "button",
                                disabled: param_count >= promptlab_shared::coding::MAX_PARAMS,
                                onclick: move |_| {
                                    let n = draft.read().params.len() + 1;
                                    draft.write().params.push(Param { name: format!("arg{n}"), ty: ValueType::Int });
                                },
                                "＋ 인자 더하기"
                            }
                        }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-ret", "반환 타입" }
                            select {
                                id: "p-ret",
                                class: "sc-select",
                                onchange: move |e| {
                                    if let Some(t) = parse_type(&e.value()) {
                                        draft.write().returns = t;
                                    }
                                },
                                for t in ValueType::ALL {
                                    option { value: t.as_str(), selected: t == d.returns, "{t.as_str()}" }
                                }
                            }
                        }
                    }
                }
                section { class: "sc-panel", "aria-label": "테스트 케이스",
                    div { class: "sc-panel-section",
                        h2 { class: "sc-panel-title", "테스트 케이스" }
                        p { class: "muted small", "케이스는 {{\"args\": [인자…], \"expected\": 기대값}}의 JSON 배열입니다. 서명에 맞는지 입력하는 즉시 검사합니다. double은 상대·절대 1e-6 안이면 같습니다." }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-examples", "예제 케이스 — 응시자에게 보입니다({example_text}, 1~10개)" }
                            textarea {
                                id: "p-examples",
                                class: "sc-input case-code",
                                rows: 8,
                                spellcheck: false,
                                value: "{d.examples}",
                                oninput: move |e| draft.write().examples = e.value(),
                            }
                        }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-hidden", "히든 케이스 — 응시자에게 나가지 않습니다({hidden_text}, 5~50개)" }
                            textarea {
                                id: "p-hidden",
                                class: "sc-input case-code",
                                rows: 12,
                                spellcheck: false,
                                value: "{d.hidden}",
                                oninput: move |e| draft.write().hidden = e.value(),
                            }
                        }
                    }
                }
                section { class: "sc-panel", "aria-label": "모범 답안",
                    div { class: "sc-panel-section",
                        h2 { class: "sc-panel-title", "모범 답안" }
                        p { class: "muted small", "언어를 하나 골라 서명대로 함수를 씁니다. 점검이 예제와 히든 케이스를 모두 통과하는지 판정기로 돌려 봅니다. 응시자에게 나가지 않습니다." }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-ref-lang", "언어" }
                            select {
                                id: "p-ref-lang",
                                class: "sc-select",
                                onchange: move |e| {
                                    if let Some(l) = Language::parse(&e.value()) {
                                        draft.write().reference_language = l;
                                    }
                                },
                                for l in Language::ALL {
                                    option { value: l.as_str(), selected: l == d.reference_language, "{l.display_name()}" }
                                }
                            }
                        }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-ref", "코드" }
                            textarea {
                                id: "p-ref",
                                class: "sc-input case-code",
                                rows: 14,
                                spellcheck: false,
                                value: "{d.reference_solution}",
                                oninput: move |e| draft.write().reference_solution = e.value(),
                            }
                        }
                    }
                }
                section { class: "sc-panel", "aria-label": "언어별 틀 미리보기",
                    div { class: "sc-panel-section",
                        h2 { class: "sc-panel-title", "언어별 틀 미리보기" }
                        p { class: "muted small", "응시자가 처음 보는 빈 함수입니다. 서명에서 자동으로 만들며 점검이 4개 언어 모두 컴파일되는지 확인합니다." }
                        div { class: "sc-field",
                            label { class: "sc-field-label", r#for: "p-tpl-lang", "언어" }
                            select {
                                id: "p-tpl-lang",
                                class: "sc-select",
                                onchange: move |e| {
                                    if let Some(l) = Language::parse(&e.value()) {
                                        tpl_lang.set(l);
                                    }
                                },
                                for l in Language::ALL {
                                    option { value: l.as_str(), selected: l == *tpl_lang.read(), "{l.display_name()}" }
                                }
                            }
                        }
                        match template {
                            Some(code) => rsx! { pre { class: "case-code tpl-preview", tabindex: "0", "{code}" } },
                            None => rsx! { p { class: "muted small", "서명을 고치면 틀이 나옵니다(함수 이름·인자 이름을 확인하세요)." } },
                        }
                    }
                }
            }
            aside { class: "sc-split-rail", "aria-label": "발행 전 점검",
                CheckPanel {
                    view,
                    dirty,
                    saved: problem_id.read().is_some(),
                    is_running,
                    on_run: start_check,
                }
            }
        }
    }
}

#[component]
fn CheckPanel(
    view: Option<PreviewView>,
    dirty: bool,
    saved: bool,
    is_running: bool,
    on_run: EventHandler<MouseEvent>,
) -> Element {
    let run_label = if is_running {
        "점검하는 중…"
    } else if view.is_some() {
        "다시 돌리기"
    } else {
        "점검 돌리기"
    };
    let run_disabled = is_running || dirty || !saved;
    let run_title = if dirty || !saved {
        "먼저 저장해야 점검할 수 있습니다(점검은 저장된 원본으로 돕니다)"
    } else {
        ""
    };

    let body = match &view {
        None => rsx! {
            div { class: "sc-panel-section",
                p { class: "muted small", "아직 점검하지 않았습니다. 저장한 뒤 「점검 돌리기」를 누르세요. 모범 답안을 판정기로 예제와 히든 테스트에 돌려 모두 통과하는지, 4개 언어의 빈 틀이 모두 컴파일되는지 봅니다." }
            }
        },
        Some(v) => {
            let state = match v.state {
                PreviewState::Queued => {
                    let wait = match v.queue_ahead {
                        Some(n) if n > 0 => format!("기다리는 중 — 앞에 {n}건(응시자 채점이 먼저)"),
                        _ => "곧 시작합니다".to_owned(),
                    };
                    rsx! { span { class: "sc-badge sc-badge--info", "{wait}" } }
                }
                PreviewState::Running => {
                    rsx! { span { class: "sc-badge sc-badge--info", "점검하는 중" } }
                }
                PreviewState::Failed => {
                    let why = v.failure_reason.clone().unwrap_or_default();
                    rsx! {
                        span { class: "sc-badge sc-badge--danger", "점검하지 못했습니다" }
                        p { class: "muted small", "{why}" }
                    }
                }
                PreviewState::Done if v.publishable => {
                    rsx! { span { class: "sc-badge sc-badge--success", "발행 가능" } }
                }
                PreviewState::Done => {
                    rsx! { span { class: "sc-badge sc-badge--danger", "발행 불가" } }
                }
            };
            rsx! {
                div { class: "sc-panel-section",
                    {state}
                    if !v.current {
                        p { class: "muted small", "원본이 바뀌어 이 결과는 낡았습니다 — 다시 돌리세요." }
                    }
                }
                div { class: "sc-panel-section",
                    p { class: "muted small", "예제 {v.example_count}개 · 히든 테스트 {v.hidden_count}개" }
                    if let Some(r) = &v.result {
                        div { class: "spread",
                            span { "모범 답안 통과" }
                            b { class: if r.passed == r.total && r.compiled { "sc-tone-success" } else { "sc-tone-danger" },
                                if r.compiled { "{r.passed} / {r.total}" } else { "컴파일 안 됨" }
                            }
                        }
                    }
                    if let Some(r) = &v.result {
                        for t in r.templates.iter() {
                            div { class: "spread", key: "{t.language.as_str()}",
                                span { "{t.language.display_name()} 틀 컴파일" }
                                b { class: if t.compiled { "sc-tone-success" } else { "sc-tone-danger" },
                                    if t.compiled { "통과" } else { "실패" }
                                }
                            }
                        }
                    }
                    if let Some(reason) = &v.reason {
                        p { class: "small", "{reason}" }
                    }
                }
            }
        }
    };

    rsx! {
        section { class: "sc-panel",
            div { class: "sc-panel-section",
                div { class: "spread",
                    h2 { class: "sc-panel-title", "발행 전 점검" }
                    button {
                        class: "sc-btn sc-btn--secondary sc-btn--sm",
                        r#type: "button",
                        disabled: run_disabled,
                        title: "{run_title}",
                        onclick: move |e| on_run.call(e),
                        "{run_label}"
                    }
                }
                if dirty {
                    p { class: "muted small", "저장하지 않은 변경이 있습니다. 저장 → 점검 → 발행 순서입니다." }
                }
            }
            {body}
        }
    }
}
