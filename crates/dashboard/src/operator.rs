//! 운영자 화면(#19): 대표 계정에게만 메뉴가 보인다([`MeView::is_owner`](promptlab_shared::api::MeView)) — 권한은
//! 서버가 강제한다(`/operator/*`, 그 밖의 주체 403). 네 칸: 시범 조직 승인, 문제은행(운영자 문항 만들기·고치기,
//! 내장·운영자 문항 내리기), 채점 큐·비용, 허용 목록(상수 ∪ 더한 값 − 막은 값, 변경 이력).

use dioxus::prelude::*;
use promptlab_shared::authoring::OrgProblem;
use promptlab_shared::integrity::{AllowAction, AllowKind};
use promptlab_shared::operator::{
    AllowlistChangeRequest, AllowlistView, BankProblemSummary, OperatorMetrics, OperatorOrg,
    ProblemOrigin, QueueStats,
};

use crate::problem_editor::ProblemEditor;
use crate::problems::{EditTarget, short};
use crate::route::{OperatorTab, Route};
use crate::shell::{PageHead, Tabs};
use crate::signals::kst;
use crate::usage::{kind_table, thousands, usd};
use crate::{api, notice};

fn when(secs: i64) -> String {
    kst(secs * 1000)
}

#[component]
pub fn OperatorPanel(tab: OperatorTab) -> Element {
    let tabs: Vec<(String, Route)> = OperatorTab::ALL
        .into_iter()
        .map(|t| (t.label().to_owned(), Route::Operator(t)))
        .collect();
    rsx! {
        PageHead { title: "운영자", sub: "시범 조직 승인·문제은행·큐와 비용·허용 목록. 대표 계정만 봅니다." }
        Tabs { label: "운영자 화면", tabs, current: Route::Operator(tab) }
        match tab {
            OperatorTab::Orgs => rsx! { OrgsTab {} },
            OperatorTab::Problems => rsx! { ProblemsTab {} },
            OperatorTab::Metrics => rsx! { MetricsTab {} },
            OperatorTab::Allowlist => rsx! { AllowlistTab {} },
        }
    }
}

// ── 시범 조직 ──

#[component]
fn OrgsTab() -> Element {
    let mut list = use_resource(api::operator_orgs);
    let mut error = use_signal(|| None::<String>);
    let body = match &*list.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(orgs)) if orgs.is_empty() => rsx! { p { class: "muted", "아직 조직이 없습니다." } },
        Some(Ok(orgs)) => {
            let orgs: Vec<OperatorOrg> = orgs.clone();
            rsx! {
                ul { class: "plain",
                    for org in orgs {
                        li { class: "row spread",
                            div { class: "desc",
                                strong { "{org.name}" }
                                if org.approved_at.is_some() {
                                    span { class: "sc-badge sc-badge--success", "승인" }
                                } else {
                                    span { class: "sc-badge sc-badge--info", "승인 대기" }
                                }
                                p { class: "muted small",
                                    "구성원 {org.members} · 시험 {org.exams} · 만듦 {when(org.created_at)}"
                                    if let Some(at) = org.approved_at { " · 승인 {when(at)}" }
                                }
                            }
                            button {
                                class: if org.approved_at.is_none() { "sc-btn sc-btn--accent sc-btn--sm" } else { "sc-btn sc-btn--secondary sc-btn--sm" },
                                onclick: {
                                    let id = org.id.clone();
                                    let approve = org.approved_at.is_none();
                                    move |_| {
                                        let id = id.clone();
                                        spawn(async move {
                                            match api::set_org_approval(&id, approve).await {
                                                Ok(_) => {
                                                    error.set(None);
                                                    list.restart();
                                                }
                                                Err(e) => error.set(Some(e.to_string())),
                                            }
                                        });
                                    }
                                },
                                if org.approved_at.is_none() { "승인" } else { "승인 거두기" }
                            }
                        }
                    }
                }
            }
        }
    };
    rsx! {
        section { class: "sc-panel sc-panel-section",
            h2 { class: "sc-panel-title", "시범 조직 승인" }
            p { class: "muted small",
                "승인 전 조직은 시험을 만들 수 없고 응시자가 문항을 시작할 수 없습니다. 문제 만들기·구성원 초대는 승인 전에도 됩니다. 승인을 거두면 이미 만든 시험도 새 문항을 시작하지 못합니다(진행 중인 문항은 그대로)."
            }
            if let Some(text) = error.read().clone() {
                {notice(text)}
            }
            {body}
        }
    }
}

// ── 문제은행 ──

#[derive(Debug, Clone, PartialEq)]
struct Editing {
    practice: bool,
    initial: Option<OrgProblem>,
}

#[component]
fn ProblemsTab() -> Element {
    let mut list = use_resource(api::operator_problems);
    let mut editing = use_signal(|| None::<Editing>);
    let mut error = use_signal(|| None::<String>);
    let mut reason = use_signal(String::new);

    if let Some(Editing { practice, initial }) = editing.read().clone() {
        return rsx! {
            ProblemEditor {
                key: "{initial.as_ref().map(|p| p.id.as_str()).unwrap_or(\"new\")}-{practice}",
                target: EditTarget::Operator { practice },
                initial,
                on_saved: move |_| list.restart(),
                on_close: move |_| {
                    list.restart();
                    editing.set(None);
                },
            }
        };
    }

    let body = match &*list.read() {
        None => rsx! { p { class: "sc-panel-section muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(items)) => {
            let items: Vec<BankProblemSummary> = items.clone();
            rsx! {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "문항" }
                                th { scope: "col", "출처·용도" }
                                th { scope: "col", class: "sc-num", "예제" }
                                th { scope: "col", class: "sc-num", "히든 테스트" }
                                th { scope: "col", "상태" }
                                th { scope: "col", class: "sc-num", span { class: "sc-visually-hidden", "동작" } }
                            }
                        }
                        tbody {
                            for p in items {
                                tr { key: "{p.id}",
                                    td {
                                        "{short(&p.title, 60)}"
                                        span { class: "sc-table-sub", "{p.id}" }
                                        if let Some(why) = p.retired_reason.clone().filter(|w| !w.is_empty()) {
                                            span { class: "sc-table-sub", "내린 까닭: {why}" }
                                        }
                                    }
                                    td {
                                        span { class: "sc-badge sc-badge--neutral", if p.origin == ProblemOrigin::Builtin { "내장" } else { "운영자" } }
                                        " "
                                        span { class: "sc-badge sc-badge--neutral", if p.practice { "연습" } else { "시험" } }
                                    }
                                    td { class: "sc-num", "{p.example_count}" }
                                    td { class: "sc-num", "{p.hidden_count}" }
                                    td {
                                        if let Some(at) = p.retired_at {
                                            span { class: "sc-badge sc-badge--danger", "내림 {when(at)}" }
                                        } else if p.published {
                                            span { class: "sc-badge sc-badge--success", "발행됨" }
                                        } else {
                                            span { class: "sc-badge sc-badge--neutral", "초안" }
                                        }
                                    }
                                    td { class: "sc-num",
                                        div { class: "actions",
                                            if p.origin == ProblemOrigin::Operator {
                                                button {
                                                    class: "sc-btn sc-btn--secondary sc-btn--sm",
                                                    onclick: {
                                                        let id = p.id.clone();
                                                        move |_| {
                                                            let id = id.clone();
                                                            spawn(async move {
                                                                match api::operator_problem(&id).await {
                                                                    Ok(b) => {
                                                                        error.set(None);
                                                                        editing.set(Some(Editing {
                                                                            practice: b.practice,
                                                                            initial: Some(OrgProblem {
                                                                                id: b.id,
                                                                                source: b.source,
                                                                                updated_at: b.updated_at,
                                                                                publication: b.publication,
                                                                            }),
                                                                        }));
                                                                    }
                                                                    Err(e) => error.set(Some(e.to_string())),
                                                                }
                                                            });
                                                        }
                                                    },
                                                    "열기"
                                                }
                                            }
                                            button {
                                                class: "sc-btn sc-btn--secondary sc-btn--sm",
                                                onclick: {
                                                    let id = p.id.clone();
                                                    let retire = p.retired_at.is_none();
                                                    move |_| {
                                                        let id = id.clone();
                                                        let why = reason.read().trim().to_owned();
                                                        spawn(async move {
                                                            match api::set_retirement(&id, retire, &why).await {
                                                                Ok(_) => {
                                                                    error.set(None);
                                                                    list.restart();
                                                                }
                                                                Err(e) => error.set(Some(e.to_string())),
                                                            }
                                                        });
                                                    }
                                                },
                                                if p.retired_at.is_none() { "내리기" } else { "되살리기" }
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
        section { class: "sc-panel", "aria-label": "문제은행",
            div { class: "sc-toolbar",
                span { class: "muted small",
                    "내장 문항은 저장소 파일이 정본이라 여기서는 내리기만 합니다. 운영자 문항은 점검을 통과해 발행해야 모든 조직의 문제은행에 들어갑니다. 내린 문항은 새 시험·연습 목록에서 빠지고, 이미 만든 시험과 과거 결과는 그대로입니다."
                }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| editing.set(Some(Editing { practice: false, initial: None })), "＋ 새 시험 문항" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| editing.set(Some(Editing { practice: true, initial: None })), "＋ 새 연습 문항" }
            }
            div { class: "sc-panel-section",
                div { class: "sc-field",
                    label { class: "sc-field-label", r#for: "retire-reason", "내리는 까닭(선택, 다음 「내리기」에 붙는다)" }
                    input {
                        id: "retire-reason",
                        class: "sc-input",
                        value: "{reason}",
                        placeholder: "예: 유출",
                        oninput: move |e| reason.set(e.value()),
                    }
                }
            }
            if let Some(text) = error.read().clone() {
                {notice(text)}
            }
            {body}
        }
    }
}

// ── 큐·비용 ──

fn secs(v: Option<f64>) -> String {
    match v {
        None => "—".into(),
        Some(s) if s < 60.0 => format!("{s:.1}초"),
        Some(s) => format!("{:.1}분", s / 60.0),
    }
}

fn queue_row(q: &QueueStats) -> Element {
    let oldest = q
        .oldest_waiting_secs
        .map(|s| format!(" · 가장 오래 {}", secs(Some(s as f64))))
        .unwrap_or_default();
    rsx! {
        li {
            div { class: "row spread",
                strong { "{q.kind.label()}" }
                span {
                    span { class: if q.waiting > 0 { "sc-badge sc-badge--info" } else { "sc-badge sc-badge--neutral" }, "대기 {q.waiting}" }
                    " "
                    span { class: "sc-badge sc-badge--neutral", "처리 중 {q.running}" }
                }
            }
            p { class: "muted small",
                "끝남 {q.done} · "
                span { class: if q.failed > 0 { "fail" } else { "" }, "실패 {q.failed}" }
                " · 평균 대기 {secs(q.avg_wait_secs)} · 평균 처리 {secs(q.avg_run_secs)}{oldest}"
            }
        }
    }
}

#[component]
fn MetricsTab() -> Element {
    let mut metrics = use_resource(api::operator_metrics);
    let body = match &*metrics.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(m)) => metrics_body(m),
    };
    rsx! {
        section { class: "sc-panel sc-panel-section",
            div { class: "row spread",
                h2 { class: "sc-panel-title", "채점 큐·비용" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| metrics.restart(), "새로고침" }
            }
            {body}
        }
    }
}

fn metrics_body(m: &OperatorMetrics) -> Element {
    rsx! {
        h3 { "큐 (끝남·실패·평균은 최근 {m.window_hours}시간)" }
        ul { class: "plain queues",
            for q in m.queues.iter() {
                {queue_row(q)}
            }
        }
        h3 { "최근 실패" }
        if m.failures.is_empty() {
            p { class: "muted", "없습니다." }
        }
        ul { class: "plain",
            for f in m.failures.iter() {
                li {
                    p { class: "desc", "{f.reason}" }
                    span { class: "muted small", "{f.kind.label()} · {f.org.clone().unwrap_or_else(|| \"개인 연습\".into())} · {when(f.at)}" }
                }
            }
        }
        h3 { "조직별 토큰·추정 비용 (합 {usd(m.total_cost_usd)})" }
        p { class: "muted small", "스모크 조직은 빼고, 조직 없는 개인 연습 채점은 마지막 줄에 따로 셉니다. 비용은 Claude 호출(AI 리포트·평가)만 셉니다(판정기 채점은 기록하지 않는다)." }
        for c in m.costs.iter() {
            details {
                summary { "{c.name} — 응시 {thousands(c.attempts)}건 · {usd(c.cost_usd)}" }
                if c.by_kind.is_empty() {
                    p { class: "muted small", "기록이 없습니다." }
                } else {
                    {kind_table(&c.by_kind)}
                }
            }
        }
    }
}

// ── 허용 목록 ──

fn kind_label(kind: AllowKind) -> &'static str {
    match kind {
        AllowKind::AppVersion => "앱 판",
        AllowKind::ModelSha256 => "모델 sha256",
    }
}

fn action_label(action: AllowAction) -> &'static str {
    match action {
        AllowAction::Allow => "더함",
        AllowAction::Block => "막음",
        AllowAction::Reset => "기본값으로",
    }
}

#[component]
fn AllowlistTab() -> Element {
    let mut view = use_resource(api::allowlist);
    let mut kind = use_signal(|| AllowKind::AppVersion);
    let mut value = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let submit = move |action: AllowAction| {
        let request = AllowlistChangeRequest {
            kind: *kind.read(),
            value: value.read().trim().to_owned(),
            action,
            note: note.read().trim().to_owned(),
        };
        spawn(async move {
            match api::change_allowlist(&request).await {
                Ok(_) => {
                    error.set(None);
                    value.set(String::new());
                    note.set(String::new());
                    view.restart();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let body = match &*view.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(v)) => allowlist_body(v),
    };
    let model = *kind.read() == AllowKind::ModelSha256;
    rsx! {
        section { class: "sc-panel sc-panel-section",
            h2 { class: "sc-panel-title", "허용 목록" }
            p { class: "muted small",
                "서버가 받는 목록 = 코드 상수 ∪ 여기서 더한 값 − 막은 값. 새 앱 판은 지금처럼 PR로 상수에 더하고, 이 화면은 배포 없이 급히 막거나 배포 전 판을 받을 때 씁니다. 모든 변경은 누가·언제·왜와 함께 이력에 남습니다. 모델은 막기만 됩니다(채점이 고정 모델로만 돈다). 신고값이라 고친 앱은 속일 수 있습니다."
            }
            {body}
            h3 { "바꾸기" }
            div { class: "row",
                select {
                    class: "sc-select",
                    onchange: move |e| kind.set(AllowKind::parse(&e.value()).unwrap_or(AllowKind::AppVersion)),
                    for k in AllowKind::ALL {
                        option { value: "{k.as_str()}", selected: k == *kind.read(), "{kind_label(k)}" }
                    }
                }
                input {
                    class: "sc-input grow",
                    value: "{value}",
                    placeholder: if model { "sha256(16진 64자)" } else { "예: 0.2.0" },
                    oninput: move |e| value.set(e.value()),
                }
            }
            label { class: "field",
                span { "메모(왜)" }
                input { value: "{note}", oninput: move |e| note.set(e.value()) }
            }
            if let Some(text) = error.read().clone() {
                {notice(text)}
            }
            div { class: "row actions",
                if !model {
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| submit(AllowAction::Allow), "더하기" }
                }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| submit(AllowAction::Block), "막기" }
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: move |_| submit(AllowAction::Reset), "기본값으로" }
            }
        }
    }
}

fn allowlist_body(v: &AllowlistView) -> Element {
    let column = |kind: AllowKind, effective: &[String], constants: &[String]| {
        let blocked: Vec<String> = constants
            .iter()
            .filter(|c| !effective.contains(c))
            .cloned()
            .collect();
        let rows: Vec<(String, bool)> = effective
            .iter()
            .map(|e| (e.clone(), constants.contains(e)))
            .collect();
        rsx! {
            h3 { "{kind_label(kind)}" }
            ul { class: "plain values",
                for (value, constant) in rows {
                    li {
                        code { "{value}" }
                        " "
                        span { class: "sc-badge sc-badge--neutral", if constant { "상수" } else { "더함" } }
                    }
                }
                for value in blocked {
                    li {
                        code { "{value}" }
                        " "
                        span { class: "sc-badge sc-badge--danger", "막음" }
                    }
                }
            }
        }
    };
    rsx! {
        {column(AllowKind::AppVersion, &v.effective.app_versions, &v.constants.app_versions)}
        {column(AllowKind::ModelSha256, &v.effective.model_sha256, &v.constants.model_sha256)}
        h3 { "변경 이력" }
        if v.history.is_empty() {
            p { class: "muted", "아직 바꾼 적이 없습니다(상수 그대로)." }
        }
        ul { class: "plain",
            for c in v.history.iter() {
                li {
                    p { class: "desc",
                        "{kind_label(c.kind)} "
                        code { "{c.value}" }
                        " {action_label(c.action)}"
                    }
                    span { class: "muted small",
                        "{when(c.changed_at)} · {c.changed_by}"
                        if !c.note.is_empty() { " · {c.note}" }
                    }
                }
            }
        }
    }
}
