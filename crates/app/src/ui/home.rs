//! 시작 화면: 왼쪽 NavigationView + 내 시험·개인 연습(목록+상세 2칸)·설정. 응시 전 점검 페이지도 이 틀 안에 뜬다.

use dioxus::prelude::*;
use promptlab_app::session::{ExamItemRef, Phase};
use promptlab_app::workspace::nav_collapsed;
use promptlab_shared::api::{AssignmentView, ItemState};
use promptlab_shared::coding::PublicCodingProblem;

use super::{APP_VERSION, Route, Tab, Target, Ui, check, masthead};

fn assignment_state(a: &AssignmentView) -> (&'static str, &'static str) {
    let all_done = a
        .items
        .iter()
        .all(|i| matches!(i.state, ItemState::Submitted { .. }));
    let any_started = a
        .items
        .iter()
        .any(|i| !matches!(i.state, ItemState::NotStarted));
    if all_done {
        ("badge ok", "응시 완료")
    } else if any_started {
        ("badge warn", "응시 중")
    } else {
        ("badge", "시작 전")
    }
}

fn is_done(a: &AssignmentView) -> bool {
    a.items
        .iter()
        .all(|i| matches!(i.state, ItemState::Submitted { .. }))
}

#[component]
pub fn Shell() -> Element {
    let ui = use_context::<Ui>();
    let phase = ui.session.read().phase().clone();
    let route = (ui.route)();
    let tab_now = (ui.tab)();
    let collapsed = nav_collapsed((ui.width)());
    let notice = ui.session.read().notice().map(str::to_owned);
    let error = (ui.error)();
    let mut tab = ui.tab;
    let mut route_sig = ui.route;
    let nav_item = |t: Tab, label: &'static str, icon: Element| {
        rsx! {
            button {
                class: "nav-item",
                title: "{label}",
                "aria-current": if tab_now == t { "page" } else { "false" },
                onclick: move |_| {
                    tab.set(t);
                    route_sig.set(Route::Home);
                },
                {icon}
                span { class: "nav-label", "{label}" }
            }
        }
    };
    rsx! {
        div { class: "app",
            {masthead(rsx! { span { class: "badge", "상태: {phase.name()}" } })}
            div { class: "shell", "data-nav": if collapsed { "collapsed" } else { "full" },
                nav { class: "nav", "aria-label": "주 메뉴",
                    div { class: "nav-group",
                        {nav_item(Tab::Exams, "내 시험", rsx! { svg { "viewBox": "0 0 16 16", "aria-hidden": "true",
                            rect { x: "2.5", y: "1.5", width: "11", height: "13", rx: "2" }
                            path { d: "M5 5.5h6M5 8h6M5 10.5h3.5" }
                        } })}
                        {nav_item(Tab::Practice, "개인 연습", rsx! { svg { "viewBox": "0 0 16 16", "aria-hidden": "true",
                            path { d: "M3 13.5l1-3.2 7.4-7.4a1.4 1.4 0 0 1 2 2L6 12.3z" }
                            path { d: "M10 4.3l1.9 1.9" }
                        } })}
                    }
                    div { class: "nav-group",
                        div { class: "nav-sep" }
                        {nav_item(Tab::Settings, "설정", rsx! { svg { "viewBox": "0 0 16 16", "aria-hidden": "true",
                            circle { cx: "8", cy: "8", r: "2.2" }
                            path { d: "M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" }
                        } })}
                    }
                }
                main { class: "content",
                    if let Some(n) = notice {
                        div { class: "infobar", role: "status", "{n}" }
                    }
                    if let Some(e) = error {
                        div { class: "infobar error", role: "alert", "{e}" }
                    }
                    match route {
                        Route::Check(target) => rsx! { check::CheckPage { target, checking: matches!(phase, Phase::Checking) } },
                        Route::Home => match tab_now {
                            Tab::Exams => rsx! { ExamsPane {} },
                            Tab::Practice => rsx! { PracticePane {} },
                            Tab::Settings => rsx! { SettingsPane {} },
                        },
                    }
                }
            }
        }
    }
}

/// 방식 설명 네 칸(내 시험·개인 연습 상세가 같이 쓴다).
fn method_cards() -> Element {
    let steps = [
        (
            "편집기로 풀기",
            "문제의 함수 서명대로 코드를 씁니다. Python·JavaScript·Java·C++ 중 시험이 허용한 언어를 고릅니다.",
        ),
        (
            "이 PC에서 실행",
            "「실행」(직접 넣은 인자)·「예제 테스트」(공개 예제)는 이 PC의 Docker에서 채점과 같은 환경으로 돕니다.",
        ),
        ("코드 제출", "언어와 코드를 냅니다. 문항마다 한 번입니다."),
        (
            "채점",
            "서버가 숨은 테스트로 돌려 통과율을 매깁니다. 과정(코드 스냅숏·실행)은 과정 로그에 남습니다.",
        ),
    ];
    rsx! {
        div { class: "method-grid",
            for (i, (title, text)) in steps.into_iter().enumerate() {
                div { class: "method-card",
                    span { class: "step-no", "{i + 1}" }
                    div {
                        strong { "{title}" }
                        p { class: "muted", "{text}" }
                    }
                }
            }
        }
    }
}

#[component]
fn ExamsPane() -> Element {
    let ui = use_context::<Ui>();
    let mut selected = ui.selected;
    let mut query = use_signal(String::new);
    let assignments = (ui.assignments)();
    let route = ui.route;
    let ui_load = ui.clone();
    let list: Vec<AssignmentView> = match &assignments {
        Some(Ok(l)) => {
            let q = query.read().trim().to_lowercase();
            l.iter()
                .filter(|a| q.is_empty() || a.exam_title.to_lowercase().contains(&q))
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    };
    let open: Vec<&AssignmentView> = list.iter().filter(|a| !is_done(a)).collect();
    let finished: Vec<&AssignmentView> = list.iter().filter(|a| is_done(a)).collect();
    let ordered: Vec<&AssignmentView> = open.iter().chain(finished.iter()).copied().collect();
    let idx = selected().min(ordered.len().saturating_sub(1));
    let current = ordered.get(idx).copied().cloned();
    rsx! {
        div { class: "page wide",
            div { class: "page-head",
                h1 { "내 시험" }
                div { class: "head-tools",
                    input {
                        r#type: "text",
                        class: "search",
                        placeholder: "시험 이름으로 찾기",
                        "aria-label": "시험 이름으로 찾기",
                        oninput: move |e| query.set(e.value()),
                    }
                    button { onclick: move |_| ui_load.load_assignments(),
                        if assignments.is_some() { "새로 고침" } else { "내 시험 불러오기" }
                    }
                }
            }
            match &assignments {
                None => rsx! { p { class: "muted", "초대 메일의 링크로 배정받은 시험이 여기 보인다. 처음이면 브라우저로 카카오 로그인한다." } },
                Some(Err(e)) => rsx! { div { class: "infobar error", role: "alert", "불러오지 못했다: {e}" } },
                Some(Ok(l)) if l.is_empty() => rsx! { p { class: "muted", "배정된 시험이 없다." } },
                Some(Ok(_)) => rsx! {
                    div { class: "split",
                        div { class: "list", role: "listbox", "aria-label": "시험 목록",
                            if !open.is_empty() {
                                h2 { class: "list-head", "응시 가능 · {open.len()}" }
                            }
                            for (i, a) in ordered.iter().enumerate() {
                                if i == open.len() && !finished.is_empty() {
                                    h2 { class: "list-head", "끝난 시험 · {finished.len()}" }
                                }
                                {
                                    let (cls, label) = assignment_state(a);
                                    let sub = list_sub(a);
                                    let title = a.exam_title.clone();
                                    rsx! {
                                        button {
                                            class: if i == idx { "list-item on" } else { "list-item" },
                                            role: "option",
                                            "aria-selected": "{i == idx}",
                                            onclick: move |_| selected.set(i),
                                            span { class: "li-top",
                                                strong { "{title}" }
                                                span { class: "{cls}", "{label}" }
                                            }
                                            span { class: "sub", "{sub}" }
                                        }
                                    }
                                }
                            }
                        }
                        {match current {
                            Some(a) => rsx! { ExamDetail { assignment: a, route } },
                            None => rsx! { div { class: "detail muted", "찾는 시험이 없다." } },
                        }}
                    }
                },
            }
        }
    }
}

fn list_sub(a: &AssignmentView) -> String {
    let minutes = a.items.first().map_or(0, |i| i.limits.time_limit_secs / 60);
    let submitted = a
        .items
        .iter()
        .filter(|i| matches!(i.state, ItemState::Submitted { .. }))
        .count();
    if !a.scores_visible && is_done(a) {
        format!(
            "[{}] · {} · 결과는 기관이 알려 줍니다",
            a.org_name,
            a.purpose.label()
        )
    } else if submitted > 0 && !is_done(a) {
        format!(
            "[{}] · {} · 문항 {}개 중 {} 제출",
            a.org_name,
            a.purpose.label(),
            a.items.len(),
            submitted
        )
    } else {
        format!(
            "[{}] · {} · 문항 {} · {}분",
            a.org_name,
            a.purpose.label(),
            a.items.len(),
            minutes
        )
    }
}

#[component]
fn ExamDetail(assignment: AssignmentView, route: Signal<Route>) -> Element {
    let mut route = route;
    let a = assignment;
    let next = a
        .items
        .iter()
        .find(|i| !matches!(i.state, ItemState::Submitted { .. }))
        .cloned();
    let resuming = a
        .items
        .iter()
        .any(|i| matches!(i.state, ItemState::InProgress { .. }));
    let minutes = a.items.first().map_or(0, |i| i.limits.time_limit_secs / 60);
    let result_note = if a.scores_visible {
        "개인 결과 공개"
    } else {
        "결과는 기관이 알려 줍니다"
    };
    let title = a.exam_title.clone();
    let terms = a.webcam.clone().filter(|t| t.required);
    let assignment_id = a.id.clone();
    let languages = a.allowed_languages.clone();
    let language_names = a
        .allowed_languages
        .iter()
        .map(|l| l.display_name())
        .collect::<Vec<_>>()
        .join(" · ");
    rsx! {
        section { class: "detail", "aria-label": "시험 상세",
            div { class: "detail-head",
                div {
                    h2 { "{a.exam_title}" }
                    p { class: "muted", "[{a.org_name}] · {a.purpose.label()} · 문항 {a.items.len()}개 · 제한 시간 {minutes}분 · {result_note}" }
                }
                if let Some(item) = next {
                    button {
                        class: "accent",
                        onclick: move |_| route.set(Route::Check(Target::Exam {
                            item: ExamItemRef { assignment_id: assignment_id.clone(), position: item.position },
                            title: title.clone(),
                            terms: terms.clone(),
                            languages: languages.clone(),
                        })),
                        if resuming { "이어서 응시" } else { "응시 준비" }
                    }
                } else {
                    span { class: "badge ok", "응시 완료" }
                }
            }
            div { class: "detail-body",
                h3 { "시험 방식" }
                {method_cards()}
                h3 { "점수 구성" }
                p { class: "muted", "지금 최종 점수는 숨은 테스트 통과율입니다(AI 활용 평가는 준비 중). 결과를 공개하는 시험이면 결과 화면에 계산식이 나옵니다." }
                h3 { "문항" }
                table { class: "items",
                    thead { tr { th { "문항" } th { "제한 시간" } th { "상태" } } }
                    tbody {
                        for item in a.items.clone() {
                            tr {
                                td { "{item.position + 1}" }
                                td { "{item.limits.time_limit_secs / 60}분" }
                                td {
                                    match item.state {
                                        ItemState::Submitted { .. } => rsx! { span { class: "badge ok", "제출함" } },
                                        ItemState::InProgress { .. } => rsx! { span { class: "badge warn", "응시 중" } },
                                        ItemState::NotStarted => rsx! { span { class: "badge", "시작 전" } },
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "muted", "허용 언어: {language_names}. 응시 전 점검에서 이 언어들의 실행 이미지를 받아 둡니다." }
            }
        }
    }
}

#[component]
fn PracticePane() -> Element {
    let ui = use_context::<Ui>();
    let mut pick = ui.practice_pick;
    let list = (ui.practice_list)();
    let default_id = ui.api.config().problem_id.clone();
    let ui_load = ui.clone();
    let mut route = ui.route;
    rsx! {
        div { class: "page wide",
            div { class: "page-head",
                h1 { "개인 연습" }
                div { class: "head-tools",
                    button { onclick: move |_| ui_load.load_practice(),
                        if list.is_some() { "새로 고침" } else { "연습 문제 불러오기" }
                    }
                }
            }
            p { class: "muted", "초대 없이 혼자 연습한다. 연습 문제는 시험에 나오지 않고, 점수는 서버가 숨은 테스트로 매긴다. 기록은 나만 본다(고객사에 보이지 않는다)." }
            match list {
                None => rsx! {},
                Some(Err(e)) => rsx! { div { class: "infobar error", role: "alert", "불러오지 못했다: {e}" } },
                Some(Ok(problems)) => {
                    let current_id = {
                        let p = pick();
                        if p.is_empty() { default_id.clone() } else { p }
                    };
                    let current: Option<PublicCodingProblem> = problems
                        .iter()
                        .find(|p| p.id == current_id)
                        .or_else(|| problems.first())
                        .cloned();
                    rsx! {
                        div { class: "split",
                            div { class: "list", role: "listbox", "aria-label": "연습 문제 목록",
                                h2 { class: "list-head", "연습 문제 · {problems.len()}" }
                                for p in problems.clone() {
                                    {
                                        let id = p.id.clone();
                                        let on = current.as_ref().is_some_and(|c| c.id == p.id);
                                        rsx! {
                                            button {
                                                class: if on { "list-item on" } else { "list-item" },
                                                role: "option",
                                                "aria-selected": "{on}",
                                                onclick: move |_| pick.set(id.clone()),
                                                span { class: "li-top", strong { "{p.title}" } }
                                                span { class: "sub clamp", "{p.description}" }
                                            }
                                        }
                                    }
                                }
                            }
                            {match current {
                                Some(p) => rsx! {
                                    section { class: "detail", "aria-label": "연습 문제 상세",
                                        div { class: "detail-head",
                                            div {
                                                h2 { "{p.title}" }
                                                p { class: "muted", "개인 연습 · 예제 {p.examples.len()}개 · 케이스당 {p.limits.case_time_ms}ms" }
                                            }
                                            button {
                                                class: "accent",
                                                onclick: {
                                                    let p = p.clone();
                                                    move |_| route.set(Route::Check(Target::Practice { problem: p.clone() }))
                                                },
                                                "연습 준비"
                                            }
                                        }
                                        div { class: "detail-body",
                                            h3 { "문제" }
                                            pre { class: "desc", "{p.description}" }
                                            h3 { "시험 방식" }
                                            {method_cards()}
                                        }
                                    }
                                },
                                None => rsx! { div { class: "detail muted", "연습 문제가 없다." } },
                            }}
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SettingsPane() -> Element {
    let ui = use_context::<Ui>();
    // 크기는 응시 전 점검에서 바꾼다(#131) — 여기서는 보이기만.
    let model_line = {
        let pick = ui.pick.read();
        let chosen = pick.chosen();
        let how = if pick.by_user() {
            "직접 고름"
        } else {
            "자동"
        };
        let have = if ui.model_ready() {
            "받아 둠"
        } else {
            "받지 않음"
        };
        format!(
            "{} ({how}, {have}) — {}",
            chosen.model.label,
            pick.headline()
        )
    };
    rsx! {
        div { class: "page",
            h1 { "설정" }
            div { class: "card",
                dl { class: "info",
                    dt { "서버 주소" }
                    dd { "{ui.api.config().server_url}" }
                    dt { "앱 판" }
                    dd { "{APP_VERSION}" }
                    dt { "AI 모델" }
                    dd { "{model_line}" }
                    dt { "로그인" }
                    dd { "sandcorp 계정(카카오). 처음 서버를 부를 때 브라우저로 로그인한다." }
                }
            }
        }
    }
}
