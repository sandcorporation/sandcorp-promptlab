//! 이용약관 동의 화면 조각(#86): 조직 만들기의 동의 칸, 구성원 초대 수락 페이지, 조직 화면의 동의 기록 줄.
//! 약관 판·주소는 shared 상수 한 곳(`promptlab_shared::terms`)이다.

use dioxus::prelude::*;
use promptlab_shared::api::OrgView;
use promptlab_shared::terms::{TERMS_URL, TERMS_VERSION};

use crate::route::{OrgPage, Route};
use crate::shell::{self, PageHead};
use crate::signals::kst_full;
use crate::{api, notice};

/// 「이용약관({판})에 동의합니다」 체크 줄. 약관은 새 탭에서 연다.
#[component]
pub fn TermsCheck(id: String, checked: bool, on_change: EventHandler<bool>) -> Element {
    rsx! {
        label { class: "terms-check", r#for: "{id}",
            input {
                id: "{id}",
                r#type: "checkbox",
                checked,
                onchange: move |e| on_change.call(e.checked()),
            }
            span {
                a { href: TERMS_URL, target: "_blank", rel: "noopener", "이용약관" }
                " (시행 {TERMS_VERSION})에 동의합니다."
            }
        }
    }
}

/// 구성원 초대 수락 페이지(`#/invite/{token}`): 동의해야 들어온다.
#[component]
pub fn InvitePage(token: String, on_change: EventHandler<()>) -> Element {
    let route = shell::use_route();
    let mut agreed = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let accept = move |_| {
        if !*agreed.read() || *busy.read() {
            return;
        }
        busy.set(true);
        let token = token.clone();
        spawn(async move {
            match api::accept_invite(&token).await {
                Ok(org) => {
                    on_change.call(());
                    shell::replace(route, Route::org(&org.id, OrgPage::Exams));
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(format!("초대를 받지 못했습니다: {e}")));
                }
            }
        });
    };
    rsx! {
        PageHead { title: "조직 초대", sub: "동료가 보낸 초대 링크입니다. 약관에 동의하면 조직에 들어갑니다." }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        section { class: "sc-panel", "aria-label": "초대 수락",
            div { class: "sc-panel-section",
                h2 { class: "sc-panel-title", "초대 수락" }
                p { class: "muted", "초대를 받으면 그 조직의 구성원이 되어 초대된 역할로 일합니다. 초대 링크는 한 번만 쓸 수 있습니다." }
                TermsCheck { id: "invite-terms", checked: *agreed.read(), on_change: move |v| agreed.set(v) }
                if !*agreed.read() {
                    p { id: "invite-terms-why", class: "muted small", "약관에 동의해야 들어갈 수 있습니다." }
                }
                div { class: "row",
                    button {
                        class: "sc-btn sc-btn--accent",
                        disabled: !*agreed.read() || *busy.read(),
                        "aria-describedby": if *agreed.read() { None } else { Some("invite-terms-why") },
                        onclick: accept,
                        "동의하고 들어가기"
                    }
                    a { class: "sc-btn sc-btn--secondary", href: Route::Home.href(), "나중에" }
                }
            }
        }
    }
}

/// 조직 화면의 내 약관 동의 기록 줄.
#[component]
pub fn TermsLine(org: OrgView) -> Element {
    rsx! {
        p { class: "muted small terms-line",
            match &org.terms {
                Some(t) => rsx! {
                    "이용약관 {t.version} 동의 · {kst_full(t.accepted_at * 1000)} · "
                },
                None => rsx! { "이용약관 동의 기록 없음(약관 동의 도입 전에 만들어진 조직) · " },
            }
            a { href: TERMS_URL, target: "_blank", rel: "noopener", "약관 보기" }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::signals::{kst, kst_full};

    #[test]
    fn 동의_시각은_해를_붙여_분까지_보인다() {
        // 2026-11-07 10:30:00 KST = 2026-11-07 01:30:00 UTC
        let ms = 1_794_015_000_000;
        assert_eq!(kst(ms), "11-07 10:30:00");
        assert_eq!(kst_full(ms), "2026-11-07 10:30");
        // 1월·2월은 해를 하나 더한다(3월 시작 달력 계산).
        assert!(kst_full(1_767_193_200_000).starts_with("2026-01-01 00:00"));
    }
}
