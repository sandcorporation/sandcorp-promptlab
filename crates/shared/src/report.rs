//! AI 리포트·의심 요약과 토큰·비용 추적(#15).
//!
//! - 리포트 모델·단가 표의 정본은 이 파일 하나다([`REPORT_MODEL`], [`REPORT_PRICES`]). 단가를 바꾸면 여기만 고친다 —
//!   비용 기록은 기록 시점 단가를 함께 남기므로 과거 비용은 그대로다.
//! - 리포트는 **점수를 바꾸지 않는다**(주 점수는 숨은 검사 통과율). 의심은 신호로만 요약하고 판단은 고객사가 한다
//!   — 응답마다 [`REPORT_DISCLAIMER`]를 싣는다.
//! - 리포트를 보는 사람은 결과 보기 역할뿐이다. 응시자(채용·사내 진단 모두)에게는 내보내지 않는다.

use serde::{Deserialize, Serialize};

/// AI 리포트를 쓰는 모델(Anthropic Messages API 모델 ID).
pub const REPORT_MODEL: &str = "claude-sonnet-5-5";

/// AI 리포트 단가(USD / 백만 토큰) — Claude Sonnet 5.5: 입력 $2, 출력 $10, 캐시 쓰기(5분) $2.50, 캐시 읽기 $0.10.
/// 출처: https://platform.claude.com/docs/en/about-claude/pricing (2026-11-06 확인). 프롬프트 캐시 최소 길이는 286토큰.
pub const REPORT_PRICES: UnitPrices = UnitPrices {
    input: 2.0,
    output: 10.0,
    cache_read: 0.10,
    cache_write: 2.5,
};

/// 로컬 모델(채점 워커 llama.cpp) 단가 — 우리 CPU라 0이다. 토큰 수·처리 시간만 쌓는다.
pub const LOCAL_PRICES: UnitPrices = UnitPrices {
    input: 0.0,
    output: 0.0,
    cache_read: 0.0,
    cache_write: 0.0,
};

/// 배정 하나에서 리포트를 만들 수 있는 최대 횟수(자동 생성 1회 + 재생성 포함).
pub const MAX_GENERATIONS: u32 = 3;

/// 리포트 응답마다 싣는 고정 문구.
pub const REPORT_DISCLAIMER: &str = "AI 리포트는 참고용이며 점수를 바꾸지 않습니다. 점수는 숨은 검사 통과율입니다. \
의심 요약은 기록된 신호를 정리한 것일 뿐 부정행위를 판정하지 않습니다 — 판단은 담당자가 합니다.";

/// 단가(USD / 백만 토큰).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UnitPrices {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// 호출 하나의 토큰 수. `input_tokens`는 캐시에 걸리지 않은 입력만이다(Anthropic `usage`와 같은 뜻).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl UnitPrices {
    /// 추정 비용(USD) = 단가 × 토큰.
    pub fn cost_usd(&self, usage: &TokenUsage) -> f64 {
        let per = |price: f64, tokens: u64| price * tokens as f64 / 1_000_000.0;
        per(self.input, usage.input_tokens)
            + per(self.output, usage.output_tokens)
            + per(self.cache_read, usage.cache_read_tokens)
            + per(self.cache_write, usage.cache_write_tokens)
    }
}

/// 비용 기록의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageKind {
    /// AI 리포트 호출 하나(Claude).
    Report,
    /// AI 활용 평가 하나(Claude 기준표, #99).
    Evaluation,
}

impl UsageKind {
    pub const ALL: [UsageKind; 2] = [Self::Report, Self::Evaluation];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Evaluation => "evaluation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Report => "AI 리포트",
            Self::Evaluation => "AI 활용 평가",
        }
    }
}

/// 리포트 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// 아직 없다(채점이 다 끝나지 않았거나 자동 생성 대상이 아니다). 결과 보기 역할이 요청하면 만든다.
    None,
    Queued,
    Generating,
    Done,
    Failed,
    /// 서버에 Anthropic API 키가 없어 리포트 기능이 꺼져 있다.
    Disabled,
}

impl ReportStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "없음",
            Self::Queued => "대기 중",
            Self::Generating => "만드는 중",
            Self::Done => "완료",
            Self::Failed => "실패",
            Self::Disabled => "꺼짐",
        }
    }
}

/// 모델이 쓴 리포트 본문(구조화 출력 JSON 그대로).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportBody {
    /// 강점.
    pub strengths: Vec<String>,
    /// 약점.
    pub weaknesses: Vec<String>,
    /// 개선 제안.
    pub suggestions: Vec<String>,
    /// 의심 신호 요약(단정하지 않는 문장). 신호가 없으면 없다고 쓴다.
    pub suspicion_summary: String,
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/report`(결과 보기).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportView {
    pub status: ReportStatus,
    /// `Done`일 때.
    pub report: Option<ReportBody>,
    /// `Failed`일 때 짧은 까닭(키·요청 본문은 싣지 않는다).
    pub failure_reason: Option<String>,
    /// 마지막으로 만든(또는 실패한) 시각, 유닉스 초.
    pub finished_at: Option<i64>,
    /// 리포트를 쓴 모델.
    pub model: Option<String>,
    /// 지금까지 만든 횟수(실패 포함).
    pub generations: u32,
    pub max_generations: u32,
    /// 지금 (다시) 만들기를 요청할 수 있는가.
    pub can_request: bool,
    /// [`REPORT_DISCLAIMER`].
    pub disclaimer: String,
}

/// 종류 하나의 사용량 합.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub duration_ms: u64,
    pub cost_usd: f64,
}

/// 종류별 사용량.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindUsage {
    pub kind: UsageKind,
    pub totals: UsageTotals,
}

/// 시험 하나의 사용량.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExamUsage {
    pub exam_id: String,
    pub title: String,
    /// 응시 수 = 배정을 받은 초대 수.
    pub attempts: u64,
    pub by_kind: Vec<KindUsage>,
}

/// `GET /orgs/{id}/usage`(구성원 관리 = 관리자) — 조직의 응시 수·토큰·추정 비용(spec 21).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgUsage {
    pub attempts: u64,
    /// 조직 전체(시험 밖의 미리 돌리기 포함) 종류별 합.
    pub by_kind: Vec<KindUsage>,
    /// 추정 비용 합(USD).
    pub cost_usd: f64,
    pub exams: Vec<ExamUsage>,
}
