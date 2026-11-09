//! 웹캠 사진(#13): 감독관(감독 권한 — 관리자·감독관)이 응시 하나의 기준 얼굴 사진과 스냅샷(주기·감지 순간)을
//! 시간순으로 본다. 사진은 세션 쿠키로 인증된 같은 출처 요청으로만 받는다(공개 URL 없음 — 서버가 강제한다).
//! 감지 결과는 참고 신호일 뿐이고 판단은 고객사가 한다.

use dioxus::prelude::*;
use promptlab_shared::webcam::{SnapshotReason, WebcamView};

use crate::{api, notice};

/// 웹캠 사진 칸. `on_close`로 닫는다.
#[component]
pub fn WebcamPanel(
    org_id: String,
    exam_id: String,
    invite_id: String,
    on_close: EventHandler<()>,
) -> Element {
    let ids = (org_id.clone(), exam_id.clone(), invite_id.clone());
    let mut view = use_resource(move || {
        let (org_id, exam_id, invite_id) = ids.clone();
        async move { api::webcam(&org_id, &exam_id, &invite_id).await }
    });
    let base = format!(
        "{}/orgs/{org_id}/exams/{exam_id}/invites/{invite_id}/webcam/photos",
        api::API
    );
    let body = match &*view.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(v)) => photo_list(v.clone(), &base),
    };
    rsx! {
        section { class: "sc-panel sc-panel-section webcam",
            div { class: "row spread",
                h3 { "웹캠 사진" }
                span { class: "row",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| view.restart(), "새로고침" }
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_close.call(()), "닫기" }
                }
            }
            {body}
        }
    }
}

fn photo_list(v: WebcamView, base: &str) -> Element {
    let consent = match &v.consent {
        Some((version, at)) => format!(
            "동의 {} (안내문 {version})",
            crate::signals::kst(*at * 1000)
        ),
        None => "동의 기록 없음".to_owned(),
    };
    rsx! {
        p { class: "muted small",
            "{v.candidate} · {consent} · 사진은 촬영 {v.retention_days}일 뒤 자동 삭제 — 판단은 고객사가 합니다"
        }
        div { class: "photos",
            match v.baseline {
                Some(b) => rsx! {
                    figure { class: "baseline",
                        img { src: "{base}/{b.id}", alt: "기준 얼굴 사진", loading: "lazy" }
                        figcaption { "기준 사진 · {crate::signals::kst(b.at_ms)}" }
                    }
                },
                None => rsx! { p { class: "muted", "기준 사진 없음" } },
            }
            for s in v.snapshots {
                figure { class: if s.reason == SnapshotReason::Periodic { "snap" } else { "snap alert" },
                    img { src: "{base}/{s.id}", alt: "{s.reason.label()} 스냅샷", loading: "lazy" }
                    figcaption { "문항 {s.position + 1} · {s.reason.label()} · {crate::signals::kst(s.at_ms)}" }
                }
            }
        }
    }
}
