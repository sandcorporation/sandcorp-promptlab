//! 내 연습 기록(#18): 응시 앱에서 혼자 푼 연습의 문항별 최고·최근 점수와 추이, 최근 제출.
//! 로그인한 본인 것만(`GET /me/practice`) — 조직 화면과 따로 두고, 고객사(조직) 화면 어디에도 싣지 않는다.

use dioxus::prelude::*;
use promptlab_shared::api::SubmissionState;
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::practice::{PracticeAttempt, PracticeRecords, ProblemRecord};

use crate::api;
use crate::results::{score_text, ymd};

/// 최근 제출 목록에 보이는 개수.
const RECENT: usize = 10;

#[component]
pub fn PracticePanel() -> Element {
    let records = use_resource(|| async { api::my_practice().await });
    let problems = use_resource(|| async { api::practice_problems().await.unwrap_or_default() });

    let body = match &*records.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => crate::notice(e.to_string()),
        Some(Ok(r)) if r.attempts.is_empty() => rsx! {
            p { class: "muted", "아직 연습 기록이 없습니다. 응시 앱의 「개인 연습」에서 문제를 골라 풀면 여기에 쌓입니다." }
        },
        Some(Ok(r)) => {
            let catalog = problems.read().clone().unwrap_or_default();
            records_view(r, &catalog)
        }
    };
    rsx! {
        section { class: "sc-panel sc-panel-section practice",
            h2 { class: "sc-panel-title", "내 연습 기록" }
            p { class: "muted", "나만 봅니다. 소속 조직(고객사)의 화면·결과에는 보이지 않습니다. 연습 문제는 시험에 나오지 않습니다." }
            {body}
        }
    }
}

fn title(catalog: &[PublicCodingProblem], id: &str) -> String {
    let Some(p) = catalog.iter().find(|p| p.id == id) else {
        return id.to_owned();
    };
    p.title.clone()
}

fn records_view(r: &PracticeRecords, catalog: &[PublicCodingProblem]) -> Element {
    let problems = r.problems.clone();
    let recent: Vec<PracticeAttempt> = r.attempts.iter().take(RECENT).cloned().collect();
    rsx! {
        h3 { "문항별" }
        ul { class: "plain",
            for p in problems {
                li { key: "{p.problem_id}",
                    div { class: "row spread",
                        span { class: "desc",
                            strong { "{p.problem_id}" }
                            br {}
                            span { class: "muted", "{title(catalog, &p.problem_id)}" }
                        }
                        {sparkline(&p.trend)}
                    }
                    div { class: "row",
                        span { class: "sc-badge sc-badge--neutral", "시도 {p.attempts}" }
                        span { class: "sc-badge sc-badge--success", "최고 {opt_score(p.best)}" }
                        span { class: "sc-badge sc-badge--neutral", "최근 {opt_score(p.latest)}" }
                        span { class: "muted small", "마지막 {ymd(p.last_submitted_at)}" }
                    }
                    if p.trend.len() > 1 {
                        p { class: "muted small trend", "추이 {trend_text(&p)}" }
                    }
                }
            }
        }
        h3 { "최근 제출" }
        ul { class: "plain",
            for a in recent {
                li { key: "{a.submission_id}", class: "row spread",
                    span { class: "desc", "{ymd(a.submitted_at)} · {a.problem_id}" }
                    {state_pill(&a.state)}
                }
            }
        }
    }
}

fn opt_score(score: Option<f64>) -> String {
    score.map_or_else(|| "—".into(), score_text)
}

fn trend_text(p: &ProblemRecord) -> String {
    p.trend
        .iter()
        .map(|s| format!("{:.0}", s * 100.0))
        .collect::<Vec<_>>()
        .join(" → ")
}

fn state_pill(state: &SubmissionState) -> Element {
    match state {
        SubmissionState::Done { report } => {
            rsx! { span { class: "sc-badge sc-badge--success", "{score_text(report.pass_rate())}" } }
        }
        SubmissionState::Queued => rsx! { span { class: "sc-badge sc-badge--info", "채점 대기" } },
        SubmissionState::Grading => rsx! { span { class: "sc-badge sc-badge--info", "채점 중" } },
        SubmissionState::Failed { reason } => {
            rsx! { span { class: "sc-badge sc-badge--danger", title: "{reason}", "채점 실패" } }
        }
        SubmissionState::Completed => rsx! { span { class: "sc-badge sc-badge--neutral", "완료" } },
    }
}

/// 점수 추이(0~100%) 꺾은선. 점이 하나면 점 하나.
fn sparkline(trend: &[f64]) -> Element {
    const W: f64 = 120.0;
    const H: f64 = 32.0;
    if trend.is_empty() {
        return rsx! {};
    }
    let n = trend.len();
    let points: Vec<(f64, f64)> = trend
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let x = if n > 1 {
                3.0 + i as f64 * (W - 6.0) / (n - 1) as f64
            } else {
                W / 2.0
            };
            (x, H - 3.0 - s.clamp(0.0, 1.0) * (H - 6.0))
        })
        .collect();
    let line = points
        .iter()
        .map(|(x, y)| format!("{x:.1},{y:.1}"))
        .collect::<Vec<_>>()
        .join(" ");
    let (lx, ly) = *points.last().expect("비지 않았다");
    rsx! {
        svg {
            class: "spark",
            width: "{W}",
            height: "{H}",
            view_box: "0 0 {W} {H}",
            role: "img",
            "aria-label": "점수 추이",
            line { x1: "0", y1: "{H - 2.0}", x2: "{W}", y2: "{H - 2.0}", class: "base" }
            polyline { class: "trend", points: "{line}", stroke_width: "2" }
            circle { class: "last", cx: "{lx}", cy: "{ly}", r: "3" }
        }
    }
}
