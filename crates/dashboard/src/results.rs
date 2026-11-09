//! 결과 화면(#10): 시험별 응시 현황·응시자별 점수(숨은 검사 통과율)·과정 지표(턴 수·프롬프트 길이·소요 시간),
//! CSV 내려받기, 사내 진단의 응시자 점수 공개 설정, 같은 구성원의 회차 비교.
//! 결과 보기 권한(관리자·감독관·열람자)만 연다 — 서버가 강제하고 화면은 같은 역할 표로 가릴 뿐이다.
//! 표 대신 줄 목록이라 모바일 폭에서도 가로로 넘치지 않는다.

use dioxus::prelude::*;
use promptlab_shared::api::{ExamView, MemberRounds, ResultRow};
use promptlab_shared::exam::{InviteStatus, Purpose};

use crate::route::{CandTab, OrgPage, Route};
use crate::{api, notice, shell};

/// 점수(0.0~1.0)를 「78.5점」으로.
pub fn score_text(score: f64) -> String {
    format!("{:.1}점", score * 100.0)
}

/// `m:ss`.
pub(crate) fn clock(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// 유닉스 초 → `YYYY-MM-DD`(UTC).
pub(crate) fn ymd(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn count(rows: &[ResultRow], status: InviteStatus) -> usize {
    rows.iter().filter(|r| r.status == status).count()
}

/// 평균(값이 하나도 없으면 없음).
fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0u32), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / f64::from(n))
}

/// 0~100 점수 → 「72.5」, 없으면 「—」.
pub(crate) fn points(v: Option<f64>) -> String {
    v.map_or_else(|| "—".into(), |v| format!("{v:.1}"))
}

/// 응시자가 문항을 시작할 때 쓴 AI 모델들(#131, 문항 순서, 겹치면 한 번) → 「Qwen2.5-Coder 7B」, 없으면 「—」.
pub(crate) fn models(row: &ResultRow) -> String {
    let mut names: Vec<&str> = Vec::new();
    for model in row.items.iter().filter_map(|i| i.model.as_deref()) {
        if !names.contains(&model) {
            names.push(model);
        }
    }
    if names.is_empty() {
        "—".into()
    } else {
        names.join(" · ")
    }
}

/// 응시자가 낸 언어들(문항 순서, 겹치면 한 번) → 「Python · Java」, 없으면 「—」.
pub(crate) fn languages(row: &ResultRow) -> String {
    let mut names: Vec<&str> = Vec::new();
    for language in row.items.iter().filter_map(|i| i.language) {
        let name = language.display_name();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if names.is_empty() {
        "—".into()
    } else {
        names.join(" · ")
    }
}

fn eval_scores(row: &ResultRow) -> impl Iterator<Item = f64> + '_ {
    row.items
        .iter()
        .filter_map(|i| i.score.as_ref().and_then(|s| s.evaluation_score))
}

fn status_badge(status: InviteStatus) -> &'static str {
    match status {
        InviteStatus::Invited => "sc-badge sc-badge--neutral",
        InviteStatus::InProgress => "sc-badge sc-badge--info",
        InviteStatus::Submitted => "sc-badge sc-badge--warning",
        InviteStatus::Graded => "sc-badge sc-badge--success",
    }
}

/// 거름 칩 하나(None = 전체).
const FILTERS: [Option<InviteStatus>; 5] = [
    None,
    Some(InviteStatus::Invited),
    Some(InviteStatus::InProgress),
    Some(InviteStatus::Submitted),
    Some(InviteStatus::Graded),
];

fn filter_label(f: Option<InviteStatus>) -> &'static str {
    f.map_or("전체", |s| s.label())
}

/// 시험 하나의 결과 칸: 요약 타일·거름 칩·응시자 표. 줄을 누르면 응시자 상세로 간다.
/// (응시자 점수 공개 설정은 시험 설정 탭 — 여기에는 두지 않는다.)
#[component]
pub fn ResultsPanel(org_id: String, exam: ExamView) -> Element {
    let ids = (org_id.clone(), exam.id.clone());
    let mut results = use_resource(move || {
        let (org_id, exam_id) = ids.clone();
        async move { api::exam_results(&org_id, &exam_id).await }
    });
    let mut filter = use_signal(|| None::<InviteStatus>);
    let csv = format!("{}/orgs/{}/exams/{}/results.csv", api::API, org_id, exam.id);

    let body = match &*results.read() {
        None => rsx! { p { class: "sc-panel-section muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(r)) if r.rows.is_empty() => {
            rsx! { p { class: "sc-panel-section muted", "아직 초대한 응시자가 없습니다." } }
        }
        Some(Ok(r)) => {
            let all = r.rows.clone();
            let retention_days = r.retention_days;
            let graded = count(&all, InviteStatus::Graded);
            let avg_final = mean(all.iter().filter_map(|r| r.final_score));
            let avg_hidden = mean(all.iter().filter_map(|r| r.score.map(|s| s * 100.0)));
            let avg_eval = mean(all.iter().flat_map(eval_scores));
            let total = all.len();
            let shown: Vec<ResultRow> = all
                .iter()
                .filter(|r| filter.read().is_none_or(|f| r.status == f))
                .cloned()
                .collect();
            let chips: Vec<(Option<InviteStatus>, usize)> = FILTERS
                .iter()
                .map(|f| (*f, f.map_or(total, |s| count(&all, s))))
                .collect();
            let org_for_rows = org_id.clone();
            let exam_for_rows = exam.id.clone();
            rsx! {
                dl { class: "sc-stat",
                    div { class: "sc-stat-item",
                        dt { class: "sc-stat-label", "응시자" }
                        dd { class: "sc-stat-value sc-num", "{total}" }
                        dd { class: "sc-stat-note", "채점 완료 {graded}" }
                    }
                    div { class: "sc-stat-item",
                        dt { class: "sc-stat-label", "평균 최종 점수" }
                        dd { class: "sc-stat-value sc-num", "{points(avg_final)}" }
                        dd { class: "sc-stat-note", "통과율 + AI 활용" }
                    }
                    div { class: "sc-stat-item",
                        dt { class: "sc-stat-label", "통과율" }
                        dd { class: "sc-stat-value sc-num", "{points(avg_hidden)}" }
                        dd { class: "sc-stat-note", "통과율 평균" }
                    }
                    div { class: "sc-stat-item",
                        dt { class: "sc-stat-label", "AI 활용" }
                        dd { class: "sc-stat-value sc-num", "{points(avg_eval)}" }
                        dd { class: "sc-stat-note", "평가가 끝난 문항 평균" }
                    }
                }
                div { class: "sc-toolbar",
                    div { class: "sc-toolbar-chips", role: "group", "aria-label": "상태 거름",
                        for (f , n) in chips {
                            button {
                                class: "sc-chip",
                                r#type: "button",
                                "aria-pressed": if *filter.read() == f { "true" } else { "false" },
                                onclick: move |_| filter.set(f),
                                "{filter_label(f)} "
                                span { class: "sc-chip-count", "{n}" }
                            }
                        }
                    }
                    span { class: "muted small", "보관 {retention_days}일 — 응시가 끝난 뒤 이 기간이 지나면 자동 삭제됩니다." }
                }
                if shown.is_empty() {
                    p { class: "sc-panel-section muted", "이 상태의 응시자가 없습니다." }
                } else {
                    div { class: "sc-table-scroll",
                        table { class: "sc-table",
                            thead {
                                tr {
                                    th { scope: "col", "응시자" }
                                    th { scope: "col", "상태" }
                                    th { scope: "col", class: "sc-num", "최종 점수" }
                                    th { scope: "col", class: "sc-num hide-sm", "통과율" }
                                    th { scope: "col", class: "sc-num hide-sm", "AI 활용" }
                                    th { scope: "col", class: "hide-sm", "언어" }
                                    th { scope: "col", class: "hide-sm", title: "응시자 PC에 맞춰 고른 AI 도우미 모델 — 크기에 따라 도우미 성능이 다릅니다", "AI 모델" }
                                    th { scope: "col", class: "hide-sm", "삭제까지" }
                                }
                            }
                            tbody {
                                for row in shown {
                                    {
                                        let target = Route::org(
                                            &org_for_rows,
                                            OrgPage::Candidate {
                                                exam: exam_for_rows.clone(),
                                                invite: row.invite_id.clone(),
                                                tab: CandTab::Conversations,
                                            },
                                        );
                                        let go_to = target.clone();
                                        let hidden = row.score.map(|s| s * 100.0);
                                        let eval = mean(eval_scores(&row));
                                        let langs = languages(&row);
                                        let used_models = models(&row);
                                        rsx! {
                                            tr {
                                                key: "{row.invite_id}",
                                                class: "sc-table-row-link",
                                                onclick: move |_| shell::go(go_to.clone()),
                                                td {
                                                    a {
                                                        href: "{target.href()}",
                                                        onclick: move |e| e.stop_propagation(),
                                                        "{row.email}"
                                                    }
                                                    if let Some(name) = &row.candidate_name {
                                                        span { class: "sc-table-sub", "{name}" }
                                                    }
                                                }
                                                td { span { class: status_badge(row.status), "{row.status.label()}" } }
                                                td { class: "sc-num", "{points(row.final_score)}" }
                                                td { class: "sc-num hide-sm", "{points(hidden)}" }
                                                td { class: "sc-num hide-sm", "{points(eval)}" }
                                                td { class: "hide-sm", "{langs}" }
                                                td { class: "hide-sm", "{used_models}" }
                                                td { class: "hide-sm",
                                                    if let Some(days) = row.delete_in_days {
                                                        span { class: "muted small", title: "보관 기간이 지나면 이 응시의 기록이 자동 삭제됩니다", "{days}일 뒤" }
                                                    } else {
                                                        span { class: "muted small", "—" }
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
            }
        }
    };

    rsx! {
        section { class: "sc-panel", "aria-label": "응시 결과",
            div { class: "sc-toolbar",
                if exam.purpose == Purpose::Recruiting {
                    span { class: "muted small", "채용 시험의 결과는 고객사만 봅니다. 응시자에게는 「응시 완료」만 보입니다." }
                } else {
                    span { class: "muted small", "응시자에게 점수를 보일지는 「문항과 채점」 탭의 결과 공개에서 정합니다." }
                }
                span { class: "row",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| results.restart(), "새로고침" }
                    a { class: "sc-btn sc-btn--secondary sc-btn--sm csv", href: "{csv}", download: "results.csv", "CSV 내려받기" }
                }
            }
            {body}
        }
    }
}

/// 조직의 사내 진단에서 같은 사람의 결과를 회차(시험)별로 시간순으로 비교한다.
#[component]
pub fn RoundsPanel(org_id: String) -> Element {
    let mut rounds = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::member_rounds(&org_id).await }
    });
    let body = match &*rounds.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(list)) if list.is_empty() => {
            rsx! { p { class: "muted", "아직 사내 진단을 배정받은 구성원이 없습니다." } }
        }
        Some(Ok(list)) => {
            let list: Vec<MemberRounds> = list.clone();
            rsx! {
                ul { class: "plain",
                    for m in list {
                        li { key: "{m.sub}",
                            div { class: "email",
                                "{m.name.clone().unwrap_or_else(|| m.sub.clone())}"
                            }
                            div { class: "chain",
                                for (i , r) in m.rounds.iter().enumerate() {
                                    if i > 0 {
                                        {delta(m.rounds[i - 1].score, r.score)}
                                    }
                                    span { class: "chip",
                                        "{r.exam_title} "
                                        span { class: "muted small", "{ymd(r.assigned_at)} " }
                                        strong {
                                            if let Some(s) = r.score { "{score_text(s)}" } else { "{r.status.label()}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    rsx! {
        section { class: "sc-panel sc-panel-section rounds",
            div { class: "row spread",
                h2 { class: "sc-panel-title", "회차 비교" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| rounds.restart(), "새로고침" }
            }
            p { class: "muted small", "사내 진단에서 같은 구성원(계정)의 시험별 점수를 배정받은 시간순으로 봅니다. 채용 시험은 넣지 않습니다." }
            {body}
        }
    }
}

/// 앞 회차 대비 점수 변화(둘 다 점수가 있을 때만 숫자, 아니면 화살표만).
fn delta(prev: Option<f64>, next: Option<f64>) -> Element {
    match (prev, next) {
        (Some(a), Some(b)) => {
            let d = (b - a) * 100.0;
            let class = if d > 0.05 {
                "delta up"
            } else if d < -0.05 {
                "delta down"
            } else {
                "delta"
            };
            rsx! { span { class: "{class}", "→ {d:+.1} →" } }
        }
        _ => rsx! { span { class: "delta muted", "→" } },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 날짜와_점수_글자() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1_767_225_600), "2026-01-01");
        assert_eq!(ymd(1_709_164_800), "2024-02-29");
        assert_eq!(score_text(0.785), "78.5점");
        assert_eq!(clock(72), "1:12");
    }
}
