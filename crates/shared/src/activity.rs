//! 응시 문항 하나의 코딩·AI 활용 기록(#99, spec #93 결정 7) — 검증된 과정 로그 사건에서 세운다.
//!
//! AI 활용 평가 입력([`crate::evaluation::evaluation_input`])·AI 리포트 입력·감독자용 원문 API(`…/items/{n}/activity`)가
//! 같은 것을 쓴다.
//!
//! - **턴**: 응시자 메시지([`LogEvent::AiMessage`]) 하나가 턴 하나를 연다(1부터). 그 뒤 AI 답([`LogEvent::AiReply`])은
//!   그 턴에 든다. 앱은 문항마다 대화가 하나라(#98) 대화 라벨은 없다. 도구 결과(`read_code`가 돌려준 코드 등)는 로그에
//!   없다 — 모델이 본 코드는 그 근처 [`LogEvent::CodeSnapshot`]이다(`read_code` 직전에 앱이 스냅숏을 강제한다).
//! - **제안**: [`LogEvent::EditProposed`]의 `proposal` 번호 그대로(「제안 #k」). 결정은 뒤의 `EditAccepted`·`EditRejected`.
//! - **수락 뒤 수정**([`AfterAccept`]): 사건이 아니라 여기서 계산한다. 수락한 제안이 넣은 글(앞뒤 빈 줄을 빼고 줄 끝 공백을
//!   무시한 줄들)이 **수락 뒤의 코드 스냅숏마다·최종 코드에** 연속한 줄로 그대로 있으면 `Kept`, 처음으로 없어진 곳이 있으면
//!   `Modified`(그 스냅숏의 순번, 최종 코드면 `None`)다. 넣은 글이 빈 제안(지우기)은 늘 `Kept`. 수락 뒤 스냅숏도 최종 코드도
//!   없으면 판정하지 않는다(`None`).

use serde::{Deserialize, Serialize};

use crate::coding::Language;
use crate::log::{EditRange, LocalRunKind, LogEvent, ToolCall};

/// 응시자 턴 하나(응시자 메시지 + 그 뒤 AI 답들).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    /// 1부터.
    pub turn: u32,
    pub seq: u64,
    /// 앱 시계의 유닉스 밀리초.
    pub at_ms: i64,
    /// 응시자 글 그대로.
    pub message: String,
    pub replies: Vec<Reply>,
}

/// AI 답 하나(글 + 도구 호출).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub seq: u64,
    pub at_ms: i64,
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    /// 응시자가 생성을 멈춘 답(#116, 글은 그때까지).
    #[serde(default)]
    pub stopped: bool,
}

/// 제안에 대한 응시자 결정.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Rejected,
    /// 결정하지 않았다(문항을 냈거나 끝나 제안이 남았다).
    Undecided,
}

impl Decision {
    pub fn label(self) -> &'static str {
        match self {
            Self::Accepted => "수락",
            Self::Rejected => "거절",
            Self::Undecided => "결정 없음",
        }
    }
}

/// 수락한 제안의 그 뒤.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AfterAccept {
    /// 넣은 글이 최종 코드까지 그대로 남았다.
    Kept,
    /// 응시자가 그 영역을 고쳤거나 지웠다. `seq` = 처음 달라진 코드 스냅숏(최종 코드에서 처음 알았으면 `None`).
    Modified { seq: Option<u64> },
}

/// AI 편집 제안 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// 「제안 #k」의 k(앱이 매긴 문항 안 번호).
    pub proposal: u32,
    pub seq: u64,
    pub at_ms: i64,
    /// 제안이 나온 턴(첫 응시자 메시지 앞이면 없다).
    pub turn: Option<u32>,
    pub range: EditRange,
    /// 그 범위에 넣을 글.
    pub text: String,
    pub decision: Decision,
    /// 결정 사건의 순번.
    pub decided_seq: Option<u64>,
    /// 수락했을 때만, 판정할 수 있을 때만.
    pub after_accept: Option<AfterAccept>,
}

/// 로컬 실행 한 번.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub seq: u64,
    pub at_ms: i64,
    /// 그때까지 열린 마지막 턴(없으면 대화 전).
    pub turn: Option<u32>,
    pub kind: LocalRunKind,
    pub passed: Option<u32>,
    pub total: Option<u32>,
    pub summary: String,
}

/// 문항 하나의 기록.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub turns: Vec<Turn>,
    pub proposals: Vec<Proposal>,
    pub runs: Vec<Run>,
    /// 고른 언어(바꾼 차례대로).
    pub languages: Vec<Language>,
    /// 코드 스냅숏 수.
    pub snapshots: u32,
    /// 첫 응시자 메시지 앞에 나온 AI 답 수(정상 흐름에서는 0).
    #[serde(default)]
    pub orphan_replies: u32,
}

/// 로그 사건 하나(순번·앱 시각·사건).
pub type Entry<'a> = (u64, i64, &'a LogEvent);

/// 비교할 줄들: 앞뒤 빈 줄을 빼고 줄 끝 공백(`\r` 포함)을 지운다.
fn block_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').map(str::trim_end).collect();
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// `code`에 `text`의 줄들이 연속해 그대로 있나(줄 끝 공백은 무시). 빈 글이면 늘 있다.
pub fn block_present(code: &str, text: &str) -> bool {
    let block = block_lines(text);
    if block.is_empty() {
        return true;
    }
    let lines: Vec<&str> = code.split('\n').map(str::trim_end).collect();
    lines.windows(block.len()).any(|w| w == block.as_slice())
}

impl Activity {
    /// 검증된 로그 사건(순번 차례)과 최종 코드(제출 코드 — 없으면 `None`)로 세운다.
    pub fn from_events<'a>(
        events: impl IntoIterator<Item = Entry<'a>>,
        final_code: Option<&str>,
    ) -> Self {
        let mut out = Self::default();
        let mut snapshots: Vec<(u64, &str)> = Vec::new();
        for (seq, at_ms, event) in events {
            let turn = out.turns.last().map(|t| t.turn);
            match event {
                LogEvent::LanguageChosen { language } => out.languages.push(*language),
                LogEvent::CodeSnapshot { code } => {
                    out.snapshots += 1;
                    snapshots.push((seq, code));
                }
                LogEvent::AiMessage { text } => out.turns.push(Turn {
                    turn: turn.unwrap_or(0) + 1,
                    seq,
                    at_ms,
                    message: text.clone(),
                    replies: Vec::new(),
                }),
                LogEvent::AiReply {
                    text,
                    tool_calls,
                    stopped,
                } => match out.turns.last_mut() {
                    Some(t) => t.replies.push(Reply {
                        seq,
                        at_ms,
                        text: text.clone(),
                        tool_calls: tool_calls.clone(),
                        stopped: *stopped,
                    }),
                    None => out.orphan_replies += 1,
                },
                LogEvent::EditProposed {
                    proposal,
                    range,
                    text,
                } => out.proposals.push(Proposal {
                    proposal: *proposal,
                    seq,
                    at_ms,
                    turn,
                    range: *range,
                    text: text.clone(),
                    decision: Decision::Undecided,
                    decided_seq: None,
                    after_accept: None,
                }),
                LogEvent::EditAccepted { proposal } => {
                    out.decide(*proposal, Decision::Accepted, seq)
                }
                LogEvent::EditRejected { proposal } => {
                    out.decide(*proposal, Decision::Rejected, seq)
                }
                LogEvent::LocalRun {
                    kind,
                    passed,
                    total,
                    summary,
                } => out.runs.push(Run {
                    seq,
                    at_ms,
                    turn,
                    kind: *kind,
                    passed: *passed,
                    total: *total,
                    summary: summary.clone(),
                }),
                _ => {}
            }
        }
        for p in &mut out.proposals {
            let (Decision::Accepted, Some(decided)) = (p.decision, p.decided_seq) else {
                continue;
            };
            let later: Vec<(Option<u64>, &str)> = snapshots
                .iter()
                .filter(|(seq, _)| *seq > decided)
                .map(|(seq, code)| (Some(*seq), *code))
                .chain(final_code.map(|c| (None, c)))
                .collect();
            if later.is_empty() {
                continue;
            }
            p.after_accept = Some(
                match later.iter().find(|(_, code)| !block_present(code, &p.text)) {
                    Some((seq, _)) => AfterAccept::Modified { seq: *seq },
                    None => AfterAccept::Kept,
                },
            );
        }
        out
    }

    /// 아직 결정하지 않은 같은 번호의 제안에 결정을 단다(이미 결정했으면 처음 결정이 남는다).
    fn decide(&mut self, proposal: u32, decision: Decision, seq: u64) {
        if let Some(p) = self
            .proposals
            .iter_mut()
            .rev()
            .find(|p| p.proposal == proposal && p.decision == Decision::Undecided)
        {
            p.decision = decision;
            p.decided_seq = Some(seq);
        }
    }

    pub fn has_turn(&self, turn: u32) -> bool {
        self.turns.iter().any(|t| t.turn == turn)
    }

    pub fn has_proposal(&self, proposal: u32) -> bool {
        self.proposals.iter().any(|p| p.proposal == proposal)
    }

    /// 수락한 제안 수·그중 뒤에 고친 수·거절한 제안 수.
    pub fn decision_counts(&self) -> (u32, u32, u32) {
        let mut accepted = 0;
        let mut modified = 0;
        let mut rejected = 0;
        for p in &self.proposals {
            match p.decision {
                Decision::Accepted => {
                    accepted += 1;
                    if matches!(p.after_accept, Some(AfterAccept::Modified { .. })) {
                        modified += 1;
                    }
                }
                Decision::Rejected => rejected += 1,
                Decision::Undecided => {}
            }
        }
        (accepted, modified, rejected)
    }
}
