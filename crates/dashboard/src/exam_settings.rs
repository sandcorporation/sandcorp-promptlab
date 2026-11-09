//! 시험 설정(#75 B, 시안 `WExamSettings`): 점수 비율 w 슬라이더·추가 기준표(최대 2)·결과 공개,
//! 저장 막대. 새 시험 만들기 폼(`exams::ExamForm`)도 같은 칸 묶음 [`GradingFields`]를 쓴다.
//! 범위 검사는 서버와 같은 [`ExamSettings::validate`].

use dioxus::prelude::*;
use promptlab_shared::api::ExamView;
use promptlab_shared::coding::Language;
use promptlab_shared::evaluation::COMMON_CRITERIA;
use promptlab_shared::exam::{
    ExamSettings, MAX_RUBRIC_DESCRIPTION_CHARS, MAX_RUBRIC_ITEMS, MAX_RUBRIC_NAME_CHARS, Purpose,
    RubricItem,
};

use crate::retention::RetentionPanel;
use crate::{api, notice};

/// 입력 칸 그대로의 설정(숫자는 글자).
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsDraft {
    pub weight: u8,
    pub rubric: Vec<RubricItem>,
    /// 허용 언어(#96, 1개 이상).
    pub languages: Vec<Language>,
}

impl SettingsDraft {
    pub fn from_settings(s: &ExamSettings) -> Self {
        Self {
            weight: s.score_weight,
            rubric: s.rubric.clone(),
            languages: s.allowed_languages.clone(),
        }
    }

    /// 서버에 보낼 설정. 숫자를 읽고 서버와 같은 범위 검사를 한다.
    pub fn to_settings(&self) -> Result<ExamSettings, String> {
        let settings = ExamSettings {
            score_weight: self.weight,
            rubric: self.rubric.clone(),
            allowed_languages: self.languages.clone(),
        };
        settings.validate()?;
        Ok(settings)
    }

    /// 저장하지 않은 변경 이름(저장 막대 문구용).
    fn changed(&self, saved: &SettingsDraft) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.weight != saved.weight {
            out.push("점수 비율");
        }
        if self.rubric != saved.rubric {
            out.push("기준표 항목");
        }
        if self.languages != saved.languages {
            out.push("허용 언어");
        }
        out
    }
}

/// 최종 점수 미리보기 한 줄(예시 값 85·71).
pub fn preview_line(weight: u8) -> String {
    let w = f64::from(weight) / 100.0;
    let last = 85.0 * w + 71.0 * (1.0 - w);
    format!("예: 통과율 85% · AI 활용 71 → 최종 {last:.1}")
}

/// 점수 비율·추가 기준표 칸 묶음(시험 설정 페이지와 새 시험 폼이 같이 쓴다).
#[component]
pub fn GradingFields(draft: Signal<SettingsDraft>, can_edit: bool) -> Element {
    let mut draft = draft;
    let d = draft.read().clone();
    let weight = d.weight;
    let rest = 100 - weight;
    let extra = d.rubric.len();
    let languages = d.languages.clone();
    rsx! {
        section { class: "sc-panel", "aria-label": "허용 언어",
            div { class: "sc-panel-section",
                h2 { class: "sc-panel-title", "허용 언어" }
                p { class: "muted small", "응시자는 문제마다 이 중 하나를 골라 풀고 제출합니다. 1개 이상 남겨야 합니다." }
                div { class: "lang-options",
                    for lang in Language::ALL {
                        label { class: "row", key: "{lang.as_str()}",
                            input {
                                r#type: "checkbox",
                                checked: languages.contains(&lang),
                                disabled: !can_edit,
                                onchange: move |e| {
                                    let mut d = draft.write();
                                    if e.checked() {
                                        if !d.languages.contains(&lang) {
                                            d.languages.push(lang);
                                            d.languages.sort();
                                        }
                                    } else {
                                        d.languages.retain(|l| *l != lang);
                                    }
                                },
                            }
                            span { "{lang.display_name()}" }
                        }
                    }
                }
                if languages.is_empty() {
                    span { class: "sc-tone-danger small", role: "alert", "허용 언어를 1개 이상 고르세요." }
                }
            }
        }
        section { class: "sc-panel", "aria-label": "점수 비율",
            div { class: "sc-panel-section",
                h2 { class: "sc-panel-title", "점수 비율" }
                div { class: "spread",
                    span { "통과율 ", b { "{weight}" }, "%" }
                    span { "AI 활용 ", b { "{rest}" }, "%" }
                }
                input {
                    class: "sc-slider sc-slider--split",
                    r#type: "range",
                    min: "0",
                    max: "100",
                    step: "5",
                    value: "{weight}",
                    style: "--sc-value: {weight}%",
                    "aria-label": "통과율 비율",
                    disabled: !can_edit,
                    oninput: move |e| {
                        if let Ok(v) = e.value().parse::<u8>() {
                            draft.write().weight = v.min(100);
                        }
                    },
                }
                span { class: "muted small", "{preview_line(weight)}" }
                span { class: "muted small", "최종 = 통과율 통과율 × 비율 + AI 활용 × (1 − 비율). AI 활용가 없으면 통과율만 씁니다." }
            }
        }
        section { class: "sc-panel", "aria-label": "AI 활용 평가 기준표",
            div { class: "sc-panel-section",
                h2 { class: "sc-panel-title", "AI 활용 평가 기준표" }
                p { class: "muted small", "Claude가 제출 뒤 문항마다 한 번 채점하고, 항목마다 근거(턴·제안 번호)를 남깁니다. 공통 4항목은 모든 시험에 같습니다." }
            }
            div { class: "sc-table-scroll",
                table { class: "sc-table",
                    thead {
                        tr {
                            th { scope: "col", "항목" }
                            th { scope: "col", "설명" }
                            th { scope: "col", class: "sc-num", "범위" }
                            th { scope: "col", class: "sc-num", span { class: "sc-visually-hidden", "빼기" } }
                        }
                    }
                    tbody {
                        for (_, name, description) in COMMON_CRITERIA {
                            tr { key: "{name}",
                                td { "{name}" span { class: "sc-table-sub", "공통" } }
                                td { "{description}" }
                                td { class: "sc-num", "0~5" }
                                td {}
                            }
                        }
                        for (i, item) in d.rubric.iter().cloned().enumerate() {
                            tr { key: "extra{i}",
                                td {
                                    input {
                                        class: "sc-input",
                                        "aria-label": "추가 항목 {i + 1} 이름",
                                        maxlength: "{MAX_RUBRIC_NAME_CHARS}",
                                        placeholder: "항목 이름",
                                        value: "{item.name}",
                                        disabled: !can_edit,
                                        oninput: move |e| draft.write().rubric[i].name = e.value(),
                                    }
                                }
                                td {
                                    input {
                                        class: "sc-input",
                                        "aria-label": "추가 항목 {i + 1} 설명",
                                        maxlength: "{MAX_RUBRIC_DESCRIPTION_CHARS}",
                                        placeholder: "무엇을 보는가(예: 출력 형식을 초반에 못 박았나)",
                                        value: "{item.description}",
                                        disabled: !can_edit,
                                        oninput: move |e| draft.write().rubric[i].description = e.value(),
                                    }
                                }
                                td { class: "sc-num", "0~5" }
                                td { class: "sc-num",
                                    if can_edit {
                                        button {
                                            class: "sc-btn sc-btn--secondary sc-btn--sm",
                                            r#type: "button",
                                            "aria-label": "추가 항목 {i + 1} 빼기",
                                            onclick: move |_| { draft.write().rubric.remove(i); },
                                            "빼기"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if can_edit {
                div { class: "sc-toolbar",
                    span { class: "muted small", "추가 항목 {extra} / {MAX_RUBRIC_ITEMS}" }
                    button {
                        class: "sc-btn sc-btn--secondary sc-btn--sm",
                        r#type: "button",
                        disabled: extra >= MAX_RUBRIC_ITEMS,
                        onclick: move |_| {
                            draft.write().rubric.push(RubricItem { name: String::new(), description: String::new() });
                        },
                        if extra >= MAX_RUBRIC_ITEMS { "항목 더하기 (다 찼음)" } else { "＋ 항목 더하기 ({MAX_RUBRIC_ITEMS - extra}개 남음)" }
                    }
                }
            }
        }
    }
}

/// 시험 상세의 「설정」 탭: 문항 표(읽기) + 채점 칸 + 결과 공개 + 보관 기간 + 저장 막대.
#[component]
pub fn ExamSettingsPanel(
    org_id: String,
    exam: ExamView,
    can_edit: bool,
    on_saved: EventHandler<()>,
) -> Element {
    let saved = SettingsDraft::from_settings(&exam.settings);
    let saved_reveal = exam.reveal_scores;
    let internal = exam.purpose == Purpose::Internal;
    let mut draft = use_signal({
        let saved = saved.clone();
        move || saved.clone()
    });
    let mut reveal = use_signal(move || saved_reveal);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let d = draft.read().clone();
    let mut changes = d.changed(&saved);
    if *reveal.read() != saved_reveal {
        changes.push("결과 공개");
    }
    let dirty = !changes.is_empty();
    let text = format!(
        "저장하지 않은 변경 {}개 — {}",
        changes.len(),
        changes.join(", ")
    );

    let revert = {
        let saved = saved.clone();
        move |_| {
            draft.set(saved.clone());
            reveal.set(saved_reveal);
            error.set(None);
        }
    };
    let save = {
        let org_id = org_id.clone();
        let exam_id = exam.id.clone();
        move |_| {
            let settings = match draft.read().to_settings() {
                Ok(s) => s,
                Err(e) => return error.set(Some(e)),
            };
            let reveal_now = *reveal.read();
            let org_id = org_id.clone();
            let exam_id = exam_id.clone();
            busy.set(true);
            spawn(async move {
                let reveal_change = (internal && reveal_now != saved_reveal).then_some(reveal_now);
                match api::update_exam(&org_id, &exam_id, reveal_change, Some(&settings)).await {
                    Ok(_) => {
                        error.set(None);
                        on_saved.call(());
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                busy.set(false);
            });
        }
    };

    rsx! {
        div { class: "settings-grid",
            div { class: "settings-col",
                section { class: "sc-panel", "aria-label": "문항",
                    div { class: "sc-toolbar", h2 { class: "sc-panel-title", "문항" } }
                    div { class: "sc-table-scroll",
                        table { class: "sc-table",
                            thead {
                                tr {
                                    th { scope: "col", class: "sc-num", "번호" }
                                    th { scope: "col", "문제" }
                                    th { scope: "col", class: "sc-num", "제한 시간" }
                                }
                            }
                            tbody {
                                for (n, item) in exam.items.iter().enumerate() {
                                    tr { key: "{n}",
                                        td { class: "sc-num", "{n + 1}" }
                                        td { "{item.problem_id}" }
                                        td { class: "sc-num", "{item.limits.time_limit_secs / 60}분" }
                                    }
                                }
                            }
                        }
                    }
                }
                GradingFields { draft, can_edit }
            }
            div { class: "settings-col",
                if internal {
                    section { class: "sc-panel", "aria-label": "결과 공개",
                        div { class: "sc-panel-section",
                            h2 { class: "sc-panel-title", "결과 공개" }
                            label { class: "row",
                                input {
                                    class: "sc-toggle",
                                    r#type: "checkbox",
                                    role: "switch",
                                    checked: *reveal.read(),
                                    disabled: !can_edit,
                                    onchange: move |e| reveal.set(e.checked()),
                                }
                                "응시자에게 점수 보여 주기"
                            }
                            span { class: "muted small", "사내 진단만 켤 수 있습니다." }
                        }
                    }
                } else {
                    section { class: "sc-panel", "aria-label": "결과 공개",
                        div { class: "sc-panel-section",
                            h2 { class: "sc-panel-title", "결과 공개" }
                            span { class: "muted small", "채용 시험은 응시자에게 「응시 완료」만 보입니다(바꿀 수 없음)." }
                        }
                    }
                }
                RetentionPanel { org_id: org_id.clone(), exam_id: Some(exam.id.clone()), can_edit }
            }
        }
        if let Some(text) = error.read().clone() {
            {notice(text)}
        }
        div { class: "sc-savebar", hidden: !dirty,
            span { class: "sc-savebar-text", role: "status", "{text}" }
            div { class: "sc-savebar-actions",
                button { class: "sc-btn sc-btn--secondary sc-btn--sm", r#type: "button", onclick: revert, "되돌리기" }
                button { class: "sc-btn sc-btn--accent sc-btn--sm", r#type: "button", disabled: *busy.read(), onclick: save, "저장" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 기본_설정은_그대로_돌아온다() {
        let d = SettingsDraft::from_settings(&ExamSettings::default());
        assert_eq!(d.to_settings().unwrap(), ExamSettings::default());
        assert!(d.changed(&d).is_empty());
    }

    #[test]
    fn 빈_기준표_항목은_막는다() {
        let mut d = SettingsDraft::from_settings(&ExamSettings::default());
        assert!(d.to_settings().is_ok());
        d.rubric.push(RubricItem {
            name: String::new(),
            description: String::new(),
        });
        assert!(d.to_settings().is_err());
    }

    #[test]
    fn 허용_언어는_하나_이상이어야_저장된다() {
        let mut d = SettingsDraft::from_settings(&ExamSettings::default());
        assert_eq!(d.languages.len(), 4);
        d.languages = vec![Language::Python];
        assert_eq!(
            d.to_settings().unwrap().allowed_languages,
            vec![Language::Python]
        );
        assert_eq!(
            d.changed(&SettingsDraft::from_settings(&ExamSettings::default())),
            vec!["허용 언어"]
        );
        d.languages.clear();
        assert!(d.to_settings().is_err());
    }

    #[test]
    fn 미리보기_계산식() {
        assert_eq!(preview_line(50), "예: 통과율 85% · AI 활용 71 → 최종 78.0");
    }
}
