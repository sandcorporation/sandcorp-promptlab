//! 과정 다시보기(#9): 응시 문항 하나의 서명·체인 검증된 사건을 시간순으로 보인다. 결과 보기 권한(관리자·감독관·
//! 열람자)만 부른다 — 서버가 강제한다. 신호는 「살펴볼 것」일 뿐이고 판단은 고객사가 한다.

use dioxus::prelude::*;
use promptlab_shared::api::{ProcessLogView, ReplayEvent};
use promptlab_shared::log::{LocalRunKind, LogEvent};

use crate::{api, notice};

/// 다시보기 칸. `on_close`로 닫는다.
#[component]
pub fn ReplayPanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    position: u32,
    on_close: EventHandler<()>,
) -> Element {
    let ids = (org_id, exam_id, invite_id);
    let mut log = use_resource(move || {
        let (org_id, exam_id, invite_id) = ids.clone();
        async move { api::process_log(&org_id, &exam_id, &invite_id, position).await }
    });
    let body = match &*log.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(view)) => timeline(view.clone()),
    };
    rsx! {
        section { class: "sc-panel sc-panel-section replay",
            div { class: "row spread",
                h3 { "과정 다시보기 · 문항 {position + 1}" }
                span { class: "row",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| log.restart(), "새로고침" }
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
                }
            }
            {body}
        }
    }
}

/// 문항 시작부터 흐른 시간(앱 시계 기준, `m:ss`). 시작 시각을 모르면 첫 사건부터.
fn offset(at_ms: i64, origin_ms: i64) -> String {
    let secs = (at_ms - origin_ms).max(0) / 1000;
    format!("+{}:{:02}", secs / 60, secs % 60)
}

fn timeline(view: ProcessLogView) -> Element {
    let origin_ms = view
        .started_at
        .map(|s| s * 1000)
        .or_else(|| view.events.first().map(|e| e.at_ms))
        .unwrap_or(0);
    let submitted = view.submission_id.is_some();
    rsx! {
        p { class: "muted",
            "{view.candidate} · 사건 {view.events.len()}개"
            if submitted { " · 제출함" } else { " · 아직 제출 안 함" }
        }
        if !view.signals.is_empty() {
            div { class: "sc-alert", role: "status",
                strong { "살펴볼 신호" }
                span { class: "muted small", " — 판단은 고객사가 합니다" }
                ul {
                    for s in view.signals.iter() {
                        li { "{s.describe()}" }
                    }
                }
            }
        }
        if view.events.is_empty() {
            p { class: "muted", "올라온 과정 로그가 없습니다." }
        } else {
            ol { class: "plain timeline",
                for e in view.events {
                    {event_row(e, origin_ms)}
                }
            }
        }
    }
}

fn event_row(e: ReplayEvent, origin_ms: i64) -> Element {
    // 앱에서 일어난 뒤 1분 넘게 지나 받은 사건은 오프라인으로 쌓였다 올라온 것이다.
    let late = e.received_at * 1000 - e.at_ms > 60_000;
    let label = e.event.label();
    let detail = match e.event {
        LogEvent::LanguageChosen { language } => {
            rsx! { div { class: "muted small", "{language.display_name()}" } }
        }
        LogEvent::CodeSnapshot { code } => {
            let chars = code.chars().count();
            rsx! {
                details {
                    summary { class: "muted small", "코드 {chars}자" }
                    pre { class: "log", "{code}" }
                }
            }
        }
        LogEvent::AiMessage { text } => rsx! { pre { class: "log", "{text}" } },
        LogEvent::AiReply {
            text,
            tool_calls,
            stopped,
        } => rsx! {
            pre { class: "log", "{text}" }
            if stopped {
                div { class: "muted small", "응시자가 생성을 멈춤" }
            }
            if !tool_calls.is_empty() {
                div { class: "muted small",
                    "도구 호출: "
                    {tool_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")}
                }
            }
        },
        LogEvent::EditProposed {
            proposal,
            range,
            text,
        } => {
            let chars = text.chars().count();
            rsx! {
                details {
                    summary { class: "muted small",
                        "제안 {proposal} · {range.start_line}:{range.start_column}–{range.end_line}:{range.end_column} · {chars}자"
                    }
                    pre { class: "log", "{text}" }
                }
            }
        }
        LogEvent::EditAccepted { proposal } | LogEvent::EditRejected { proposal } => {
            rsx! { div { class: "muted small", "제안 {proposal}" } }
        }
        LogEvent::LocalRun {
            kind,
            passed,
            total,
            summary,
        } => {
            let kind = match kind {
                LocalRunKind::Run => "직접 실행",
                LocalRunKind::Examples => "예제 테스트",
            };
            let ratio = match (passed, total) {
                (Some(p), Some(t)) => format!(" · {p}/{t} 통과"),
                _ => String::new(),
            };
            rsx! { div { class: "muted small", "{kind}{ratio} · {summary}" } }
        }
        LogEvent::PasteBlocked { chars } => rsx! { div { class: "muted small", "{chars}자" } },
        LogEvent::Submitted {
            language,
            code_sha256,
        } => {
            let short: String = code_sha256.chars().take(12).collect();
            rsx! { div { class: "muted small", "{language.display_name()} · 코드 sha256 {short}…" } }
        }
        LogEvent::FocusReturned { away_ms } => {
            rsx! { div { class: "muted small", "비운 시간 {away_ms / 1000}초" } }
        }
        LogEvent::MultipleFaces { count, .. } => {
            rsx! { div { class: "muted small", "얼굴 {count}개" } }
        }
        LogEvent::CameraUnavailable { reason } => rsx! { div { class: "muted small", "{reason}" } },
        _ => rsx! {},
    };
    rsx! {
        li { class: "event",
            div { class: "row",
                span { class: "t", "{offset(e.at_ms, origin_ms)}" }
                strong { "{label}" }
                if e.key_index > 0 {
                    span { class: "sc-badge sc-badge--neutral", "설치 키 {e.key_index + 1}" }
                }
                if late {
                    span { class: "sc-badge sc-badge--info", "늦게 받음" }
                }
            }
            {detail}
        }
    }
}
