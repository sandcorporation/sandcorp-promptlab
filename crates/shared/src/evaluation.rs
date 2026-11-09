//! AI 활용 평가(Claude 기준표)와 최종 점수(#99, spec #93 결정 7).
//!
//! - **기준표** = 공통 4항목([`COMMON_CRITERIA`]: 지시 품질·검증·AI 제안 비판·효율) + 시험의 추가 항목
//!   ([`crate::exam::RubricItem`], 최대 2). 항목마다 0~[`MAX_CRITERION_SCORE`]점. 키·이름·설명은 여기 한 곳.
//! - **AI 활용 점수** = 항목 합 / 만점 × 100([`evaluation_score`]).
//! - **문항 최종 점수** = 히든 테스트 통과율(0~100) × w + AI 활용 × (1 − w), w = 시험의 `score_weight`(%)
//!   ([`item_score`]). **시험 점수** = 문항 최종 점수의 평균([`exam_final_score`]).
//! - **평가가 없을 때**(스모크 조직·키 없음·개인 연습·평가 실패·자동 평가 창이 지난 제출)는 최종 = 통과율만이다
//!   ([`ScoreBasis::HiddenOnly`]). 평가가 빠진 까닭이 응시자 몫이 아니므로 0점으로 깎지 않는다. 응답의
//!   [`ItemScore::basis`]·[`ItemScore::formula`]가 어떤 식으로 계산했는지 드러낸다.
//! - 채점 실패 문항은 0점([`ScoreBasis::GradingFailed`]).
//! - **근거**([`Evidence`])는 「턴 n」 또는 「제안 #k」 — 서버가 실제 있는 것인지 [`Activity`]로 검사해 없는 근거만
//!   버린다(점수는 남긴다, [`accept`]).
//! - **평가 입력**([`evaluation_input`]): 공개 문제(설명·서명·공개 예제)·기준표·AI 대화(턴)·편집 제안과 결정·수락 뒤
//!   수정·로컬 실행 결과·최종 코드·히든 통과 수(숫자). **보내지 않는 것**: 히든 테스트 입력·기대값·케이스별 결과·
//!   채점 상세(`failure_detail`)·응시자 이름·이메일 — 이 함수는 그런 값을 받지 않는다(타입으로 막는다).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::activity::{Activity, AfterAccept, Decision};
use crate::coding::{Language, PublicCodingProblem};
use crate::exam::RubricItem;
use crate::log::LocalRunKind;

/// 기준표 항목 하나의 만점.
pub const MAX_CRITERION_SCORE: u8 = 5;
/// 평가 큐에서 한 제출을 시도하는 최대 횟수(처음 포함). 형식이 맞지 않는 응답·일시 오류 뒤 다시 큐로.
pub const MAX_ATTEMPTS: u32 = 3;
/// 근거는 항목마다 이만큼까지 남긴다.
pub const MAX_EVIDENCE_PER_CRITERION: usize = 5;
/// 항목 근거(짧은 이유) 최대 글자 수.
pub const MAX_REASON_CHARS: usize = 400;
/// 평가 입력에서 메시지·답·제안 글 하나를 이 글자 수까지만 싣는다.
pub const MAX_TEXT_CHARS: usize = 2_000;
/// 평가 입력의 최종 코드 상한(글자).
pub const MAX_CODE_CHARS: usize = 20_000;
/// 평가 입력에 싣는 턴·로컬 실행 수 상한(뒤의 것부터).
pub const MAX_TURNS: usize = 60;
pub const MAX_RUNS: usize = 60;

/// 기준표 항목 하나(이름·설명은 Claude에게도, 화면에도 간다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Criterion {
    /// 고정 키: 공통은 `instruction`·`verification`·`critique`·`efficiency`, 추가는 `extra_1`·`extra_2`.
    pub key: String,
    pub name: String,
    pub description: String,
}

/// 공통 4항목(키, 이름, 설명).
pub const COMMON_CRITERIA: [(&str, &str, &str); 4] = [
    (
        "instruction",
        "지시 품질",
        "AI에게 무엇을 어떻게 시켰는가: 문제 조건·입출력·경계 사례·원하는 범위를 분명히 주고, 필요한 만큼만 맡겼는가.",
    ),
    (
        "verification",
        "검증",
        "AI 답과 자기 코드를 로컬 실행·공개 예제·직접 만든 입력으로 확인했는가(수락한 뒤 실행해 보았는가).",
    ),
    (
        "critique",
        "AI 제안 비판",
        "AI 편집 제안을 그대로 받지 않고 따져 보았는가: 틀린 제안을 거절하거나, 수락한 뒤 문제를 고치고, 그 근거가 대화·실행에 드러나는가.",
    ),
    (
        "efficiency",
        "효율",
        "불필요한 턴·되풀이 요청 없이 적은 왕복으로 풀이에 다가갔는가(AI가 필요 없는 일까지 묻지 않았는가).",
    ),
];

/// 시험의 기준표 = 공통 4항목 + 추가 항목(차례대로 `extra_1`, `extra_2`).
pub fn rubric(extra: &[RubricItem]) -> Vec<Criterion> {
    COMMON_CRITERIA
        .iter()
        .map(|(key, name, description)| Criterion {
            key: (*key).to_owned(),
            name: (*name).to_owned(),
            description: (*description).to_owned(),
        })
        .chain(extra.iter().enumerate().map(|(i, item)| Criterion {
            key: format!("extra_{}", i + 1),
            name: item.name.clone(),
            description: item.description.clone(),
        }))
        .collect()
}

/// 근거 하나: 「턴 n」 또는 「제안 #k」.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "n", rename_all = "snake_case")]
pub enum Evidence {
    /// 응시자 턴 번호(1부터, [`crate::activity::Turn::turn`]).
    Turn(u32),
    /// 편집 제안 번호([`crate::activity::Proposal::proposal`]).
    Proposal(u32),
}

impl Evidence {
    /// 화면 표기(「턴 3」·「제안 #2」).
    pub fn label(self) -> String {
        match self {
            Self::Turn(n) => format!("턴 {n}"),
            Self::Proposal(n) => format!("제안 #{n}"),
        }
    }

    /// 기록에 실제 있는 것인가.
    pub fn exists_in(self, activity: &Activity) -> bool {
        match self {
            Self::Turn(n) => activity.has_turn(n),
            Self::Proposal(n) => activity.has_proposal(n),
        }
    }
}

/// 항목 하나의 점수·근거.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CriterionScore {
    pub key: String,
    pub name: String,
    pub score: u8,
    pub max: u8,
    /// 짧은 근거 문장.
    pub reason: String,
    /// 근거(서버가 실제 있는 것만 남긴 것).
    pub evidence: Vec<Evidence>,
}

/// 문항 하나의 AI 활용 평가 결과(저장·응답 꼴).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub criteria: Vec<CriterionScore>,
    /// AI 활용 점수(0~100) = 항목 합 / 만점 × 100.
    pub score: f64,
    /// 평가한 모델.
    pub model: String,
    /// 기록에 없는 턴·제안을 가리켜 버린 근거 수.
    #[serde(default)]
    pub dropped_evidence: u32,
}

/// 항목 합 / 만점 × 100. 항목이 없으면 0.
pub fn evaluation_score(criteria: &[CriterionScore]) -> f64 {
    let max: u32 = criteria.iter().map(|c| u32::from(c.max)).sum();
    if max == 0 {
        return 0.0;
    }
    let sum: u32 = criteria.iter().map(|c| u32::from(c.score.min(c.max))).sum();
    f64::from(sum) / f64::from(max) * 100.0
}

/// 모델이 낸 항목 하나(구조화 출력 그대로).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCriterion {
    pub key: String,
    pub score: i64,
    pub reason: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

/// 모델 응답 전체(구조화 출력 그대로).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEvaluation {
    pub criteria: Vec<RawCriterion>,
}

fn clip(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…(이하 생략)")
    } else {
        head
    }
}

/// 모델 응답을 받아들인다: 기준표 항목이 모두 있고 점수가 0~만점이어야 한다(아니면 `None` — 형식 오류). 기록에 없는
/// 근거는 그것만 버리고 센다, 겹친 근거는 하나로, 항목마다 [`MAX_EVIDENCE_PER_CRITERION`]개까지.
pub fn accept(
    raw: &RawEvaluation,
    rubric: &[Criterion],
    activity: &Activity,
    model: &str,
) -> Option<Evaluation> {
    let mut dropped = 0u32;
    let mut criteria = Vec::with_capacity(rubric.len());
    for criterion in rubric {
        let found = raw.criteria.iter().find(|c| c.key == criterion.key)?;
        let score = u8::try_from(found.score)
            .ok()
            .filter(|s| *s <= MAX_CRITERION_SCORE)?;
        let mut evidence: Vec<Evidence> = Vec::new();
        for e in &found.evidence {
            if !e.exists_in(activity) {
                dropped += 1;
            } else if !evidence.contains(e) && evidence.len() < MAX_EVIDENCE_PER_CRITERION {
                evidence.push(*e);
            }
        }
        criteria.push(CriterionScore {
            key: criterion.key.clone(),
            name: criterion.name.clone(),
            score,
            max: MAX_CRITERION_SCORE,
            reason: clip(found.reason.trim(), MAX_REASON_CHARS),
            evidence,
        });
    }
    Some(Evaluation {
        score: evaluation_score(&criteria),
        criteria,
        model: model.to_owned(),
        dropped_evidence: dropped,
    })
}

/// 평가 입력의 채점 요약 — 히든 테스트는 **통과 수 숫자만**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenPassCount {
    pub passed: u32,
    pub total: u32,
}

/// 평가 입력의 최종 코드(제출 코드).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalCode<'a> {
    pub language: Language,
    pub code: &'a str,
}

fn after_accept_text(p: Option<AfterAccept>) -> &'static str {
    match p {
        Some(AfterAccept::Kept) => "넣은 코드가 최종까지 그대로",
        Some(AfterAccept::Modified { .. }) => "수락 뒤 응시자가 그 영역을 고치거나 지움",
        None => "그 뒤 코드 기록 없음",
    }
}

/// 평가 모델에 보낼 입력(사용자 메시지). 받는 값 밖의 것(히든 입력·기대값·케이스별 결과·이름·이메일)은 실을 수 없다.
pub fn evaluation_input(
    problem: Option<&PublicCodingProblem>,
    rubric: &[Criterion],
    activity: &Activity,
    final_code: Option<FinalCode<'_>>,
    hidden: Option<HiddenPassCount>,
    model: Option<&str>,
) -> String {
    let mut out = String::from("# 문제(공개)\n");
    match problem {
        Some(p) => {
            let _ = writeln!(out, "제목: {}", clip(&p.title, 200));
            let _ = writeln!(out, "설명:\n{}", clip(&p.description, 4_000));
            let params: Vec<String> = p
                .signature
                .params
                .iter()
                .map(|q| format!("{}: {}", q.name, q.ty))
                .collect();
            let _ = writeln!(
                out,
                "함수: {}({}) -> {}",
                p.signature.name,
                params.join(", "),
                p.signature.returns
            );
            let _ = writeln!(out, "공개 예제 {}개:", p.examples.len());
            for (i, ex) in p.examples.iter().enumerate() {
                let args = serde_json::to_string(&ex.args).unwrap_or_default();
                let expected = serde_json::to_string(&ex.expected).unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {}. 입력 {} → 기대 {}",
                    i + 1,
                    clip(&args, 400),
                    clip(&expected, 200)
                );
            }
        }
        None => out.push_str("(문제를 찾지 못했다)\n"),
    }
    let _ = writeln!(out, "\n# 기준표(항목마다 0~{MAX_CRITERION_SCORE}점)");
    for c in rubric {
        let _ = writeln!(out, "- {} — {}: {}", c.key, c.name, c.description);
    }

    let (accepted, modified, rejected) = activity.decision_counts();
    let _ = writeln!(
        out,
        "\n# 요약\nAI 턴 {}개, 편집 제안 {}개(수락 {accepted} — 그중 수락 뒤 고침 {modified}, 거절 {rejected}), 로컬 실행 {}번, 코드 스냅숏 {}개",
        activity.turns.len(),
        activity.proposals.len(),
        activity.runs.len(),
        activity.snapshots
    );
    let languages: Vec<&str> = activity
        .languages
        .iter()
        .map(|l| l.display_name())
        .collect();
    let _ = writeln!(
        out,
        "고른 언어: {}",
        if languages.is_empty() {
            "-".to_owned()
        } else {
            languages.join(" → ")
        }
    );
    if let Some(model) = model {
        let _ = writeln!(
            out,
            "AI 도우미 모델: {} — 응시자 PC에 맞춰 고른 크기라 제안 품질이 다를 수 있다. 모델의 성능이 아니라 응시자가 AI를 어떻게 썼는지를 평가한다.",
            clip(model, 80)
        );
    }
    match hidden {
        Some(h) => {
            let _ = writeln!(
                out,
                "서버 히든 테스트 통과: {}/{} (숫자만 — 케이스 내용은 주지 않는다)",
                h.passed, h.total
            );
        }
        None => out.push_str("서버 히든 테스트: 결과 없음\n"),
    }

    let _ = writeln!(
        out,
        "\n# AI 대화({}턴 — 근거는 「턴 n」으로 단다)",
        activity.turns.len()
    );
    let skip = activity.turns.len().saturating_sub(MAX_TURNS);
    if skip > 0 {
        let _ = writeln!(out, "…(앞의 {skip}턴 생략)");
    }
    for t in activity.turns.iter().skip(skip) {
        let _ = writeln!(
            out,
            "[턴 {}] 응시자: {}",
            t.turn,
            clip(&t.message, MAX_TEXT_CHARS)
        );
        for r in &t.replies {
            let calls: Vec<&str> = r.tool_calls.iter().map(|c| c.name.as_str()).collect();
            let _ = writeln!(
                out,
                "[턴 {}] AI: {}{}",
                t.turn,
                clip(r.text.trim(), MAX_TEXT_CHARS),
                if calls.is_empty() {
                    String::new()
                } else {
                    format!(" (도구 호출: {})", calls.join(", "))
                }
            );
        }
    }

    let _ = writeln!(
        out,
        "\n# AI 편집 제안({}개 — 근거는 「제안 #k」로 단다)",
        activity.proposals.len()
    );
    for p in &activity.proposals {
        let _ = writeln!(
            out,
            "[제안 #{}] {}에 나옴, 줄 {}~{} 바꿈 → {}{}\n```\n{}\n```",
            p.proposal,
            p.turn
                .map_or_else(|| "대화 전".to_owned(), |t| format!("턴 {t}")),
            p.range.start_line,
            p.range.end_line,
            p.decision.label(),
            if p.decision == Decision::Accepted {
                format!("({})", after_accept_text(p.after_accept))
            } else {
                String::new()
            },
            clip(&p.text, MAX_TEXT_CHARS)
        );
    }

    let _ = writeln!(out, "\n# 로컬 실행({}번, 시간 차례)", activity.runs.len());
    let skip = activity.runs.len().saturating_sub(MAX_RUNS);
    if skip > 0 {
        let _ = writeln!(out, "…(앞의 {skip}번 생략)");
    }
    for r in activity.runs.iter().skip(skip) {
        let kind = match r.kind {
            LocalRunKind::Run => "직접 입력 실행",
            LocalRunKind::Examples => "공개 예제 실행",
        };
        let counts = match (r.passed, r.total) {
            (Some(p), Some(t)) => format!(" {p}/{t} 통과"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "- {} 뒤: {kind}{counts} — {}",
            r.turn
                .map_or_else(|| "대화 전".to_owned(), |t| format!("턴 {t}")),
            clip(&r.summary, 300)
        );
    }

    match final_code {
        Some(c) => {
            let _ = writeln!(
                out,
                "\n# 최종 코드(제출, {})\n```\n{}\n```",
                c.language.display_name(),
                clip(c.code, MAX_CODE_CHARS)
            );
        }
        None => out.push_str("\n# 최종 코드\n(없음)\n"),
    }
    out
}

/// AI 활용 평가의 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    /// 평가 큐에서 기다린다(채점이 막 끝나 큐에 들어가기 전도 여기).
    Queued,
    /// Claude가 평가하는 중.
    Evaluating,
    Done,
    /// 다시 해도 실패했다(최종 = 통과율만).
    Failed,
    /// 서버에 Claude 키가 없다(평가 꺼짐).
    Disabled,
    /// 스모크 조직 — 평가하지 않는다.
    Smoke,
    /// 개인 연습 — 과정 로그가 없어 평가하지 않는다.
    Practice,
    /// 평가하지 않는 제출: 채점 실패·자동 평가 창이 지난 제출.
    NotEvaluated,
}

impl EvaluationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "평가 대기",
            Self::Evaluating => "평가 중",
            Self::Done => "평가 완료",
            Self::Failed => "평가 실패",
            Self::Disabled => "AI 활용 평가 꺼짐",
            Self::Smoke => "스모크 — 평가 안 함",
            Self::Practice => "개인 연습 — 평가 없음",
            Self::NotEvaluated => "평가 없음",
        }
    }

    /// 아직 끝나지 않았나(최종 점수를 기다린다).
    pub fn pending(self) -> bool {
        matches!(self, Self::Queued | Self::Evaluating)
    }
}

/// 최종 점수를 어떤 식으로 냈나.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreBasis {
    /// 통과율 × w + AI 활용 × (1 − w).
    Weighted,
    /// AI 활용 평가가 없어 히든 테스트 통과율만.
    HiddenOnly,
    /// 채점 실패 — 0점.
    GradingFailed,
    /// 채점 또는 AI 활용 평가를 기다린다(최종 점수 없음).
    Pending,
}

/// 문항 하나의 최종 점수와 계산식. 점수는 모두 0~100.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemScore {
    /// 최종 점수. `basis`가 `Pending`이면 없다.
    pub final_score: Option<f64>,
    /// 히든 테스트 통과율 × 100(채점이 끝났을 때).
    pub hidden_score: Option<f64>,
    /// AI 활용 점수(평가가 끝났을 때).
    pub evaluation_score: Option<f64>,
    /// 통과율 비율 w(%). AI 활용 비율은 100 − w.
    pub weight: u8,
    pub basis: ScoreBasis,
    /// 사람이 읽는 계산식(예: 「최종 72.5 = 통과율 80.0 × 50% + AI 활용 65.0 × 50%」).
    pub formula: String,
    pub evaluation_status: EvaluationStatus,
    /// 평가가 끝났으면 항목·근거.
    pub evaluation: Option<Evaluation>,
}

/// 문항의 채점 상태(최종 점수 계산의 입력).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Graded {
    /// 아직 채점하지 않았다.
    Pending,
    /// 히든 테스트 통과율(0.0~1.0).
    Done(f64),
    Failed,
}

fn one(x: f64) -> String {
    format!("{x:.1}")
}

/// 문항 최종 점수를 낸다. `evaluation`은 `status`가 `Done`일 때만 쓴다.
pub fn item_score(
    graded: Graded,
    weight: u8,
    status: EvaluationStatus,
    evaluation: Option<Evaluation>,
) -> ItemScore {
    let weight = weight.min(100);
    let w = f64::from(weight) / 100.0;
    let evaluation = evaluation.filter(|_| status == EvaluationStatus::Done);
    let evaluation_score = evaluation.as_ref().map(|e| e.score);
    let base = |final_score, hidden_score, basis, formula| ItemScore {
        final_score,
        hidden_score,
        evaluation_score,
        weight,
        basis,
        formula,
        evaluation_status: status,
        evaluation: evaluation.clone(),
    };
    match graded {
        Graded::Pending => base(None, None, ScoreBasis::Pending, "채점을 기다린다".into()),
        Graded::Failed => base(
            Some(0.0),
            None,
            ScoreBasis::GradingFailed,
            "채점 실패 — 최종 0점".into(),
        ),
        Graded::Done(rate) => {
            let hidden = rate.clamp(0.0, 1.0) * 100.0;
            match evaluation_score {
                Some(eval) => {
                    let total = hidden * w + eval * (1.0 - w);
                    base(
                        Some(total),
                        Some(hidden),
                        ScoreBasis::Weighted,
                        format!(
                            "최종 {} = 통과율 {} × {weight}% + AI 활용 {} × {}%",
                            one(total),
                            one(hidden),
                            one(eval),
                            100 - weight
                        ),
                    )
                }
                // w = 100이면 AI 활용 평가를 기다릴 까닭이 없다.
                None if weight == 100 => base(
                    Some(hidden),
                    Some(hidden),
                    ScoreBasis::Weighted,
                    format!("최종 {} = 통과율 {} × 100%", one(hidden), one(hidden)),
                ),
                None if status.pending() => base(
                    None,
                    Some(hidden),
                    ScoreBasis::Pending,
                    format!(
                        "통과율 {} — AI 활용 평가를 기다린다(최종 = 통과율 × {weight}% + AI 활용 × {}%)",
                        one(hidden),
                        100 - weight
                    ),
                ),
                None => base(
                    Some(hidden),
                    Some(hidden),
                    ScoreBasis::HiddenOnly,
                    format!(
                        "최종 {} = 통과율 {}만({} — AI 활용 없이)",
                        one(hidden),
                        one(hidden),
                        status.label()
                    ),
                ),
            }
        }
    }
}

/// 시험 점수 = 문항 최종 점수의 평균(0~100). 문항이 없거나 하나라도 최종 점수가 없으면(채점·평가 대기, 안 낸
/// 문항) `None`.
pub fn exam_final_score<'a>(items: impl IntoIterator<Item = Option<&'a ItemScore>>) -> Option<f64> {
    let mut sum = 0.0;
    let mut n = 0u32;
    for item in items {
        sum += item?.final_score?;
        n += 1;
    }
    (n > 0).then(|| sum / f64::from(n))
}

/// 응시의 AI 활용 평균(0~100) = 평가가 끝난 문항들의 AI 활용 점수 평균. 평가된 문항이 없으면 `None`.
pub fn exam_evaluation_score<'a>(
    items: impl IntoIterator<Item = Option<&'a ItemScore>>,
) -> Option<f64> {
    let scores: Vec<f64> = items
        .into_iter()
        .filter_map(|i| i?.evaluation_score)
        .collect();
    (!scores.is_empty()).then(|| scores.iter().sum::<f64>() / scores.len() as f64)
}
