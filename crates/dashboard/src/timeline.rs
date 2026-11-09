//! 응시자 상세의 편집 타임라인·근거 앵커 계산(#100). UI 없이 시험한다.
//!
//! 입력은 서버가 검증해 준 과정 로그 사건(코드 스냅숏)과 [`Activity`](제안·결정·수락 뒤 수정). 출력은 화면이 그대로
//! 그리는 줄들이다 — 스냅숏끼리 줄 diff, 제안 결정, 수락 뒤 수정이 처음 드러난 스냅숏 표시.

use promptlab_shared::activity::{Activity, AfterAccept, Decision, Proposal};
use promptlab_shared::api::ReplayEvent;
use promptlab_shared::evaluation::Evidence;
use promptlab_shared::log::LogEvent;

/// diff 한 줄의 종류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// 줄 수가 이보다 크면 LCS 표가 커지므로 통째 교체로 보인다(스냅숏 상한 64KB라 거의 닿지 않는다).
const MAX_DIFF_LINES: usize = 1500;

/// 줄 단위 diff(최장 공통 부분열). 줄 끝 `\r`은 무시하지 않고 그대로 비교한다.
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = if old.is_empty() {
        Vec::new()
    } else {
        old.split('\n').collect()
    };
    let b: Vec<&str> = if new.is_empty() {
        Vec::new()
    } else {
        new.split('\n').collect()
    };
    if a.len() > MAX_DIFF_LINES || b.len() > MAX_DIFF_LINES {
        let mut out: Vec<DiffLine> = a.iter().map(|t| line(DiffKind::Removed, t)).collect();
        out.extend(b.iter().map(|t| line(DiffKind::Added, t)));
        return out;
    }
    // lcs[i][j] = a[i..]와 b[j..]의 최장 공통 부분열 길이.
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(line(DiffKind::Same, a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line(DiffKind::Removed, a[i]));
            i += 1;
        } else {
            out.push(line(DiffKind::Added, b[j]));
            j += 1;
        }
    }
    out.extend(a[i..].iter().map(|t| line(DiffKind::Removed, t)));
    out.extend(b[j..].iter().map(|t| line(DiffKind::Added, t)));
    out
}

fn line(kind: DiffKind, text: &str) -> DiffLine {
    DiffLine {
        kind,
        text: text.to_owned(),
    }
}

/// 타임라인의 한 줄.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// 코드가 바뀐 스냅숏. 앞 스냅숏(없으면 빈 코드)과의 diff와 더한/뺀 줄 수.
    Snapshot {
        seq: u64,
        at_ms: i64,
        added: usize,
        removed: usize,
        diff: Vec<DiffLine>,
        /// 수락한 제안의 글이 처음 사라지거나 바뀐 스냅숏이면 그 제안 번호들(「수락 뒤 수정」).
        modifies: Vec<u32>,
    },
    /// AI 제안(결정·수락 뒤 수정 포함). 번호는 `proposal-{n}` 앵커.
    Proposal(Proposal),
}

impl Step {
    pub fn seq(&self) -> u64 {
        match self {
            Step::Snapshot { seq, .. } => *seq,
            Step::Proposal(p) => p.seq,
        }
    }
}

/// 사건 순서대로 코드 스냅숏 diff와 제안을 한 줄로 세운다. 앞과 같은 스냅숏은 건너뛴다.
pub fn edit_timeline(events: &[ReplayEvent], activity: &Activity) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    let mut prev: &str = "";
    for e in events {
        if let LogEvent::CodeSnapshot { code } = &e.event {
            if code == prev {
                continue;
            }
            let diff = diff_lines(prev, code);
            let added = diff.iter().filter(|d| d.kind == DiffKind::Added).count();
            let removed = diff.iter().filter(|d| d.kind == DiffKind::Removed).count();
            let modifies = activity
                .proposals
                .iter()
                .filter(|p| matches!(p.after_accept, Some(AfterAccept::Modified { seq: Some(s) }) if s == e.seq))
                .map(|p| p.proposal)
                .collect();
            steps.push(Step::Snapshot {
                seq: e.seq,
                at_ms: e.at_ms,
                added,
                removed,
                diff,
                modifies,
            });
            prev = code;
        }
    }
    steps.extend(activity.proposals.iter().cloned().map(Step::Proposal));
    steps.sort_by_key(Step::seq);
    steps
}

/// 근거 칩이 가리키는 요소 id(대화 칸의 턴·타임라인의 제안).
pub fn anchor(e: &Evidence) -> String {
    match e {
        Evidence::Turn(n) => format!("turn-{n}"),
        Evidence::Proposal(n) => format!("proposal-{n}"),
    }
}

/// 제안 줄 머리에 붙는 결정 설명(「수락 · 그대로 남음」 등).
pub fn decision_text(p: &Proposal) -> String {
    match (p.decision, p.after_accept) {
        (Decision::Accepted, Some(AfterAccept::Kept)) => "수락 · 최종 코드에 그대로 남음".into(),
        (Decision::Accepted, Some(AfterAccept::Modified { .. })) => {
            "수락 · 받아들인 뒤 응시자가 고침".into()
        }
        (d, _) => d.label().into(),
    }
}

/// 범위 표기(「3:1–5:12」).
pub fn range_text(p: &Proposal) -> String {
    format!(
        "{}:{}–{}:{}",
        p.range.start_line, p.range.start_column, p.range.end_line, p.range.end_column
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use promptlab_shared::log::EditRange;

    fn snap(seq: u64, code: &str) -> ReplayEvent {
        ReplayEvent {
            seq,
            at_ms: seq as i64 * 1000,
            received_at: 0,
            key_index: 0,
            event: LogEvent::CodeSnapshot { code: code.into() },
        }
    }

    fn kinds(d: &[DiffLine]) -> String {
        d.iter()
            .map(|l| match l.kind {
                DiffKind::Same => '=',
                DiffKind::Added => '+',
                DiffKind::Removed => '-',
            })
            .collect()
    }

    fn proposal(n: u32, seq: u64, after: Option<AfterAccept>) -> Proposal {
        Proposal {
            proposal: n,
            seq,
            at_ms: seq as i64 * 1000,
            turn: Some(1),
            range: EditRange {
                start_line: 1,
                start_column: 1,
                end_line: 2,
                end_column: 1,
            },
            text: "x".into(),
            decision: Decision::Accepted,
            decided_seq: Some(seq + 1),
            after_accept: after,
        }
    }

    #[test]
    fn diff_marks_added_removed_same() {
        let d = diff_lines("a\nb\nc", "a\nx\nc\nd");
        assert_eq!(kinds(&d), "=-+=+");
        assert_eq!(diff_lines("", "p\nq").len(), 2);
        assert_eq!(kinds(&diff_lines("p", "")), "-");
        assert!(diff_lines("", "").is_empty());
    }

    #[test]
    fn timeline_skips_identical_snapshots_and_orders_by_seq() {
        let events = [snap(1, "a"), snap(3, "a"), snap(5, "a\nb")];
        let activity = Activity {
            proposals: vec![proposal(1, 4, Some(AfterAccept::Kept))],
            ..Activity::default()
        };
        let steps = edit_timeline(&events, &activity);
        let seqs: Vec<u64> = steps.iter().map(Step::seq).collect();
        assert_eq!(seqs, [1, 4, 5]);
        match &steps[2] {
            Step::Snapshot { added, removed, .. } => assert_eq!((*added, *removed), (1, 0)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn modified_snapshot_names_its_proposal() {
        let events = [snap(1, "a"), snap(5, "b")];
        let activity = Activity {
            proposals: vec![
                proposal(2, 2, Some(AfterAccept::Modified { seq: Some(5) })),
                proposal(3, 3, Some(AfterAccept::Modified { seq: None })),
            ],
            ..Activity::default()
        };
        let steps = edit_timeline(&events, &activity);
        let modifies: Vec<&Vec<u32>> = steps
            .iter()
            .filter_map(|s| match s {
                Step::Snapshot { modifies, .. } => Some(modifies),
                _ => None,
            })
            .collect();
        assert_eq!(modifies, [&Vec::<u32>::new(), &vec![2]]);
    }

    #[test]
    fn evidence_anchors_match_chip_labels() {
        assert_eq!(anchor(&Evidence::Turn(3)), "turn-3");
        assert_eq!(anchor(&Evidence::Proposal(2)), "proposal-2");
        assert_eq!(Evidence::Proposal(2).label(), "제안 #2");
    }

    #[test]
    fn decision_text_reflects_after_accept() {
        let kept = proposal(1, 1, Some(AfterAccept::Kept));
        assert!(decision_text(&kept).contains("그대로"));
        let mut rejected = proposal(2, 2, None);
        rejected.decision = Decision::Rejected;
        assert_eq!(decision_text(&rejected), "거절");
    }
}
