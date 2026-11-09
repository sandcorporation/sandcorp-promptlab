//! 응시자 상세 「점수」 탭의 가운데 칸(#100): 제출 코드(읽기 전용)·편집 타임라인(스냅숏 diff·제안 결정·수락 뒤 수정)·
//! AI 대화 원문·로컬 실행 기록. 평가 근거 칩(「턴 n」「제안 #k」)이 가리키는 자리 — 앵커 `turn-{n}`·`proposal-{n}`.

use dioxus::prelude::*;
use promptlab_shared::activity::{Activity, Run, Turn};
use promptlab_shared::api::{ItemActivity, ProcessLogView};

use crate::timeline::{DiffKind, Step, decision_text, edit_timeline, range_text};
use crate::{api, notice};

/// 문항 하나의 코드·타임라인·대화. `picked`는 근거 칩이 눌러 둔 앵커(그 요소를 강조한다).
#[component]
pub fn ItemActivityPanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    position: u32,
    picked: Signal<Option<String>>,
) -> Element {
    let ids = (org_id, exam_id, invite_id);
    let data = use_resource(move || {
        let (org_id, exam_id, invite_id) = ids.clone();
        async move {
            let activity = api::item_activity(&org_id, &exam_id, &invite_id, position).await?;
            // 스냅숏 원문은 로그에 있다. 로그가 없어도(올라오지 않음) 기록·코드는 보인다.
            let log = api::process_log(&org_id, &exam_id, &invite_id, position)
                .await
                .ok();
            Ok::<_, api::Error>((activity, log))
        }
    });
    match &*data.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok((activity, log))) => body(activity, log.as_ref(), picked),
    }
}

fn hl(picked: Signal<Option<String>>, id: &str) -> &'static str {
    if picked().as_deref() == Some(id) {
        "hl"
    } else {
        ""
    }
}

fn body(
    item: &ItemActivity,
    log: Option<&ProcessLogView>,
    picked: Signal<Option<String>>,
) -> Element {
    let activity: &Activity = &item.activity;
    let events = log.map(|l| l.events.as_slice()).unwrap_or_default();
    let steps = edit_timeline(events, activity);
    let origin = log
        .and_then(|l| l.started_at.map(|s| s * 1000))
        .or_else(|| events.first().map(|e| e.at_ms))
        .unwrap_or(0);
    rsx! {
        section { class: "sc-panel sc-panel-section", "aria-label": "제출 코드",
            div { class: "row spread",
                h2 { class: "sc-panel-title", "제출 코드" }
                div { class: "row",
                    if let Some(language) = item.language {
                        span { class: "sc-badge sc-badge--neutral", "{language.display_name()}" }
                    }
                    if let Some(model) = &item.model {
                        span {
                            class: "sc-badge sc-badge--neutral",
                            title: "문항을 시작할 때 쓴 AI 도우미 모델 — 응시자 PC에 맞춰 고른 크기라 도우미 성능이 다를 수 있습니다",
                            "AI {model}"
                        }
                    }
                }
            }
            match &item.final_code {
                Some(code) => rsx! { pre { class: "code", tabindex: "0", "aria-label": "제출 코드 원문", "{code}" } },
                None => rsx! { p { class: "muted", "아직 제출하지 않았습니다." } },
            }
        }
        section { class: "sc-panel sc-panel-section", "aria-label": "편집 타임라인",
            h2 { class: "sc-panel-title", "편집 타임라인" }
            if steps.is_empty() {
                p { class: "muted", "코드 스냅숏이나 AI 편집 제안이 올라오지 않았습니다." }
            } else {
                ol { class: "plain steps",
                    for s in steps {
                        {step_row(s, origin, picked)}
                    }
                }
            }
        }
        section { class: "sc-panel sc-panel-section", "aria-label": "AI 대화",
            h2 { class: "sc-panel-title", "AI 대화 원문" }
            if activity.turns.is_empty() {
                p { class: "muted", "AI 도우미와 나눈 대화가 없습니다." }
            }
            for t in activity.turns.iter() {
                {turn_block(t, origin, picked)}
            }
        }
        section { class: "sc-panel sc-panel-section", "aria-label": "로컬 실행",
            h2 { class: "sc-panel-title", "로컬 실행 기록" }
            if activity.runs.is_empty() {
                p { class: "muted", "로컬 실행이 없습니다." }
            } else {
                ul { class: "plain steps",
                    for r in activity.runs.iter() {
                        {run_row(r, origin)}
                    }
                }
            }
        }
    }
}

/// 문항 시작부터 흐른 시간(`+m:ss`).
fn offset(at_ms: i64, origin_ms: i64) -> String {
    let secs = (at_ms - origin_ms).max(0) / 1000;
    format!("+{}:{:02}", secs / 60, secs % 60)
}

fn step_row(step: Step, origin: i64, picked: Signal<Option<String>>) -> Element {
    match step {
        Step::Snapshot {
            seq,
            at_ms,
            added,
            removed,
            diff,
            modifies,
        } => rsx! {
            li { key: "s{seq}", class: "step",
                div { class: "row",
                    span { class: "t", "{offset(at_ms, origin)}" }
                    strong { "코드 바뀜" }
                    span { class: "sc-badge sc-badge--success", "+{added}" }
                    span { class: "sc-badge sc-badge--danger", "−{removed}" }
                    for p in modifies {
                        span { class: "sc-badge sc-badge--warning", "제안 #{p} 수락 뒤 수정" }
                    }
                }
                details {
                    summary { class: "muted small", "줄 변경 보기" }
                    pre { class: "diff", tabindex: "0",
                        for (i, d) in diff.into_iter().enumerate() {
                            span {
                                key: "{i}",
                                class: match d.kind {
                                    DiffKind::Same => "ln",
                                    DiffKind::Added => "ln add",
                                    DiffKind::Removed => "ln del",
                                },
                                "{d.text}\n"
                            }
                        }
                    }
                }
            }
        },
        Step::Proposal(p) => {
            let id = format!("proposal-{}", p.proposal);
            let class = format!("step {}", hl(picked, &id));
            let badge = match p.decision {
                promptlab_shared::activity::Decision::Accepted => "sc-badge sc-badge--success",
                promptlab_shared::activity::Decision::Rejected => "sc-badge sc-badge--neutral",
                promptlab_shared::activity::Decision::Undecided => "sc-badge sc-badge--info",
            };
            rsx! {
                li { key: "p{p.proposal}", id: "{id}", class: "{class}",
                    div { class: "row",
                        span { class: "t", "{offset(p.at_ms, origin)}" }
                        strong { "제안 #{p.proposal}" }
                        span { class: "{badge}", "{decision_text(&p)}" }
                        if let Some(turn) = p.turn {
                            span { class: "muted small", "턴 {turn}에서" }
                        }
                    }
                    details {
                        summary { class: "muted small", "줄 {range_text(&p)}에 넣을 글 {p.text.chars().count()}자" }
                        pre { class: "code", tabindex: "0", "{p.text}" }
                    }
                }
            }
        }
    }
}

fn turn_block(t: &Turn, origin: i64, picked: Signal<Option<String>>) -> Element {
    let id = format!("turn-{}", t.turn);
    let class = format!("turn {}", hl(picked, &id));
    rsx! {
        div { key: "{id}", id: "{id}", class: "{class}",
            div { class: "row",
                span { class: "t", "{offset(t.at_ms, origin)}" }
                strong { "턴 {t.turn}" }
                span { class: "muted small", "응시자" }
            }
            pre { class: "msg user", "{t.message}" }
            for r in t.replies.iter() {
                div { key: "r{r.seq}", class: "reply",
                    span { class: "muted small", "AI 도우미" }
                    if !r.text.is_empty() {
                        pre { class: "msg", "{r.text}" }
                    }
                    if r.stopped {
                        div { class: "muted small", "응시자가 생성을 멈춤" }
                    }
                    if !r.tool_calls.is_empty() {
                        div { class: "muted small",
                            "도구 호출: "
                            {r.tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")}
                        }
                    }
                }
            }
        }
    }
}

fn run_row(r: &Run, origin: i64) -> Element {
    let kind = match r.kind {
        promptlab_shared::log::LocalRunKind::Run => "직접 실행",
        promptlab_shared::log::LocalRunKind::Examples => "예제 테스트",
    };
    rsx! {
        li { key: "run{r.seq}", class: "step",
            div { class: "row",
                span { class: "t", "{offset(r.at_ms, origin)}" }
                strong { "{kind}" }
                if let (Some(p), Some(t)) = (r.passed, r.total) {
                    span { class: "sc-badge sc-badge--neutral", "{p}/{t} 통과" }
                }
                if let Some(turn) = r.turn {
                    span { class: "muted small", "턴 {turn} 뒤" }
                }
            }
            if !r.summary.is_empty() {
                div { class: "muted small", "{r.summary}" }
            }
        }
    }
}
