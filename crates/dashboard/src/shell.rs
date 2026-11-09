//! 대시보드 골격(#75): 왼쪽 메뉴(`sc-sidenav`, 1180px 아래 아이콘만 — CSS) · 윗막대(이동 경로 + 계정 메뉴) ·
//! 페이지(`sc-app-page`). 지금 페이지는 [`Route`] 하나이고 컨텍스트로 나눈다([`use_route`]) — 주소창 해시가
//! 정본이라 링크는 `<a href="#/…">`, 코드에서 옮길 때는 [`go`]. 페이지 머리는 [`PageHead`], 이동 경로의 마지막
//! 칸(시험 이름처럼 페이지가 불러와야 아는 것)은 [`use_crumb_tail`]로 페이지가 채운다.

use dioxus::prelude::*;
use promptlab_shared::account::{Permission, Role};
use promptlab_shared::api::{MeView, OrgView};

use crate::api;
use crate::route::{ExamTab, OperatorTab, OrgPage, Route};

/// 지금 페이지(컨텍스트).
#[derive(Clone, Copy)]
struct Nav {
    route: Signal<Route>,
    tail: Signal<Option<String>>,
}

/// 골격을 세운다(앱이 한 번). 해시를 읽고 바뀔 때마다 따라간다.
pub fn provide_nav() {
    let route = use_signal(|| Route::parse(&api::current_hash()));
    let tail = use_signal(|| None::<String>);
    use_hook(move || {
        let mut route = route;
        let mut tail = tail;
        api::on_hash_change(move |hash| {
            let next = Route::parse(&hash);
            if *route.peek() != next {
                tail.set(None);
                route.set(next);
            }
        });
    });
    use_context_provider(|| Nav { route, tail });
}

/// 지금 페이지.
pub fn use_route() -> Signal<Route> {
    use_context::<Nav>().route
}

/// 이동 경로의 마지막 칸(페이지 제목). 페이지가 불러온 이름을 넣는다.
pub fn use_crumb_tail() -> Signal<Option<String>> {
    use_context::<Nav>().tail
}

/// 다른 페이지로 간다(주소창 해시도 바뀐다 — 뒤로 가기가 된다).
pub fn go(route: Route) {
    api::navigate(&route.href());
}

/// 주소창을 바꾸되 방문 기록은 남기지 않는다(첫 화면 고르기).
pub fn replace(mut current: Signal<Route>, route: Route) {
    api::replace_path(&format!("/{}", route.href()));
    current.set(route);
}

// 메뉴 아이콘(16px, 선 1.3 — sandcorp-design 견본과 같은 꼴).
const ICON_EXAMS: &str = r#"<rect x="2.5" y="1.5" width="11" height="13" rx="2" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M5 5.5h6M5 8h6M5 10.5h3.5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#;
const ICON_PROBLEMS: &str = r#"<path d="M10.5 2.5l3 3-7.5 7.5H3v-3z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/><path d="M9 4l3 3" stroke="currentColor" stroke-width="1.3"/>"#;
const ICON_LIVE: &str = r#"<rect x="1.5" y="4" width="9.5" height="8" rx="1.5" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M11 7l3.5-2v6L11 9" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>"#;
const ICON_USAGE: &str = r#"<path d="M3 13.5V8M8 13.5V3M13 13.5V6" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>"#;
const ICON_MEMBERS: &str = r#"<circle cx="6" cy="5.5" r="2.3" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M1.8 13.5c.4-2.4 2.1-3.8 4.2-3.8s3.8 1.4 4.2 3.8" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M10.5 3.4a2.2 2.2 0 010 4.2M12 9.9c1.2.5 2 1.7 2.2 3.6" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/>"#;
const ICON_RETENTION: &str = r#"<path d="M2.5 4.5h11M6 4.5V3h4v1.5M4 4.5l.7 9h6.6l.7-9" fill="none" stroke="currentColor" stroke-width="1.3"/>"#;
const ICON_PRACTICE: &str = r#"<path d="M2 12l3.5-4 3 2.5L14 4" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"/>"#;
const ICON_OPERATOR: &str = r#"<path d="M8 1.8l5 2v4c0 3.2-2.1 5.4-5 6.4-2.9-1-5-3.2-5-6.4v-4z" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"/>"#;
const ICON_ORGS: &str = r#"<rect x="2.5" y="2" width="7" height="12" fill="none" stroke="currentColor" stroke-width="1.3"/><path d="M9.5 6.5h4V14h-4M4.5 4.5h3M4.5 7h3M4.5 9.5h3" fill="none" stroke="currentColor" stroke-width="1.3"/>"#;

/// 메뉴 항목 하나.
#[component]
fn NavItem(route: Route, label: String, icon: &'static str, current: bool) -> Element {
    rsx! {
        a {
            class: "sc-sidenav-item",
            href: route.href(),
            title: "{label}",
            "aria-current": if current { Some("page") } else { None },
            svg {
                width: "16",
                height: "16",
                view_box: "0 0 16 16",
                "aria-hidden": "true",
                dangerous_inner_html: icon,
            }
            span { class: "sc-sidenav-label", "{label}" }
        }
    }
}

/// 조직 메뉴의 어느 칸이 지금인가(시험 상세는 「시험」, 상세의 실시간 감독 탭은 「실시간 감독」).
fn org_nav_of(page: &OrgPage) -> OrgPage {
    match page {
        OrgPage::NewExam => OrgPage::Exams,
        OrgPage::Exam {
            tab: ExamTab::Live, ..
        } => OrgPage::Live,
        OrgPage::Exam { .. } | OrgPage::Candidate { .. } => OrgPage::Exams,
        other => other.clone(),
    }
}

/// 조직 메뉴의 이름.
pub fn org_page_label(page: &OrgPage) -> &'static str {
    match page {
        OrgPage::Exams | OrgPage::NewExam | OrgPage::Exam { .. } | OrgPage::Candidate { .. } => {
            "시험"
        }
        OrgPage::Problems => "문제",
        OrgPage::Live => "실시간 감독",
        OrgPage::Usage => "사용량",
        OrgPage::Members => "구성원·초대",
        OrgPage::Retention => "보관·삭제",
    }
}

/// 왼쪽 메뉴.
#[component]
fn SideNav(me: MeView, org: Option<OrgView>) -> Element {
    let route = use_route();
    let current = route.read().clone();
    let here = match &current {
        Route::Org { page, .. } => Some(org_nav_of(page)),
        _ => None,
    };
    let mut items: Vec<(OrgPage, &'static str, Option<&'static str>)> = Vec::new();
    if let Some(org) = &org {
        let role: Role = org.my_role;
        items.push((OrgPage::Exams, ICON_EXAMS, None));
        if role.allows(Permission::ManageExams) {
            items.push((OrgPage::Problems, ICON_PROBLEMS, None));
        }
        if role.allows(Permission::Proctor) {
            items.push((OrgPage::Live, ICON_LIVE, None));
        }
        if role.allows(Permission::ManageMembers) {
            items.push((OrgPage::Usage, ICON_USAGE, None));
        }
        items.push((OrgPage::Members, ICON_MEMBERS, Some("조직")));
        items.push((OrgPage::Retention, ICON_RETENTION, None));
    }
    rsx! {
        aside { class: "sc-sidenav", "aria-label": "주 메뉴",
            a {
                class: "sc-sidenav-org",
                href: Route::Home.href(),
                title: "조직 바꾸기",
                "aria-current": if current == Route::Home { Some("page") } else { None },
                if let Some(org) = &org {
                    span { class: "sc-sidenav-mark", "aria-hidden": "true",
                        "{org.name.chars().next().unwrap_or('·')}"
                    }
                    span { class: "sc-sidenav-org-text",
                        "{org.name}"
                        small { "{org.my_role.label()} · 조직 바꾸기" }
                    }
                } else {
                    span { class: "sc-sidenav-mark", "aria-hidden": "true",
                        svg { width: "16", height: "16", view_box: "0 0 16 16", dangerous_inner_html: ICON_ORGS }
                    }
                    span { class: "sc-sidenav-org-text",
                        if matches!(current, Route::Invite(_)) {
                            "조직 초대"
                            small { "초대 수락 중" }
                        } else {
                            "내 조직"
                            small { "조직 고르기·만들기" }
                        }
                    }
                }
            }
            if let Some(org) = &org {
                for (page, icon, heading) in items {
                    if let Some(h) = heading {
                        h2 { class: "sc-sidenav-heading", "{h}" }
                    }
                    NavItem {
                        current: here.as_ref() == Some(&page),
                        label: org_page_label(&page).to_owned(),
                        route: Route::org(&org.id, page.clone()),
                        icon,
                    }
                }
            }
            h2 { class: "sc-sidenav-heading", "개인" }
            NavItem {
                current: current == Route::Practice,
                label: "내 연습 기록".to_owned(),
                route: Route::Practice,
                icon: ICON_PRACTICE,
            }
            if me.is_owner {
                h2 { class: "sc-sidenav-heading", "운영자" }
                NavItem {
                    current: matches!(current, Route::Operator(_)),
                    label: "운영자 화면".to_owned(),
                    route: Route::Operator(OperatorTab::Orgs),
                    icon: ICON_OPERATOR,
                }
            }
        }
    }
}

/// 이동 경로 칸 하나: (이름, 링크 — 마지막 칸은 없음).
type Crumb = (String, Option<Route>);

fn crumbs(route: &Route, org: Option<&OrgView>, tail: Option<String>) -> Vec<Crumb> {
    let mut out: Vec<Crumb> = Vec::new();
    match route {
        Route::Home => out.push(("내 조직".into(), None)),
        Route::Practice => out.push(("내 연습 기록".into(), None)),
        Route::Invite(_) => out.push(("조직 초대".into(), None)),
        Route::Operator(tab) => {
            out.push(("운영자".into(), Some(Route::Operator(OperatorTab::Orgs))));
            out.push((tab.label().into(), None));
        }
        Route::Org { org: id, page } => {
            if let Some(org) = org {
                out.push((org.name.clone(), Some(Route::Home)));
            }
            let section = org_nav_of(page);
            out.push((
                org_page_label(&section).into(),
                Some(Route::org(id, section.clone())),
            ));
            match page {
                OrgPage::NewExam => out.push(("새 시험".into(), None)),
                OrgPage::Exam { exam, tab } => {
                    let name = tail.unwrap_or_else(|| "시험".into());
                    out.push((
                        name,
                        Some(Route::org(
                            id,
                            OrgPage::Exam {
                                exam: exam.clone(),
                                tab: ExamTab::Candidates,
                            },
                        )),
                    ));
                    out.push((tab.label().into(), None));
                }
                OrgPage::Candidate { exam, .. } => {
                    let name = tail.unwrap_or_else(|| "시험".into());
                    out.push((
                        name,
                        Some(Route::org(
                            id,
                            OrgPage::Exam {
                                exam: exam.clone(),
                                tab: ExamTab::Results,
                            },
                        )),
                    ));
                    out.push(("응시자".into(), None));
                }
                _ => {}
            }
        }
    }
    // 마지막 칸은 지금 페이지라 링크가 없다.
    if let Some(last) = out.last_mut() {
        last.1 = None;
    }
    out
}

/// 윗막대: 이동 경로 + 계정 메뉴(로그아웃).
#[component]
fn TopBar(
    me: MeView,
    org: Option<OrgView>,
    crumb_override: Option<String>,
    on_logout: EventHandler<()>,
) -> Element {
    let route = use_route();
    let tail = use_crumb_tail();
    let mut open = use_signal(|| false);
    let list = match crumb_override {
        Some(only) => vec![(only, None)],
        None => crumbs(&route.read(), org.as_ref(), tail.read().clone()),
    };
    let name = me.name.clone().unwrap_or_else(|| me.sub.clone());
    let initial = name.chars().next().unwrap_or('·');
    let role = match &org {
        Some(org) => org.my_role.label().to_owned(),
        None if me.is_owner => "운영자".to_owned(),
        None => "개인".to_owned(),
    };
    rsx! {
        header { class: "sc-topbar",
            nav { "aria-label": "이동 경로",
                ol { class: "sc-topbar-crumbs",
                    for (label, link) in list {
                        li {
                            if let Some(link) = link {
                                a { href: link.href(), "{label}" }
                            } else {
                                span { "aria-current": "page", "{label}" }
                            }
                        }
                    }
                }
            }
            div { class: "account",
                button {
                    class: "sc-topbar-account",
                    r#type: "button",
                    "aria-haspopup": "menu",
                    "aria-expanded": if *open.read() { "true" } else { "false" },
                    onclick: move |_| {
                        let now = *open.read();
                        open.set(!now);
                    },
                    span { class: "sc-topbar-avatar", "aria-hidden": "true", "{initial}" }
                    span { class: "sc-topbar-account-text",
                        "{name}"
                        small { "{role}" }
                    }
                    svg { width: "10", height: "10", view_box: "0 0 12 12", "aria-hidden": "true",
                        path { d: "M2.5 4.5L6 8l3.5-3.5", fill: "none", stroke: "currentColor", stroke_width: "1.4" }
                    }
                    span { class: "sc-visually-hidden", "계정 메뉴" }
                }
                if *open.read() {
                    div { class: "account-menu sc-panel", role: "menu",
                        p { class: "muted small", "{me.sub}" }
                        button {
                            class: "sc-btn sc-btn--secondary sc-btn--sm",
                            role: "menuitem",
                            onclick: move |_| {
                                open.set(false);
                                on_logout.call(());
                            },
                            "로그아웃"
                        }
                    }
                }
            }
        }
    }
}

/// 로그인한 사람의 골격. `org`는 지금 페이지의 조직(없으면 조직 밖 페이지), `crumb`를 주면 이동 경로를 그것 하나로.
#[component]
pub fn Shell(
    me: MeView,
    org: Option<OrgView>,
    crumb: Option<String>,
    on_logout: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "sc-app",
            SideNav { me: me.clone(), org: org.clone() }
            div { class: "sc-app-main",
                TopBar { me, org, crumb_override: crumb, on_logout }
                main { class: "sc-app-page", {children} }
            }
        }
    }
}

/// 페이지 머리: 제목(+ 배지) · 보조 줄 · 오른쪽 동작.
#[component]
pub fn PageHead(
    title: String,
    sub: Option<String>,
    badges: Option<Element>,
    actions: Option<Element>,
) -> Element {
    rsx! {
        div { class: "sc-page-head",
            div {
                h1 { class: "sc-page-head-title",
                    "{title}"
                    if let Some(badges) = badges {
                        " "
                        {badges}
                    }
                }
                if let Some(sub) = sub {
                    p { class: "sc-page-head-sub", "{sub}" }
                }
            }
            if let Some(actions) = actions {
                div { class: "sc-page-head-actions", {actions} }
            }
        }
    }
}

/// 링크 탭(`nav.sc-tabs`).
#[component]
pub fn Tabs(label: String, tabs: Vec<(String, Route)>, current: Route) -> Element {
    rsx! {
        nav { class: "sc-tabs", "aria-label": "{label}",
            for (name, route) in tabs {
                a {
                    href: route.href(),
                    "aria-current": if route == current { Some("page") } else { None },
                    "{name}"
                }
            }
        }
    }
}
