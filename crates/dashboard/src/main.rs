//! 웹 대시보드 진입점. `dx serve -p promptlab-dashboard --platform web` 으로 띄운다(`Dioxus.toml`이 `/api`를
//! 개발 서버 `127.0.0.1:18080`으로 넘긴다).
//!
//! 지금 화면: 카카오(sandcorp-auth) 로그인·로그아웃, 조직 만들기, 구성원과 역할, 동료 초대 링크 만들기·받기,
//! 시험 만들기(문제은행·문항별 제한)·응시자 초대(일괄)·초대 현황([`exams`]), 응시자가 초대 링크(`/exam-invite/…`)로
//! 들어와 배정받는 화면. 운영에서는 서버가 이 앱의 wasm 산출물을 `/`에서 서빙한다(`docker/server.Dockerfile`).
//! 로그인은 서버가 기밀 클라이언트로 처리하고(`/api/auth/login`) 세션 쿠키로 API를 부른다. 권한은 서버가
//! 강제하고, 화면은 같은 역할 표([`Role::allows`])로 쓸 수 없는 버튼을 숨길 뿐이다. 결과 화면은 #10.
//!
//! 골격(#75): 왼쪽 메뉴·윗막대·이동 경로([`shell`]) 안에 페이지 하나([`route::Route`], 주소는 해시 `#/…`).

mod activity_view;
mod api;
mod candidate;
mod erasure;
mod exam_settings;
mod exams;
mod live;
mod operator;
mod practice;
mod problem_draft;
mod problem_editor;
mod problems;
mod replay;
mod report;
mod results;
mod retention;
mod route;
mod shell;
mod signals;
mod terms;
mod timeline;
mod usage;
mod webcam;

use dioxus::prelude::*;
use promptlab_shared::account::{Permission, Role};
use promptlab_shared::api::{MeView, MemberView, OrgView};

use route::{OrgPage, Route};
use shell::{PageHead, Shell};

#[cfg(target_arch = "wasm32")]
fn main() {
    dioxus::launch(App);
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("대시보드는 웹(wasm32) 앱이다: dx serve -p promptlab-dashboard --platform web");
    let _ = App;
}

/// sandcorp-design(태그 고정 복사, `design/` — `scripts/sync-design.sh`) 다음에 대시보드 규칙을 얹는다.
/// 둘 다 wasm에 굽는다: 운영(wasm-bindgen 산출물)과 개발(`dx serve`)이 같은 모습이고 따로 서빙할 CSS가 없다.
const DESIGN_CSS: &str = include_str!("../../../design/sandcorp.css");
const STYLE: &str = include_str!("style.css");
const MARK: &str = include_str!("../../../design/mark.svg");
const MARK_DARK: &str = include_str!("../../../design/mark-dark.svg");

/// 머리띠의 브랜드(라이트·다크 표지는 CSS가 `prefers-color-scheme`으로 고른다).
fn brand() -> Element {
    rsx! {
        a { class: "sc-brand", href: "/",
            span { class: "mark mark-light", "aria-hidden": "true", dangerous_inner_html: MARK }
            span { class: "mark mark-dark", "aria-hidden": "true", dangerous_inner_html: MARK_DARK }
            "promptlab"
        }
    }
}

/// 카카오 로그인 심볼(카카오 디자인 가이드, sandcorp-design 견본과 같은 경로). 색은 `.sc-btn-kakao`가 토큰으로 준다.
const KAKAO_SYMBOL: &str = "M68.001 16.5225C64.4222 16.5225 61.5225 19.0037 61.5225 22.0641C61.5225 24.0312 62.7219 25.7596 64.5292 26.7423L63.9181 29.2113C63.8954 29.285 63.9132 29.364 63.9619 29.4184C63.9975 29.457 64.0461 29.478 64.0931 29.478C64.1337 29.478 64.1742 29.464 64.2082 29.4341L66.834 27.5144C67.2117 27.5723 67.6007 27.6039 67.9994 27.6039C71.5767 27.6039 74.478 25.1226 74.478 22.0623C74.478 19.002 71.5783 16.5225 68.001 16.5225Z";

/// 화면 위의 알림.
fn notice(text: String) -> Element {
    rsx! { div { class: "sc-alert sc-alert--error", role: "alert", "{text}" } }
}

#[component]
fn App() -> Element {
    shell::provide_nav();
    let path = api::current_path();
    let invite_token = path.strip_prefix("/invite/").map(str::to_owned);
    let exam_token = path
        .strip_prefix("/exam-invite/")
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let login_error = api::current_query().contains("login_error");
    let mut me = use_resource(api::me);
    let logout = move |_| {
        spawn(async move {
            let _ = api::logout().await;
            me.restart();
        });
    };

    let body = match &*me.read() {
        None => rsx! { p { class: "muted", "불러오는 중…" } },
        Some(Err(api::Error::Unauthorized)) => {
            // 응시 초대 링크로 왔으면 바로 로그인으로 보낸다(링크 → 카카오 로그인 한 번 → 배정).
            if exam_token.is_some() && !login_error {
                api::navigate(&api::login_url(&path));
            }
            return rsx! {
                style { {DESIGN_CSS} }
                style { {STYLE} }
                header { class: "sc-masthead",
                    div { class: "sc-masthead-row",
                        {brand()}
                        p { class: "sc-masthead-meta", "대시보드" }
                    }
                }
                main { class: "login",
                    h1 { class: "sc-visually-hidden", "promptlab 대시보드" }
                    if login_error {
                        {notice("로그인하지 못했습니다. 다시 시도해 주세요.".into())}
                    }
                    section { class: "sc-panel login-panel",
                        if exam_token.is_some() {
                            p { "시험을 배정받으려면 카카오로 로그인하세요." }
                        } else {
                            p { "promptlab 대시보드는 Sandcorp 계정으로 들어옵니다." }
                        }
                        a { class: "sc-btn-social sc-btn-kakao", href: api::login_url(&path),
                            svg { view_box: "61.5225 16.5225 12.9555 12.9555", "aria-hidden": "true",
                                path { d: KAKAO_SYMBOL }
                            }
                            span { "카카오 로그인" }
                        }
                    }
                }
            };
        }
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(me_view)) if exam_token.is_some() => rsx! {
            Shell {
                me: me_view.clone(),
                org: None::<OrgView>,
                crumb: Some("시험 배정".to_owned()),
                on_logout: logout,
                exams::ExamInvitePage { token: exam_token.clone().unwrap_or_default() }
            }
        },
        Some(Ok(me_view)) => rsx! {
            Signed {
                me: me_view.clone(),
                invite_token: invite_token.clone(),
                on_change: move |_| me.restart(),
                on_logout: logout,
            }
        },
    };
    rsx! {
        style { {DESIGN_CSS} }
        style { {STYLE} }
        {body}
    }
}

/// 로그인한 사람: 지금 주소의 페이지를 골격 안에 그린다.
#[component]
fn Signed(
    me: MeView,
    invite_token: Option<String>,
    on_change: EventHandler<()>,
    on_logout: EventHandler<()>,
) -> Element {
    let route = shell::use_route();
    // 처음 들어왔고 주소가 비었으면 첫 조직의 시험으로(조직이 없으면 「내 조직」).
    let first_org = me.orgs.first().map(|o| o.id.clone());
    let no_invite = invite_token.is_none();
    use_hook(move || {
        if no_invite
            && api::current_hash()
                .trim_start_matches('#')
                .trim_matches('/')
                .is_empty()
            && let Some(org) = first_org
        {
            shell::replace(route, Route::org(&org, OrgPage::Exams));
        }
    });

    // 동료 초대 링크(`/invite/{token}`)로 들어왔으면 초대 수락 페이지로(약관 동의 뒤 받는다, #86).
    let invite = invite_token.clone();
    use_hook(move || {
        if let Some(token) = invite {
            shell::replace(route, Route::Invite(token));
        }
    });

    let current = route.read().clone();
    let org = current
        .org_id()
        .and_then(|id| me.orgs.iter().find(|o| o.id == id))
        .cloned();
    let page = match &current {
        Route::Home => rsx! { OrgsPage { me: me.clone(), on_change } },
        Route::Practice => rsx! {
            PageHead { title: "내 연습 기록", sub: "개인 연습(조직 밖)에서 낸 기록입니다. 나만 봅니다." }
            practice::PracticePanel {}
        },
        Route::Operator(tab) if me.is_owner => rsx! { operator::OperatorPanel { tab: *tab } },
        Route::Operator(_) => notice("운영자 화면은 대표 계정만 봅니다.".into()),
        Route::Invite(token) => {
            rsx! { terms::InvitePage { key: "{token}", token: token.clone(), on_change } }
        }
        Route::Org { page, .. } => match &org {
            Some(org) => {
                rsx! { OrgRouter { key: "{org.id}", org: org.clone(), page: page.clone(), me_sub: me.sub.clone() } }
            }
            None => rsx! {
                {notice("이 조직의 구성원이 아닙니다.".into())}
                p { a { href: Route::Home.href(), "내 조직으로" } }
            },
        },
    };
    rsx! {
        Shell { me: me.clone(), org, on_logout,
            {page}
        }
    }
}

/// 내 조직: 조직 고르기·만들기. 조직 만들기는 약관 동의(#86)를 받는다.
#[component]
fn OrgsPage(me: MeView, on_change: EventHandler<()>) -> Element {
    let route = shell::use_route();
    let mut new_org = use_signal(String::new);
    let mut agreed = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let create = move |_| {
        let name = new_org.read().trim().to_owned();
        if name.is_empty() || !*agreed.read() {
            return;
        }
        spawn(async move {
            match api::create_org(&name).await {
                Ok(org) => {
                    new_org.set(String::new());
                    agreed.set(false);
                    on_change.call(());
                    shell::replace(route, Route::org(&org.id, OrgPage::Exams));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    rsx! {
        PageHead { title: "내 조직", sub: "조직을 고르면 그 조직의 시험·문제·구성원을 봅니다." }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        section { class: "sc-panel", "aria-label": "내 조직",
            if me.orgs.is_empty() {
                p { class: "sc-panel-section muted", "아직 속한 조직이 없습니다. 조직을 만들거나 동료에게 초대 링크를 받으세요." }
            } else {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "조직" }
                                th { scope: "col", "내 역할" }
                                th { scope: "col", "상태" }
                            }
                        }
                        tbody {
                            for org in me.orgs.iter().cloned() {
                                tr { class: "sc-table-row-link",
                                    td { a { href: Route::org(&org.id, OrgPage::Exams).href(), "{org.name}" } }
                                    td { "{org.my_role.label()}" }
                                    td {
                                        if org.approved {
                                            span { class: "sc-badge sc-badge--success", "승인됨" }
                                        } else {
                                            span { class: "sc-badge sc-badge--warning", "승인 대기" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        section { class: "sc-panel", "aria-label": "조직 만들기",
            div { class: "sc-panel-section",
                h2 { class: "sc-panel-title", "조직 만들기" }
                input {
                    class: "sc-input",
                    "aria-label": "새 조직 이름",
                    placeholder: "새 조직 이름(회사명)",
                    value: "{new_org}",
                    oninput: move |e| new_org.set(e.value()),
                }
                terms::TermsCheck { id: "org-terms", checked: *agreed.read(), on_change: move |v| agreed.set(v) }
                div { class: "row",
                    button {
                        class: "sc-btn sc-btn--accent sc-btn--sm",
                        disabled: !*agreed.read() || new_org.read().trim().is_empty(),
                        "aria-describedby": "create-org-why",
                        onclick: create,
                        "조직 만들기"
                    }
                    if !*agreed.read() {
                        span { id: "create-org-why", class: "muted small", "약관에 동의해야 만들 수 있습니다." }
                    } else if new_org.read().trim().is_empty() {
                        span { id: "create-org-why", class: "muted small", "조직 이름을 입력하세요." }
                    }
                }
                p { class: "muted small", "새 조직은 운영자 승인 전까지 시험을 만들 수 없습니다. 문제 만들기·구성원 초대는 바로 됩니다." }
            }
        }
    }
}

/// 조직 안 페이지 고르기.
#[component]
fn OrgRouter(org: OrgView, page: OrgPage, me_sub: String) -> Element {
    let can_manage = org.my_role.allows(Permission::ManageMembers);
    let content = match page {
        OrgPage::Exams => rsx! { exams::ExamsPage { org: org.clone() } },
        OrgPage::NewExam => rsx! { exams::NewExamPage { org: org.clone() } },
        OrgPage::Exam { exam, tab } => rsx! {
            exams::ExamPage { key: "{exam}", org: org.clone(), exam_id: exam.clone(), tab }
        },
        OrgPage::Candidate { exam, invite, tab } => rsx! {
            candidate::CandidatePage { key: "{exam}-{invite}", org: org.clone(), exam_id: exam.clone(), invite_id: invite.clone(), tab }
        },
        OrgPage::Problems => rsx! { problems::ProblemsPanel { org: org.clone() } },
        OrgPage::Live => rsx! { exams::LivePage { org: org.clone() } },
        OrgPage::Usage if can_manage => rsx! {
            PageHead { title: "사용량", sub: "응시 수·종류별 토큰·추정 비용입니다." }
            usage::UsagePanel { org_id: org.id.clone() }
        },
        OrgPage::Members => rsx! { MembersPage { org: org.clone(), me_sub } },
        OrgPage::Retention => rsx! {
            PageHead { title: "보관·삭제", sub: "응시 데이터 보관 기간과 응시자 데이터 삭제 기록입니다." }
            retention::RetentionPanel { org_id: org.id.clone(), exam_id: None::<String>, can_edit: can_manage }
            if can_manage {
                erasure::ErasureLogPanel { org_id: org.id.clone() }
            }
        },
        _ => notice("이 페이지를 볼 권한이 없습니다.".into()),
    };
    rsx! {
        if !org.approved {
            div { class: "sc-alert", role: "status",
                "운영자 승인 전인 시범 조직입니다 — 승인되면 시험을 만들고 응시를 시작할 수 있습니다. 구성원 초대·문제 만들기는 지금도 됩니다."
            }
        }
        {content}
    }
}

/// 구성원·동료 초대.
#[component]
fn MembersPage(org: OrgView, me_sub: String) -> Element {
    let org_id = org.id.clone();
    let mut members = use_resource(move || {
        let org_id = org_id.clone();
        async move { api::members(&org_id).await }
    });
    let mut invite_role = use_signal(|| Role::Viewer);
    let mut invite_link = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let can_manage = org.my_role.allows(Permission::ManageMembers);

    let org_id = org.id.clone();
    let make_invite = move |_| {
        let org_id = org_id.clone();
        let role = *invite_role.read();
        spawn(async move {
            match api::create_invite(&org_id, role).await {
                Ok(invite) => {
                    invite_link.set(Some(format!("{}/invite/{}", api::origin(), invite.token)))
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let rows = match &*members.read() {
        None => rsx! { p { class: "sc-panel-section muted", "불러오는 중…" } },
        Some(Err(e)) => notice(e.to_string()),
        Some(Ok(list)) => {
            let list: Vec<MemberView> = list.clone();
            rsx! {
                div { class: "sc-table-scroll",
                    table { class: "sc-table",
                        thead {
                            tr {
                                th { scope: "col", "구성원" }
                                th { scope: "col", "역할" }
                                if can_manage {
                                    th { scope: "col", span { class: "sc-visually-hidden", "동작" } }
                                }
                            }
                        }
                        tbody {
                            for member in list {
                                MemberRow {
                                    key: "{member.sub}",
                                    org_id: org.id.clone(),
                                    member: member.clone(),
                                    can_manage,
                                    is_me: member.sub == me_sub,
                                    on_change: move |result: Result<(), String>| {
                                        if let Err(e) = result {
                                            error.set(Some(e));
                                        }
                                        members.restart();
                                    },
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    rsx! {
        PageHead {
            title: "구성원·초대",
            sub: format!("{} · 내 역할: {}", org.name, org.my_role.label()),
        }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        section { class: "sc-panel", "aria-label": "구성원", {rows} }
        terms::TermsLine { org: org.clone() }
        if can_manage {
            section { class: "sc-panel", "aria-label": "동료 초대",
                div { class: "sc-panel-section",
                    h2 { class: "sc-panel-title", "동료 초대" }
                    div { class: "row",
                        RoleSelect { value: *invite_role.read(), on_select: move |r| invite_role.set(r) }
                        button { class: "sc-btn sc-btn--accent sc-btn--sm", onclick: make_invite, "초대 링크 만들기" }
                    }
                    if let Some(link) = invite_link.read().clone() {
                        p { class: "muted small", "이 링크를 동료에게 보내세요(한 번 쓰면 끝, 7일 뒤 만료):" }
                        code { "{link}" }
                    }
                }
            }
        }
    }
}

#[component]
fn MemberRow(
    org_id: String,
    member: MemberView,
    can_manage: bool,
    is_me: bool,
    on_change: EventHandler<Result<(), String>>,
) -> Element {
    let name = member.name.clone().unwrap_or_else(|| member.sub.clone());
    let sub = member.sub.clone();
    let org = org_id.clone();
    let change_role = move |role: Role| {
        let (org, sub) = (org.clone(), sub.clone());
        spawn(async move {
            on_change.call(
                api::set_role(&org, &sub, role)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
            );
        });
    };
    let sub = member.sub.clone();
    let remove = move |_| {
        let (org, sub) = (org_id.clone(), sub.clone());
        spawn(async move {
            on_change.call(
                api::remove_member(&org, &sub)
                    .await
                    .map_err(|e| e.to_string()),
            );
        });
    };
    rsx! {
        tr {
            td {
                "{name}"
                if is_me {
                    span { class: "muted", " (나)" }
                }
            }
            if can_manage {
                td { RoleSelect { value: member.role, on_select: change_role } }
                td { class: "sc-num",
                    button { class: "sc-btn sc-btn--secondary sc-btn--sm", onclick: remove, "내보내기" }
                }
            } else {
                td { "{member.role.label()}" }
            }
        }
    }
}

#[component]
fn RoleSelect(value: Role, on_select: EventHandler<Role>) -> Element {
    rsx! {
        select {
            class: "sc-select",
            "aria-label": "역할",
            onchange: move |e| {
                if let Some(role) = Role::parse(&e.value()) {
                    on_select.call(role);
                }
            },
            for role in Role::ALL {
                option { value: role.as_str(), selected: role == value, "{role.label()}" }
            }
        }
    }
}
