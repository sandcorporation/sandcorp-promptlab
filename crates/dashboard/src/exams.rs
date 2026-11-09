//! 시험 화면(#8): 시험 목록·만들기(문제은행에서 문항 고르기·문항별 제한)·응시자 초대(일괄)·초대 현황,
//! 그리고 응시자가 초대 링크(`/exam-invite/{token}`)로 들어와 배정받는 화면.
//! 결과 보기 권한이면 배정받은 응시자의 문항별 과정 다시보기(#9, [`crate::replay`])와 의심 신호 목록
//! (#12, [`crate::signals`])을 연다.
//! 권한은 서버가 강제하고 화면은 같은 역할 표로 버튼을 숨길 뿐이다.

use dioxus::prelude::*;
use promptlab_shared::account::Permission;
use std::collections::HashMap;

use promptlab_shared::api::{
    AssignmentView, CreateExamRequest, ExamInviteView, ExamInvitesCreated, ExamView, OrgView,
};
use promptlab_shared::authoring::ORG_PROBLEM_PREFIX;
use promptlab_shared::coding::PublicCodingProblem;
use promptlab_shared::exam::ItemLimits;
use promptlab_shared::exam::{
    self, ExamItem, InviteStatus, Purpose, TIME_LIMIT_RANGE, split_emails,
};

use crate::exam_settings::{ExamSettingsPanel, GradingFields, SettingsDraft};
use crate::live::{LivePanel, WindowPanel};
use crate::results::{ResultsPanel, RoundsPanel};
use crate::route::{CandTab, ExamTab, OrgPage, Route};
use crate::shell::{PageHead, Tabs, use_crumb_tail};
use crate::signals::kst;
use crate::{api, notice};

/// 문항으로 넣을 때 처음 채우는 제한 시간(분) — 코딩 문제에는 문항 시간이 없어 출제자가 고친다.
const DEFAULT_ITEM_MINUTES: u32 = 30;

/// 시험 목록 한 줄의 진행 요약(화면이 결과·초대 현황·응시 가능 시간에서 모은다 — 볼 수 있는 것만).
#[derive(Debug, Clone, Default, PartialEq)]
struct ExamStats {
    /// 초대 수·제출(모든 문항을 낸) 수. 볼 권한이 없으면 `None`.
    invited: Option<usize>,
    submitted: Option<usize>,
    /// 응시자 최종 점수(0~100)의 평균.
    avg_final: Option<f64>,
    /// 응시 가능 시간의 끝(채용, 유닉스 초)과 지금 열려 있는가.
    ends_at: Option<i64>,
    closed: bool,
}

/// 시험 목록의 거름 칩.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    All,
    Running,
    Preparing,
    Ended,
}

impl Phase {
    const ALL: [Phase; 4] = [Phase::All, Phase::Running, Phase::Preparing, Phase::Ended];

    fn label(self) -> &'static str {
        match self {
            Phase::All => "전체",
            Phase::Running => "진행 중",
            Phase::Preparing => "준비 중",
            Phase::Ended => "끝남",
        }
    }

    /// 준비 중 = 아직 초대가 없다, 끝남 = 응시 가능 시간이 닫혔거나 지났다(채용), 나머지는 진행 중.
    fn of(stats: Option<&ExamStats>) -> Phase {
        match stats {
            Some(s) if s.closed => Phase::Ended,
            Some(s) if s.invited == Some(0) => Phase::Preparing,
            _ => Phase::Running,
        }
    }

    fn matches(self, stats: Option<&ExamStats>) -> bool {
        self == Phase::All || self == Phase::of(stats)
    }
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

/// 시험 하나의 요약을 모은다(권한이 없는 조회는 건너뛴다 — 실패도 빈칸).
async fn exam_stats(org: &OrgView, exam: &ExamView) -> ExamStats {
    let mut stats = ExamStats::default();
    if org.my_role.allows(Permission::ViewResults) {
        if let Ok(results) = api::exam_results(&org.id, &exam.id).await {
            stats.invited = Some(results.rows.len());
            stats.submitted = Some(
                results
                    .rows
                    .iter()
                    .filter(|r| matches!(r.status, InviteStatus::Submitted | InviteStatus::Graded))
                    .count(),
            );
            stats.avg_final = mean(results.rows.iter().filter_map(|r| r.final_score));
        }
    } else if org.my_role.allows(Permission::ManageExams)
        && let Ok(list) = api::exam_invites(&org.id, &exam.id).await
    {
        stats.invited = Some(list.len());
        stats.submitted = Some(
            list.iter()
                .filter(|i| matches!(i.status, InviteStatus::Submitted | InviteStatus::Graded))
                .count(),
        );
    }
    if exam.purpose == Purpose::Recruiting
        && org.my_role.allows(Permission::Proctor)
        && let Ok(w) = api::exam_window(&org.id, &exam.id).await
    {
        stats.ends_at = w.window.ends_at;
        stats.closed = !w.window.open || w.window.ends_at.is_some_and(|e| w.now >= e);
    }
    stats
}

/// 목적 배지.
fn purpose_badge(purpose: Purpose) -> Element {
    let tone = match purpose {
        Purpose::Recruiting => "sc-badge sc-badge--warning",
        Purpose::Internal => "sc-badge sc-badge--info",
    };
    rsx! { span { class: tone, "{purpose.label()}" } }
}

/// 시험 이름 아래 보조 줄: 점수 비율·공개.
fn exam_sub(exam: &ExamView) -> String {
    let w = exam.settings.score_weight;
    let reveal = match exam.purpose {
        Purpose::Internal if exam.reveal_scores => "점수 공개",
        Purpose::Internal => "점수 비공개",
        Purpose::Recruiting => "응시자에게 「응시 완료」만",
    };
    format!("점수 비율 {w} : {} · {reveal}", 100 - w as u32)
}

/// 시험 목록 페이지(시안 `WExams`): 거름 칩·찾기·표. 줄을 누르면 시험 상세.
#[component]
pub fn ExamsPage(org: OrgView) -> Element {
    let can_manage = org.my_role.allows(Permission::ManageExams);
    let org_id = org.id.clone();
    let exams = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::exams(&org_id).await }
    });
    let stats_org = org.clone();
    let stats = use_resource(move || {
        let org = stats_org.clone();
        let list = match &*exams.read() {
            Some(Ok(list)) => list.clone(),
            _ => Vec::new(),
        };
        async move {
            let mut out = HashMap::new();
            for exam in &list {
                out.insert(exam.id.clone(), exam_stats(&org, exam).await);
            }
            out
        }
    });
    let mut phase = use_signal(|| Phase::All);
    let mut query = use_signal(String::new);

    let new_exam = can_manage.then(|| {
        rsx! {
            a { class: "sc-btn sc-btn--accent sc-btn--sm", href: Route::org(&org.id, OrgPage::NewExam).href(), "＋ 새 시험" }
        }
    });
    let head = rsx! {
        PageHead {
            title: "시험",
            sub: "시험을 고르면 응시자·결과·설정을 한 화면에서 봅니다.",
            actions: new_exam,
        }
    };
    let list = match &*exams.read() {
        None => return rsx! { {head} p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => return rsx! { {head} {notice(e.to_string())} },
        Some(Ok(list)) => list.clone(),
    };
    let stats_map = stats.read().clone().unwrap_or_default();
    let count = |p: Phase| {
        list.iter()
            .filter(|e| p.matches(stats_map.get(&e.id)))
            .count()
    };
    let q = query.read().trim().to_lowercase();
    let shown: Vec<ExamView> = list
        .iter()
        .filter(|e| phase.read().matches(stats_map.get(&e.id)))
        .filter(|e| q.is_empty() || e.title.to_lowercase().contains(&q))
        .cloned()
        .collect();
    let loading_stats = stats.read().is_none();

    rsx! {
        {head}
        section { class: "sc-panel", "aria-label": "시험 목록",
            div { class: "sc-toolbar",
                div { class: "sc-toolbar-chips", role: "group", "aria-label": "상태 거름",
                    for p in Phase::ALL {
                        button {
                            class: "sc-chip",
                            r#type: "button",
                            "aria-pressed": if *phase.read() == p { "true" } else { "false" },
                            onclick: move |_| phase.set(p),
                            "{p.label()} "
                            span { class: "sc-chip-count", "{count(p)}" }
                        }
                    }
                }
                label { class: "sc-toolbar-search",
                    svg { width: "14", height: "14", view_box: "0 0 16 16", "aria-hidden": "true",
                        circle { cx: "7", cy: "7", r: "4.8", fill: "none", stroke: "currentColor", stroke_width: "1.4" }
                        path { d: "M10.5 10.5L14 14", stroke: "currentColor", stroke_width: "1.4", stroke_linecap: "round" }
                    }
                    span { class: "sc-visually-hidden", "시험 찾기" }
                    input {
                        r#type: "search",
                        placeholder: "시험 이름으로 찾기",
                        value: "{query}",
                        oninput: move |e| query.set(e.value()),
                    }
                }
            }
            if list.is_empty() {
                p { class: "sc-panel-section muted",
                    if can_manage { "아직 시험이 없습니다. 「＋ 새 시험」으로 만드세요." } else { "아직 시험이 없습니다." }
                }
            } else if shown.is_empty() {
                p { class: "sc-panel-section muted", "조건에 맞는 시험이 없습니다." }
            } else {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "시험" }
                                th { scope: "col", "목적" }
                                th { scope: "col", class: "sc-num", "문항" }
                                th { scope: "col", "응시 진행" }
                                th { scope: "col", class: "sc-num", "제출 / 초대" }
                                th { scope: "col", class: "sc-num", "평균 최종 점수" }
                                th { scope: "col", "마감" }
                            }
                        }
                        tbody {
                            for exam in shown {
                                ExamListRow {
                                    key: "{exam.id}",
                                    org_id: org.id.clone(),
                                    stats: stats_map.get(&exam.id).cloned(),
                                    loading: loading_stats,
                                    exam,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 시험 목록 한 줄. 줄 어디를 눌러도 첫 칸의 링크로 간다(링크는 키보드로도 닿는다).
#[component]
fn ExamListRow(org_id: String, exam: ExamView, stats: Option<ExamStats>, loading: bool) -> Element {
    let href = Route::org(
        &org_id,
        OrgPage::Exam {
            exam: exam.id.clone(),
            tab: ExamTab::Candidates,
        },
    );
    let dash = if loading { "…" } else { "—" };
    let s = stats.unwrap_or_default();
    let progress = match (s.invited, s.submitted) {
        (Some(i), Some(n)) if i > 0 => Some((n * 100 / i) as u32),
        _ => None,
    };
    let counts = match (s.submitted, s.invited) {
        (Some(n), Some(i)) => format!("{n} / {i}"),
        _ => dash.to_owned(),
    };
    let target = href.clone();
    rsx! {
        tr {
            class: "sc-table-row-link",
            onclick: move |_| crate::shell::go(target.clone()),
            td {
                a { href: href.href(), onclick: move |e| e.stop_propagation(), "{exam.title}" }
                span { class: "sc-table-sub", "{exam_sub(&exam)}" }
            }
            td { {purpose_badge(exam.purpose)} }
            td { class: "sc-num", "{exam.items.len()}" }
            td {
                if let Some(p) = progress {
                    div {
                        class: "sc-meter",
                        role: "meter",
                        "aria-valuenow": "{p}",
                        "aria-valuemin": "0",
                        "aria-valuemax": "100",
                        "aria-label": "응시 진행 {p}%",
                        span { class: "sc-meter-fill", style: "--sc-value: {p}%" }
                    }
                } else {
                    span { class: "muted", "{dash}" }
                }
            }
            td { class: "sc-num", "{counts}" }
            td { class: "sc-num",
                if let Some(v) = s.avg_final {
                    b { "{v:.1}" }
                } else {
                    "{dash}"
                }
            }
            td {
                if let Some(end) = s.ends_at {
                    "{kst(end * 1000)}"
                } else if s.closed {
                    "닫힘"
                } else {
                    "—"
                }
            }
        }
    }
}

/// 새 시험 페이지.
#[component]
pub fn NewExamPage(org: OrgView) -> Element {
    let org_id = org.id.clone();
    let back = Route::org(&org.id, OrgPage::Exams);
    if !org.my_role.allows(Permission::ManageExams) {
        return notice("시험을 만들 권한이 없습니다.".into());
    }
    rsx! {
        PageHead { title: "새 시험", sub: "문제은행에서 문항을 고르고 문항별 제한 시간을 정합니다." }
        ExamForm {
            org_id: org.id.clone(),
            on_created: move |exam: ExamView| {
                crate::shell::go(Route::org(&org_id, OrgPage::Exam { exam: exam.id, tab: ExamTab::Candidates }));
            },
            on_cancel: move |_| crate::shell::go(back.clone()),
        }
    }
}

/// 실시간 감독 페이지: 시험을 고르면 그 시험의 「실시간 감독」 탭(응시 가능 시간·진행 중 응시자 실시간 보기).
#[component]
pub fn LivePage(org: OrgView) -> Element {
    let org_id = org.id.clone();
    let exams = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::exams(&org_id).await }
    });
    let body = match &*exams.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(list)) if list.is_empty() => {
            rsx! { p { class: "sc-panel-section muted", "아직 시험이 없습니다." } }
        }
        Some(Ok(list)) => {
            let list = list.clone();
            rsx! {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "시험" }
                                th { scope: "col", "목적" }
                                th { scope: "col", "응시 가능 시간" }
                            }
                        }
                        tbody {
                            for exam in list {
                                {
                                    let href = Route::org(&org.id, OrgPage::Exam { exam: exam.id.clone(), tab: ExamTab::Live });
                                    let target = href.clone();
                                    rsx! {
                                        tr { class: "sc-table-row-link", onclick: move |_| crate::shell::go(target.clone()),
                                            td { a { href: href.href(), onclick: move |e| e.stop_propagation(), "{exam.title}" } }
                                            td { {purpose_badge(exam.purpose)} }
                                            td {
                                                if exam.purpose == Purpose::Recruiting {
                                                    "감독관이 열고 닫습니다"
                                                } else {
                                                    span { class: "muted", "사내 진단은 늘 열림" }
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
        PageHead { title: "실시간 감독", sub: "시험을 고르면 응시 가능 시간을 열고 닫고, 진행 중인 응시자의 웹캠을 실시간으로 봅니다." }
        section { class: "sc-panel", "aria-label": "시험", {body} }
    }
}

/// 고른 문항 하나(제한은 화면에서 분 단위로 고친다).
#[derive(Debug, Clone, PartialEq)]
struct Draft {
    problem: PublicCodingProblem,
    minutes: String,
}

impl Draft {
    fn item(&self) -> Result<ExamItem, String> {
        let minutes: u32 = self
            .minutes
            .trim()
            .parse()
            .map_err(|_| format!("「{}」 제한 시간(분)이 숫자가 아닙니다", self.problem.id))?;
        let limits = ItemLimits {
            time_limit_secs: minutes.saturating_mul(60),
        };
        exam::check_limits(&limits).map_err(|why| format!("「{}」 {why}", self.problem.id))?;
        Ok(ExamItem {
            problem_id: self.problem.id.clone(),
            limits,
        })
    }
}

fn short(text: &str, max: usize) -> String {
    let mut s: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        s.push('…');
    }
    s
}

/// 시험 만들기: 제목·용도·공개 설정·문제은행에서 문항 고르기·문항별 제한.
#[component]
fn ExamForm(
    org_id: String,
    on_created: EventHandler<ExamView>,
    on_cancel: EventHandler<()>,
) -> Element {
    let bank_org = org_id.clone();
    let bank = use_resource(move || {
        let org_id = bank_org.clone();
        async move { api::problem_bank(&org_id).await }
    });
    let mut title = use_signal(String::new);
    let mut purpose = use_signal(|| Purpose::Recruiting);
    let mut reveal = use_signal(|| false);
    let mut drafts = use_signal(Vec::<Draft>::new);
    let sdraft = use_signal(|| SettingsDraft::from_settings(&Default::default()));
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    let save = move |_| {
        let items: Result<Vec<ExamItem>, String> = drafts.read().iter().map(Draft::item).collect();
        let items = match items {
            Ok(items) if !items.is_empty() => items,
            Ok(_) => return error.set(Some("문항을 하나 이상 고르세요".into())),
            Err(e) => return error.set(Some(e)),
        };
        let settings = match sdraft.read().to_settings() {
            Ok(s) => s,
            Err(e) => return error.set(Some(e)),
        };
        let request = CreateExamRequest {
            title: title.read().trim().to_owned(),
            purpose: *purpose.read(),
            reveal_scores: *purpose.read() == Purpose::Internal && *reveal.read(),
            items,
            settings,
        };
        if request.title.is_empty() {
            return error.set(Some("시험 제목을 쓰세요".into()));
        }
        let org_id = org_id.clone();
        saving.set(true);
        spawn(async move {
            match api::create_exam(&org_id, &request).await {
                Ok(exam) => on_created.call(exam),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let bank_list = match &*bank.read() {
        None => rsx! { p { class: "muted", "문제은행 불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(problems)) => {
            let problems = problems.clone();
            rsx! {
                ul { class: "plain",
                    for p in problems {
                        li { class: "problem",
                            div {
                                strong { "{p.id}" }
                                if p.id.starts_with(ORG_PROBLEM_PREFIX) {
                                    " "
                                    span { class: "sc-badge sc-badge--neutral", "우리 조직" }
                                }
                                span { class: "muted", " {p.title} · 예제 {p.examples.len()}" }
                            }
                            p { class: "muted small", "{short(&p.description, 120)}" }
                            button {
                                class: "sc-btn sc-btn--secondary sc-btn--sm",
                                onclick: {
                                    let p = p.clone();
                                    move |_| {
                                        drafts.write().push(Draft {
                                            minutes: DEFAULT_ITEM_MINUTES.to_string(),
                                            problem: p.clone(),
                                        });
                                    }
                                },
                                "문항으로 넣기"
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        section { class: "sc-panel exam-form",
            if let Some(text) = error.read().clone() {
                {notice(text)}
            }
            label { class: "field",
                span { "제목" }
                input {
                    class: "sc-input",
                    placeholder: "예: 2026 하반기 개발직 프롬프트 역량",
                    value: "{title}",
                    oninput: move |e| title.set(e.value()),
                }
            }
            div { class: "row",
                label { class: "field inline",
                    span { "용도" }
                    select {
                        class: "sc-select",
                        onchange: move |e| {
                            if let Some(p) = Purpose::parse(&e.value()) {
                                purpose.set(p);
                            }
                        },
                        for p in Purpose::ALL {
                            option { value: p.as_str(), selected: p == *purpose.read(), "{p.label()}" }
                        }
                    }
                }
                if *purpose.read() == Purpose::Internal {
                    label { class: "row",
                        input {
                            r#type: "checkbox",
                            checked: *reveal.read(),
                            onchange: move |e| reveal.set(e.checked()),
                        }
                        span { "응시자에게 점수 공개" }
                    }
                }
            }
            p { class: "muted small",
                if *purpose.read() == Purpose::Recruiting {
                    "채용 시험의 결과는 고객사만 봅니다. 응시자에게는 「응시 완료」만 보입니다."
                } else {
                    "사내 진단은 공개를 켜면 응시자가 자기 점수를 봅니다."
                }
            }
            h3 { "문항 ({drafts.read().len()})" }
            if drafts.read().is_empty() {
                p { class: "muted", "아래 문제은행에서 문항을 넣으세요." }
            }
            ol { class: "items",
                for (i, d) in drafts.read().iter().cloned().enumerate() {
                    li { key: "{i}-{d.problem.id}",
                        div { class: "row",
                            strong { "{d.problem.id}" }
                            button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| { drafts.write().remove(i); }, "빼기" }
                        }
                        div { class: "row",
                            label { class: "field inline",
                                span { "제한 시간(분)" }
                                input {
                                    class: "sc-input num",
                                    r#type: "number",
                                    min: "{TIME_LIMIT_RANGE.start() / 60}",
                                    max: "{TIME_LIMIT_RANGE.end() / 60}",
                                    value: "{d.minutes}",
                                    oninput: move |e| {
                                        if let Some(d) = drafts.write().get_mut(i) {
                                            d.minutes = e.value();
                                        }
                                    },
                                }
                            }
                        }
                    }
                }
            }
            div { class: "row",
                button { class: "sc-btn sc-btn--accent sc-btn--sm", disabled: *saving.read(), onclick: save, "시험 만들기" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| on_cancel.call(()), "취소" }
            }
            h3 { "문제은행" }
            div { class: "bank-scroll", {bank_list} }
        }
        GradingFields { draft: sdraft, can_edit: true }
    }
}

/// 시험 상세 페이지: 시험 목록에서 찾아 상세를 그린다(시험 하나를 읽는 API가 따로 없다).
#[component]
pub fn ExamPage(org: OrgView, exam_id: String, tab: ExamTab) -> Element {
    let org_id = org.id.clone();
    let mut exams = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::exams(&org_id).await }
    });
    let exam = match &*exams.read() {
        None => return rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => return notice(e.to_string()),
        Some(Ok(list)) => list.iter().find(|e| e.id == exam_id).cloned(),
    };
    let Some(exam) = exam else {
        return rsx! {
            {notice("그런 시험이 없습니다.".into())}
            p { a { href: Route::org(&org.id, OrgPage::Exams).href(), "시험 목록으로" } }
        };
    };
    let role = org.my_role;
    let can_manage = role.allows(Permission::ManageExams);
    rsx! {
        ExamDetail {
            org_id: org.id.clone(),
            exam,
            tab,
            can_manage,
            can_see_status: can_manage || role.allows(Permission::ViewResults),
            can_replay: role.allows(Permission::ViewResults),
            can_proctor: role.allows(Permission::Proctor),
            can_erase: role.allows(Permission::ManageMembers),
            on_changed: move |_| exams.restart(),
        }
    }
}

/// 상세 안에서 볼 수 있는 탭.
fn visible_tabs(can_see_status: bool, can_replay: bool, can_proctor: bool) -> Vec<ExamTab> {
    ExamTab::ALL
        .into_iter()
        .filter(|t| match t {
            ExamTab::Candidates => can_see_status,
            ExamTab::Results | ExamTab::Rounds => can_replay,
            ExamTab::Items => true,
            ExamTab::Live => can_proctor,
        })
        .collect()
}

/// 응시자 표시(이메일 + 이름).
fn who(i: &ExamInviteView) -> String {
    match &i.candidate_name {
        Some(name) => format!("{name} <{}>", i.email),
        None => i.email.clone(),
    }
}

fn status_badge(status: InviteStatus) -> &'static str {
    match status {
        InviteStatus::Invited => "sc-badge sc-badge--neutral",
        InviteStatus::InProgress => "sc-badge sc-badge--info",
        InviteStatus::Submitted => "sc-badge sc-badge--warning",
        InviteStatus::Graded => "sc-badge sc-badge--success",
    }
}

/// 켜고 끄는 작은 동작 단추(열린 칸이면 눌린 꼴).
fn toggle_btn(label: &str, on: bool, onclick: EventHandler<MouseEvent>) -> Element {
    rsx! {
        button {
            class: "sc-btn sc-btn--secondary sc-btn--sm",
            r#type: "button",
            "aria-pressed": if on { "true" } else { "false" },
            onclick: move |e| onclick.call(e),
            "{label}"
        }
    }
}

const INVITE_DRAWER: &str = "invite-drawer";

/// 고른 시험: 머리(제목·목적·응시자 초대) + 탭.
#[component]
fn ExamDetail(
    org_id: String,
    exam: ExamView,
    tab: ExamTab,
    can_manage: bool,
    can_see_status: bool,
    can_replay: bool,
    can_proctor: bool,
    can_erase: bool,
    on_changed: EventHandler<()>,
) -> Element {
    let mut crumb = use_crumb_tail();
    let title = exam.title.clone();
    use_effect(move || crumb.set(Some(title.clone())));
    // 열린 실시간 보기(#14): (초대 id, 응시자 표시). 감독 권한만.
    let mut live = use_signal(|| None::<(String, String)>);
    // 응시자 상세(#75 C)로 옮긴 칸 — 의심 신호·AI 리포트·웹캠·다시보기·데이터 삭제는 그 페이지 탭.
    let status_ids = (org_id.clone(), exam.id.clone());
    let mut invites = use_resource(move || {
        let (org_id, exam_id) = status_ids.clone();
        async move {
            if can_see_status {
                api::exam_invites(&org_id, &exam_id).await.map(Some)
            } else {
                Ok(None)
            }
        }
    });

    let tabs = visible_tabs(can_see_status, can_replay, can_proctor);
    let tab = if tabs.contains(&tab) {
        tab
    } else {
        ExamTab::Items
    };
    let tab_links: Vec<(String, Route)> = tabs
        .iter()
        .map(|t| {
            (
                t.label().to_owned(),
                Route::org(
                    &org_id,
                    OrgPage::Exam {
                        exam: exam.id.clone(),
                        tab: *t,
                    },
                ),
            )
        })
        .collect();
    let here = Route::org(
        &org_id,
        OrgPage::Exam {
            exam: exam.id.clone(),
            tab,
        },
    );

    let list: Option<Vec<ExamInviteView>> = match &*invites.read() {
        Some(Ok(Some(list))) => Some(list.clone()),
        _ => None,
    };
    let invites_state = match &*invites.read() {
        None => Some(rsx! { p { class: "sc-panel-section muted", "불러오는 중…" } }),
        Some(Err(e)) => Some(notice(e.to_string())),
        _ => None,
    };

    let content = match tab {
        ExamTab::Candidates => {
            let rows = list.clone().unwrap_or_default();
            rsx! {
                section { class: "sc-panel", "aria-label": "응시자",
                    div { class: "sc-toolbar",
                        span { class: "muted small", "응시자 {rows.len()}명" }
                        button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| invites.restart(), "새로고침" }
                    }
                    if let Some(state) = invites_state.clone() {
                        {state}
                    } else if rows.is_empty() {
                        p { class: "sc-panel-section muted",
                            if can_manage { "아직 초대한 응시자가 없습니다. 「응시자 초대」로 이메일을 넣으세요." } else { "아직 초대한 응시자가 없습니다." }
                        }
                    } else {
                        div { class: "sc-table-scroll",
                            table { class: "sc-table",
                                thead {
                                    tr {
                                        th { scope: "col", "응시자" }
                                        th { scope: "col", "상태" }
                                        th { scope: "col", "전달" }
                                    }
                                }
                                tbody {
                                    for i in rows {
                                        {
                                            let detail = (can_replay && i.status != InviteStatus::Invited)
                                                .then(|| Route::org(
                                                    &org_id,
                                                    OrgPage::Candidate {
                                                        exam: exam.id.clone(),
                                                        invite: i.id.clone(),
                                                        tab: CandTab::Conversations,
                                                    },
                                                ));
                                            rsx! {
                                                tr { key: "{i.id}",
                                                    td {
                                                        if let Some(to) = detail {
                                                            a { href: "{to.href()}", "{i.email}" }
                                                        } else {
                                                            "{i.email}"
                                                        }
                                                        if let Some(name) = &i.candidate_name {
                                                            span { class: "sc-table-sub", "{name}" }
                                                        }
                                                    }
                                                    td { span { class: status_badge(i.status), "{i.status.label()}" } }
                                                    td { span { class: "muted small", "{i.delivery.label()}" } }
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
        ExamTab::Results => rsx! {
            ResultsPanel {
                key: "results-{exam.id}",
                org_id: org_id.clone(),
                exam: exam.clone(),
            }
        },
        ExamTab::Items => rsx! {
            ExamSettingsPanel {
                key: "{exam.id}",
                org_id: org_id.clone(),
                exam: exam.clone(),
                can_edit: can_manage,
                on_saved: move |_| on_changed.call(()),
            }
        },
        ExamTab::Rounds => rsx! { RoundsPanel { org_id: org_id.clone() } },
        ExamTab::Live => {
            let running: Vec<ExamInviteView> = list
                .clone()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.status == InviteStatus::InProgress)
                .collect();
            rsx! {
                if exam.purpose == Purpose::Recruiting {
                    WindowPanel { key: "window-{exam.id}", org_id: org_id.clone(), exam_id: exam.id.clone() }
                } else {
                    p { class: "muted small", "사내 진단은 응시 가능 시간을 열고 닫지 않습니다(늘 열림)." }
                }
                section { class: "sc-panel", "aria-label": "진행 중인 응시자",
                    div { class: "sc-toolbar",
                        span { class: "muted small", "진행 중 {running.len()}명" }
                        button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| invites.restart(), "새로고침" }
                    }
                    if let Some(state) = invites_state.clone() {
                        {state}
                    } else if running.is_empty() {
                        p { class: "sc-panel-section muted", "지금 문항을 푸는 응시자가 없습니다." }
                    } else {
                        div { class: "sc-table-scroll",
                            table { class: "sc-table",
                                thead {
                                    tr {
                                        th { scope: "col", "응시자" }
                                        th { scope: "col", span { class: "sc-visually-hidden", "동작" } }
                                    }
                                }
                                tbody {
                                    for i in running {
                                        tr { key: "{i.id}",
                                            td {
                                                "{i.email}"
                                                if let Some(name) = &i.candidate_name {
                                                    span { class: "sc-table-sub", "{name}" }
                                                }
                                            }
                                            td { class: "sc-num",
                                                {
                                                    let id = i.id.clone();
                                                    let w = who(&i);
                                                    let on = live.read().as_ref().is_some_and(|(l, _)| *l == id);
                                                    toggle_btn("실시간 보기", on, EventHandler::new(move |_| live.set(Some((id.clone(), w.clone())))))
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some((invite_id, candidate)) = live.read().clone() {
                    LivePanel {
                        key: "live-{invite_id}",
                        org_id: org_id.clone(),
                        exam_id: exam.id.clone(),
                        invite_id,
                        candidate,
                        on_close: move |_| live.set(None),
                    }
                }
            }
        }
    };

    let invite_button = can_manage.then(|| {
        rsx! {
            button {
                class: "sc-btn sc-btn--accent sc-btn--sm",
                r#type: "button",
                onclick: move |_| api::show_modal(INVITE_DRAWER),
                "응시자 초대"
            }
        }
    });
    rsx! {
        PageHead {
            title: exam.title.clone(),
            sub: format!("문항 {} · {}", exam.items.len(), exam_sub(&exam)),
            badges: purpose_badge(exam.purpose),
            actions: invite_button,
        }
        Tabs { label: "시험 화면", tabs: tab_links, current: here }
        {content}
        if can_manage {
            InviteDrawer {
                org_id: org_id.clone(),
                exam: exam.clone(),
                on_sent: move |_| invites.restart(),
            }
        }
    }
}

/// 이메일 토큰 하나가 맞는 주소인가(서버와 같은 규칙 — [`split_emails`]).
fn email_ok(token: &str) -> bool {
    let (valid, invalid) = split_emails(token);
    !valid.is_empty() && invalid.is_empty()
}

/// 입력 칸의 글을 토큰과 남은 조각으로 나눈다(쉼표·세미콜론·공백·줄바꿈). `commit`이면 남은 조각도 토큰으로.
fn take_tokens(text: &str, commit: bool) -> (Vec<String>, String) {
    let ends_with_sep = text
        .chars()
        .last()
        .is_some_and(|c| c == ',' || c == ';' || c.is_whitespace());
    let mut parts: Vec<String> = text
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    if commit || ends_with_sep {
        return (parts, String::new());
    }
    let rest = parts.pop().unwrap_or_default();
    (parts, rest)
}

/// 응시자 초대 서랍(시안 `WInvite`): 이메일 토큰(Enter·쉼표·붙여 넣기로 나눔, 틀린 주소는 빨강) → 「n명 초대」 →
/// 같은 서랍에서 링크(메일이 꺼졌거나 실패하면 이것으로 전달). 닫아도 보던 탭은 그대로.
#[component]
fn InviteDrawer(org_id: String, exam: ExamView, on_sent: EventHandler<()>) -> Element {
    let mut tokens = use_signal(Vec::<String>::new);
    let mut field = use_signal(String::new);
    let mut sent = use_signal(|| None::<ExamInvitesCreated>);
    let mut error = use_signal(|| None::<String>);
    let mut sending = use_signal(|| false);

    let mut push = move |text: &str, commit: bool| {
        let (new, rest) = take_tokens(text, commit);
        if !new.is_empty() {
            let mut list = tokens.write();
            for t in new {
                if !list.contains(&t) {
                    list.push(t);
                }
            }
        }
        field.set(rest);
    };
    let valid: Vec<String> = tokens
        .read()
        .iter()
        .filter(|t| email_ok(t))
        .cloned()
        .collect();
    let invalid = tokens.read().len() - valid.len();

    let ids = (org_id.clone(), exam.id.clone());
    let send = move |_| {
        let (org_id, exam_id) = ids.clone();
        let emails: Vec<String> = tokens
            .read()
            .iter()
            .filter(|t| email_ok(t))
            .cloned()
            .collect();
        if emails.is_empty() {
            return;
        }
        error.set(None);
        sending.set(true);
        spawn(async move {
            match api::invite_to_exam(&org_id, &exam_id, emails).await {
                Ok(created) => {
                    tokens.write().retain(|t| !email_ok(t));
                    sent.set(Some(created));
                    on_sent.call(());
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            sending.set(false);
        });
    };
    let close = move |_| {
        sent.set(None);
        error.set(None);
        api::close_modal(INVITE_DRAWER);
    };

    let origin = api::origin();
    rsx! {
        dialog { class: "sc-drawer", id: INVITE_DRAWER, "aria-labelledby": "invite-title",
            div { class: "sc-drawer-head",
                h2 { class: "sc-drawer-title", id: "invite-title", "응시자 초대" }
                button { class: "sc-icon-btn", r#type: "button", "aria-label": "닫기", onclick: close, "✕" }
            }
            div { class: "sc-drawer-body",
                p { class: "muted small", "{exam.title}" }
                if let Some(text) = error.read().clone() {
                    {notice(text)}
                }
                if let Some(created) = sent.read().clone() {
                    div { class: "sc-alert", role: "status", "{created.invites.len()}명에게 초대를 만들었습니다." }
                    if !created.rejected.is_empty() {
                        {notice(format!("주소가 틀려 빼고 보냈습니다: {}", created.rejected.join(", ")))}
                    }
                    ul { class: "sent",
                        for c in created.invites {
                            li { key: "{c.id}",
                                div { class: "between",
                                    span { "{c.email}" }
                                    span { class: "muted small",
                                        if c.already_assigned { "이미 배정됨" } else { "{c.delivery.label()}" }
                                    }
                                }
                                if let Some(token) = &c.token {
                                    {
                                        let link = format!("{origin}/exam-invite/{token}");
                                        rsx! {
                                            div { class: "row",
                                                input { class: "sc-input linkbox", readonly: true, "aria-label": "초대 링크", value: "{link}" }
                                                button { class: "sc-btn sc-btn--secondary sc-btn--sm", r#type: "button", onclick: move |_| api::copy_text(&link), "링크 복사" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    p { class: "muted small", "링크는 14일 동안 쓸 수 있고 처음 들어온 한 사람에게만 배정됩니다. 다시 초대하면 새 링크가 나가고 옛 링크는 죽습니다." }
                } else {
                    p { class: "muted small", "이메일을 붙여 넣으면 하나씩 확인합니다. 링크를 연 사람이 카카오로 로그인하면 이 시험이 배정됩니다." }
                    div { class: "sc-field",
                        label { class: "sc-field-label", r#for: "invite-emails", "이메일" }
                        div { class: "sc-token-input",
                            for t in tokens.read().iter().cloned() {
                                {
                                    let ok = email_ok(&t);
                                    let gone = t.clone();
                                    rsx! {
                                        span { key: "{t}", class: if ok { "sc-token" } else { "sc-token sc-token--invalid" },
                                            "{t}"
                                            button {
                                                class: "sc-token-remove",
                                                r#type: "button",
                                                "aria-label": "{t} 지우기",
                                                onclick: move |_| tokens.write().retain(|x| *x != gone),
                                                "×"
                                            }
                                        }
                                    }
                                }
                            }
                            input {
                                class: "sc-token-input-field",
                                id: "invite-emails",
                                r#type: "text",
                                inputmode: "email",
                                autocomplete: "off",
                                placeholder: "이메일 붙여 넣기…",
                                "aria-describedby": "invite-emails-err",
                                value: "{field}",
                                oninput: move |e| push(&e.value(), false),
                                onkeydown: move |e: KeyboardEvent| {
                                    match e.key() {
                                        Key::Enter => {
                                            e.prevent_default();
                                            let text = field.read().clone();
                                            push(&text, true);
                                        }
                                        Key::Backspace if field.read().is_empty() => {
                                            tokens.write().pop();
                                        }
                                        _ => {}
                                    }
                                },
                                onblur: move |_| {
                                    let text = field.read().clone();
                                    push(&text, true);
                                },
                            }
                        }
                        if invalid > 0 {
                            span { class: "sc-field-error", id: "invite-emails-err",
                                "{invalid}개는 이메일 형식이 아닙니다 — 고치거나 빼 주세요."
                            }
                        }
                    }
                    p { class: "muted small", "메일 발송이 꺼져 있거나 보내지 못하면 초대 뒤 여기 나오는 링크를 복사해 전달하세요." }
                }
            }
            div { class: "sc-drawer-foot",
                if sent.read().is_some() {
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", r#type: "button", onclick: move |_| sent.set(None), "더 초대하기" }
                    button { class: "sc-btn sc-btn--accent sc-btn--sm", r#type: "button", onclick: close, "닫기" }
                } else {
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", r#type: "button", onclick: close, "취소" }
                    button {
                        class: "sc-btn sc-btn--accent sc-btn--sm",
                        r#type: "button",
                        disabled: valid.is_empty() || *sending.read(),
                        onclick: send,
                        if *sending.read() { "보내는 중…" } else { "{valid.len()}명 초대" }
                    }
                }
            }
        }
    }
}

/// 응시자가 초대 링크로 들어온 화면: 로그인한 뒤 배정을 받는다.
#[component]
pub fn ExamInvitePage(token: String) -> Element {
    let result = use_resource(move || {
        let token = token.clone();
        async move { api::accept_exam_invite(&token).await }
    });
    match &*result.read() {
        None => rsx! { p { class: "muted", "시험을 배정받는 중…" } },
        Some(Err(e)) => notice(format!(
            "시험을 배정받지 못했습니다: {e}. 링크가 만료됐거나 다른 계정으로 이미 받은 링크일 수 있습니다 — 담당자에게 다시 초대를 요청하세요."
        )),
        Some(Ok(a)) => assigned(a),
    }
}

fn assigned(a: &AssignmentView) -> Element {
    let minutes: u32 = a.items.iter().map(|i| i.limits.time_limit_secs / 60).sum();
    rsx! {
        PageHead {
            title: "시험이 배정됐습니다",
            sub: format!("{} · {} · 문항 {}개 · 제한 시간 합계 {minutes}분 · {}", a.exam_title, a.org_name, a.items.len(), a.purpose.label()),
        }
        section { class: "sc-panel sc-panel-section",
            ol {
                li { "promptlab 응시 앱을 설치하고 엽니다." }
                li { "「내 시험 불러오기」를 누르고 같은 카카오 계정으로 로그인합니다." }
                li { "이 시험의 문항을 하나씩 응시합니다. 문항을 시작하면 제한 시간이 흐릅니다." }
            }
            if a.scores_visible {
                p { class: "muted small", "이 시험은 제출 뒤 내 점수를 볼 수 있습니다." }
            } else {
                p { class: "muted small", "이 시험의 결과는 시험을 낸 회사만 봅니다. 내 계정에는 「응시 완료」만 남습니다." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 이메일_칸은_구분자에서_토큰으로_나눈다() {
        assert_eq!(
            take_tokens("a@x.com", false),
            (vec![], "a@x.com".to_owned())
        );
        assert_eq!(
            take_tokens("a@x.com, b@y.com", false),
            (vec!["a@x.com".to_owned()], "b@y.com".to_owned())
        );
        assert_eq!(
            take_tokens("a@x.com\nb@y.com\n", false),
            (
                vec!["a@x.com".to_owned(), "b@y.com".to_owned()],
                String::new()
            )
        );
        assert_eq!(
            take_tokens("a@x.com", true),
            (vec!["a@x.com".to_owned()], String::new())
        );
        assert!(email_ok("a@x.com"));
        assert!(!email_ok("yoon@example"));
    }

    #[test]
    fn 거름_칩은_초대와_응시_가능_시간으로_정한다() {
        assert_eq!(Phase::of(None), Phase::Running);
        let none = ExamStats {
            invited: Some(0),
            ..Default::default()
        };
        assert_eq!(Phase::of(Some(&none)), Phase::Preparing);
        let closed = ExamStats {
            invited: Some(3),
            closed: true,
            ..Default::default()
        };
        assert_eq!(Phase::of(Some(&closed)), Phase::Ended);
        assert!(Phase::All.matches(Some(&closed)));
        assert!(!Phase::Running.matches(Some(&closed)));
    }
}
