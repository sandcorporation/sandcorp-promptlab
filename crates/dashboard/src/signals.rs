//! 의심 신호 목록(#12): 응시자 한 명의 포커스 이탈·전체화면 이탈·붙여넣기, 모델 무결성 실패, 공개 예시 재실행
//! 대조 불일치, 과정 로그 검증 신호를 시간순으로 보인다. 결과 보기 권한(관리자·감독관·열람자)만 — 서버가 강제한다.
//! 신호는 「살펴볼 것」일 뿐이고 판단은 고객사가 한다.

use dioxus::prelude::*;
use promptlab_shared::api::SuspicionView;
use promptlab_shared::integrity::{SignalClock, Suspicion, SuspicionKind};

use crate::{api, notice};

/// 의심 신호 칸. `on_close`로 닫는다.
#[component]
pub fn SignalsPanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    on_close: EventHandler<()>,
) -> Element {
    let ids = (org_id, exam_id, invite_id);
    let mut view = use_resource(move || {
        let (org_id, exam_id, invite_id) = ids.clone();
        async move { api::suspicions(&org_id, &exam_id, &invite_id).await }
    });
    let body = match &*view.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(v)) => signal_list(v.clone()),
    };
    rsx! {
        section { class: "sc-panel sc-panel-section suspicions",
            div { class: "row spread",
                h3 { "의심 신호" }
                span { class: "row",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| view.restart(), "새로고침" }
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
                }
            }
            {body}
        }
    }
}

/// 유닉스 밀리초를 한국 시각 `MM-DD HH:MM:SS`로.
pub(crate) fn kst(at_ms: i64) -> String {
    let (_, rest) = kst_parts(at_ms);
    rest
}

/// 유닉스 밀리초를 한국 시각 `YYYY-MM-DD HH:MM`으로(해를 넘겨 남는 기록용).
pub(crate) fn kst_full(at_ms: i64) -> String {
    let (year, rest) = kst_parts(at_ms);
    let minutes = &rest[..rest.len() - 3];
    format!("{year}-{minutes}")
}

/// (해, `MM-DD HH:MM:SS`).
fn kst_parts(at_ms: i64) -> (i64, String) {
    let secs = at_ms.div_euclid(1000) + 9 * 3600;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // 1970-01-01부터 날 수 → 그레고리력 날짜(Howard Hinnant의 civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        format!(
            "{month:02}-{day:02} {:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        ),
    )
}

/// 크게 살펴볼 신호(모델·로그 무결성)는 붉게.
fn is_alert(kind: &SuspicionKind) -> bool {
    matches!(
        kind,
        SuspicionKind::IntegrityRejected { .. } | SuspicionKind::Log { .. }
    )
}

fn signal_list(view: SuspicionView) -> Element {
    rsx! {
        p { class: "muted",
            "{view.candidate} · 신호 {view.signals.len()}개 — 판단은 고객사가 합니다"
        }
        if view.signals.is_empty() {
            p { class: "muted", "살펴볼 신호가 없습니다." }
        } else {
            ol { class: "plain timeline",
                for s in view.signals {
                    {signal_row(s)}
                }
            }
        }
    }
}

fn signal_row(s: Suspicion) -> Element {
    let when = match s.at_ms {
        Some(ms) => kst(ms),
        None => "시각 모름".to_owned(),
    };
    let clock = match s.clock {
        SignalClock::App => "앱 시계",
        SignalClock::Server => "서버 시계",
    };
    let class = if is_alert(&s.kind) {
        "sc-badge sc-badge--danger"
    } else {
        "sc-badge sc-badge--info"
    };
    rsx! {
        li { class: "event",
            div { class: "row",
                span { class: "t", title: "{clock}", "{when}" }
                span { class: "{class}", "{s.kind.label()}" }
                span { class: "muted small", "문항 {s.position + 1} · {clock}" }
            }
            div { class: "desc", "{s.kind.describe()}" }
        }
    }
}
