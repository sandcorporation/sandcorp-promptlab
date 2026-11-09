//! AI 리포트(#15): 결과 보기 역할(관리자·감독관·열람자)이 응시자 한 명의 강점·약점·개선 제안·의심 신호 요약을
//! 본다. 리포트는 참고용이고 점수를 바꾸지 않는다(면책 문구는 서버가 내려 준다). 대기·만드는 중이면 자동으로
//! 다시 읽고, 「다시 만들기」는 서버가 정한 횟수(`max_generations`)까지만.

use dioxus::prelude::*;
use promptlab_shared::report::{ReportStatus, ReportView};

use crate::signals::kst;
use crate::{api, notice};

/// 대기·만드는 중일 때 다시 읽는 간격.
const POLL_MS: u32 = 3000;

fn in_progress(view: &ReportView) -> bool {
    matches!(view.status, ReportStatus::Queued | ReportStatus::Generating)
}

/// AI 리포트 칸. `on_close`로 닫는다.
#[component]
pub fn ReportPanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    candidate: String,
    on_close: EventHandler<()>,
) -> Element {
    let mut view = use_signal(|| None::<Result<ReportView, String>>);
    let mut busy = use_signal(|| false);
    let ids = (org_id, exam_id, invite_id);

    // 읽고, 대기·만드는 중이면 끝날 때까지 다시 읽는다.
    let load_ids = ids.clone();
    let mut poll = use_future(move || {
        let (org_id, exam_id, invite_id) = load_ids.clone();
        async move {
            loop {
                let got = api::report(&org_id, &exam_id, &invite_id).await;
                let again = matches!(&got, Ok(v) if in_progress(v));
                view.set(Some(got.map_err(|e| e.to_string())));
                if !again {
                    break;
                }
                api::sleep_ms(POLL_MS).await;
            }
        }
    });

    let request = move |_| {
        let (org_id, exam_id, invite_id) = ids.clone();
        busy.set(true);
        spawn(async move {
            match api::request_report(&org_id, &exam_id, &invite_id).await {
                Ok(v) => {
                    view.set(Some(Ok(v)));
                    poll.restart();
                }
                Err(e) => view.set(Some(Err(e.to_string()))),
            }
            busy.set(false);
        });
    };

    let body = match &*view.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.clone()),
        Some(Ok(v)) => report_body(v),
    };
    let can_request = matches!(&*view.read(), Some(Ok(v)) if v.can_request) && !busy();
    let label = match &*view.read() {
        Some(Ok(v)) if v.generations > 0 => "다시 만들기",
        _ => "만들기",
    };
    let count = match &*view.read() {
        Some(Ok(v)) => format!("{}/{}회", v.generations, v.max_generations),
        _ => String::new(),
    };

    rsx! {
        section { class: "sc-panel sc-panel-section report",
            div { class: "row spread",
                h3 { "AI 리포트" }
                span { class: "row",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| poll.restart(), "새로고침" }
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
                }
            }
            p { class: "muted", "{candidate}" }
            {body}
            div { class: "row actions",
                button { class: "sc-btn sc-btn--accent sc-btn--sm", disabled: !can_request, onclick: request, "{label}" }
                span { class: "muted small", "{count}" }
            }
        }
    }
}

fn section(title: &str, items: &[String]) -> Element {
    rsx! {
        h4 { "{title}" }
        if items.is_empty() {
            p { class: "muted", "없음" }
        } else {
            ul {
                for t in items.iter() {
                    li { "{t}" }
                }
            }
        }
    }
}

fn report_body(v: &ReportView) -> Element {
    let status_class = match v.status {
        ReportStatus::Done => "sc-badge sc-badge--success",
        ReportStatus::Failed => "sc-badge sc-badge--danger",
        ReportStatus::Queued | ReportStatus::Generating => "sc-badge sc-badge--info",
        ReportStatus::None | ReportStatus::Disabled => "sc-badge sc-badge--neutral",
    };
    let when = v.finished_at.map(|s| kst(s * 1000));
    let meta = match (&when, &v.model) {
        (Some(w), Some(m)) => format!("{w} · {m}"),
        (Some(w), None) => w.clone(),
        (None, Some(m)) => m.clone(),
        (None, None) => String::new(),
    };
    let hint = match v.status {
        ReportStatus::None => {
            Some("아직 리포트가 없습니다. 모든 문항의 채점이 끝나면 만들 수 있습니다.")
        }
        ReportStatus::Queued | ReportStatus::Generating => {
            Some("만드는 중입니다. 몇 분 걸릴 수 있습니다.")
        }
        ReportStatus::Disabled => Some("이 서버에서는 AI 리포트 기능이 꺼져 있습니다."),
        ReportStatus::Done | ReportStatus::Failed => None,
    };
    rsx! {
        div { class: "row",
            span { class: "{status_class}", "{v.status.label()}" }
            span { class: "muted small", "{meta}" }
        }
        if let Some(h) = hint {
            p { class: "muted", "{h}" }
        }
        if let Some(reason) = &v.failure_reason {
            p { class: "muted", "실패 까닭: {reason}" }
        }
        if let Some(r) = &v.report {
            {section("강점", &r.strengths)}
            {section("약점", &r.weaknesses)}
            {section("개선 제안", &r.suggestions)}
            h4 { "의심 신호 요약" }
            p { class: "desc", "{r.suspicion_summary}" }
        }
        p { class: "disclaimer small", "{v.disclaimer}" }
    }
}
