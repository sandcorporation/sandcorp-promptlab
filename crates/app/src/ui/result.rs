//! 제출 뒤 화면: 채점 대기 · 결과 3칸(문항 | 숨은 케이스·AI 활용 평가 | 대화 보기) · 응시 완료(채용 시험)·채점 실패.
//! 점수는 서버가 준 `SubmissionView.score`(최종, 0~100)와 계산식을 그대로 보인다 — `ResultRow.score`(0~1)와 섞지 않는다.

use dioxus::prelude::*;
use promptlab_app::session::{Phase, Session};
use promptlab_app::workspace::score_text;
use promptlab_shared::evaluation::{EvaluationStatus, ItemScore, ScoreBasis};

use super::{Route, Tab, Ui, masthead};

fn crumb_title(ui: &Ui) -> (String, String) {
    let s = ui.session.read();
    match s.exam_item() {
        Some(item) => {
            let title = ui
                .assignments
                .read()
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .and_then(|l| l.iter().find(|a| a.id == item.assignment_id))
                .map_or_else(|| "시험".to_owned(), |a| a.exam_title.clone());
            ("내 시험".into(), title)
        }
        None => (
            "개인 연습".into(),
            s.problem().map(|p| p.id.clone()).unwrap_or_default(),
        ),
    }
}

fn leave(ui: &Ui) {
    let practice = ui.session.read().exam_item().is_none();
    let mut tab = ui.tab;
    tab.set(if practice { Tab::Practice } else { Tab::Exams });
    ui.go_home();
    if practice {
        ui.load_practice();
    }
    let mut route = ui.route;
    route.set(Route::Home);
}

fn frame(body: Element) -> Element {
    rsx! {
        div { class: "app",
            {masthead(rsx! {})}
            div { class: "shell no-nav",
                main { class: "content", {body} }
            }
        }
    }
}

#[component]
pub fn Waiting() -> Element {
    let ui = use_context::<Ui>();
    let submitting = matches!(ui.session.read().phase(), Phase::Submitting { .. });
    frame(rsx! {
        div { class: "page",
            h1 { "제출했습니다" }
            if let Some(e) = (ui.error)() {
                div { class: "infobar error", role: "alert", "{e}" }
            }
            div { class: "card",
                p { strong { if submitting { "제출하는 중…" } else { "채점하는 중…" } } }
                p { class: "muted", "서버가 제출한 대화 뒤에 숨은 입력을 붙여 같은 모델로 채점한다(2초마다 확인). 창을 닫지 않아도 된다." }
            }
        }
    })
}

#[component]
pub fn Done() -> Element {
    let ui = use_context::<Ui>();
    let phase = ui.session.read().phase().clone();
    let ui_leave = ui.clone();
    frame(rsx! {
        div { class: "page",
            {match phase {
                Phase::GradingFailed { reason } => rsx! {
                    h1 { "채점하지 못했습니다" }
                    div { class: "infobar error", role: "alert", "채점하지 못했다: {reason}" }
                },
                _ => rsx! {
                    h1 { "응시 완료" }
                    div { class: "card",
                        p { strong { "제출을 받았습니다." } }
                        p { class: "muted", "이 시험의 결과는 시험을 낸 곳만 봅니다." }
                    }
                },
            }}
            button { class: "accent", onclick: move |_| leave(&ui_leave), "처음으로" }
        }
    })
}

fn status_note(status: EvaluationStatus) -> &'static str {
    match status {
        EvaluationStatus::Queued | EvaluationStatus::Evaluating => {
            "AI 활용 평가를 하는 중입니다. 끝나면 이 화면이 저절로 바뀝니다."
        }
        EvaluationStatus::Failed => "AI 활용 평가에 실패해 통과율만으로 최종 점수를 냈습니다.",
        EvaluationStatus::Disabled => "AI 활용 평가가 꺼져 있어 통과율만으로 최종 점수를 냈습니다.",
        EvaluationStatus::Smoke => "스모크 시험이라 AI 활용 평가를 하지 않았습니다.",
        EvaluationStatus::Practice => "개인 연습은 AI 활용 평가 없이 통과율이 최종 점수입니다.",
        EvaluationStatus::NotEvaluated => "AI 활용 평가 없이 통과율만으로 최종 점수를 냈습니다.",
        EvaluationStatus::Done => "",
    }
}

#[component]
pub fn ResultScreen() -> Element {
    let ui = use_context::<Ui>();
    let s: Session = ui.session.read().clone();
    let Phase::Graded { report, score, .. } = s.phase().clone() else {
        return rsx! {};
    };
    let (back, title) = crumb_title(&ui);
    let ui_leave = ui.clone();
    let hidden_pct = report.pass_rate() * 100.0;
    let total_cases = report.total;
    let pending = score
        .as_ref()
        .is_some_and(|x| x.basis == ScoreBasis::Pending);
    let position = s.exam_item().map(|i| i.position);
    let item_name = match position {
        Some(p) => format!("문항 {}", p + 1),
        None => "연습 문제".to_owned(),
    };
    let eval_score = score.as_ref().and_then(|x| x.evaluation_score);
    let list_sub = match (&score, eval_score) {
        (Some(_), Some(e)) => format!("숨은 {}% · 대화 {}", score_text(hidden_pct), score_text(e)),
        _ => format!("숨은 {}%", score_text(hidden_pct)),
    };
    let (code_language, code) = ui
        .editor
        .read()
        .as_ref()
        .map(|e| (e.language(), e.code().to_owned()))
        .unwrap_or((report.language, String::new()));
    let _ = code_language;
    let runs = (ui.run_count)();
    let crumbs_back = ui_leave.clone();
    frame(rsx! {
        div { class: "page wide result",
            div { class: "page-head",
                h1 { class: "crumbs",
                    button { class: "link", onclick: move |_| leave(&crumbs_back), "{back}" }
                    span { class: "sep", "›" }
                    "{title}"
                    span { class: "sep", "›" }
                    "결과"
                }
                button { onclick: move |_| leave(&ui_leave), if position.is_some() { "내 시험으로" } else { "개인 연습으로" } }
            }
            div { class: "sum3",
                section { class: "card block",
                    span { class: "sub", "최종 점수" }
                    {match score.as_ref().and_then(|x| x.final_score) {
                        Some(f) => rsx! { p { class: "big", "{score_text(f)}" span { class: "unit", " / 100" } } },
                        None if pending => rsx! { p { class: "big muted", "대기 중" } },
                        None => rsx! { p { class: "big", "{report.passed}/{total_cases}" } },
                    }}
                    p { class: "sub",
                        {match &score {
                            Some(x) => x.formula.clone(),
                            None => "숨은 검사 통과율이 주 점수다.".to_owned(),
                        }}
                    }
                }
                section { class: "card block",
                    span { class: "sub", "과정" }
                    p { class: "big small", "로컬 실행 {runs}회" }
                    p { class: "sub", "제출 언어 {report.language.display_name()}" }
                }
            }
            div { class: "grid3",
                section { class: "card block", "aria-label": "문항",
                    div { class: "qlist",
                        div { class: "qi on",
                            strong { "{item_name}" }
                            span { class: "sub", "{list_sub}" }
                        }
                    }
                }
                section { class: "card block mid", "aria-label": "문항 결과",
                    h3 { "히든 테스트" }
                    p { class: "big small", "{report.passed} / {total_cases}" span { class: "unit", " 통과 · {score_text(hidden_pct)}%" } }
                    div { class: "bar", role: "img", "aria-label": "숨은 케이스 통과율 {score_text(hidden_pct)}%",
                        span { class: "fill", style: "width: {hidden_pct}%" }
                    }
                    EvaluationCard { score: score.clone() }
                }
                section { class: "card block", "aria-label": "제출한 코드",
                    h3 { "제출한 코드" }
                    p { class: "sub", "{report.language.display_name()} · {code.len()}바이트" }
                    pre { class: "mono code-view", "{code}" }
                }
            }
        }
    })
}

#[component]
fn EvaluationCard(score: Option<ItemScore>) -> Element {
    let ui = use_context::<Ui>();
    let _ = &ui;
    let Some(score) = score else {
        return rsx! {};
    };
    rsx! {
        h3 { class: "section-gap", "AI 활용 평가" }
        {match &score.evaluation {
            Some(ev) => {
                let sum: u32 = ev.criteria.iter().map(|c| u32::from(c.score)).sum();
                let max: u32 = ev.criteria.iter().map(|c| u32::from(c.max)).sum();
                rsx! {
                    p { class: "big small", "{score_text(ev.score)}" span { class: "unit", " / 100 ({sum} / {max})" } }
                    div { class: "tscroll",
                        table { class: "crit",
                            thead { tr { th { "항목" } th { "점수" } th { "근거" } } }
                            tbody {
                                for c in ev.criteria.clone() {
                                    tr {
                                        td { strong { "{c.name}" } }
                                        td { class: "num", "{c.score}/{c.max}" }
                                        td {
                                            "{c.reason}"
                                            div { class: "ev",
                                                for e in c.evidence.clone() {
                                                    span { class: "chip", "{e.label()}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None => rsx! {
                p { class: "sub", strong { "{score.evaluation_status.label()}" } }
                div { class: if matches!(score.evaluation_status, EvaluationStatus::Queued | EvaluationStatus::Evaluating) { "infobar" } else { "infobar warn" }, role: "status", "{status_note(score.evaluation_status)}" }
            },
        }}
    }
}
