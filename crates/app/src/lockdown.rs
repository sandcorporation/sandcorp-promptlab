//! 응시 모드 잠금(#12): 시험 문항을 푸는 동안 창을 전체화면으로 두고, 창 신호(포커스·전체화면)와 붙여넣기를
//! 과정 로그 사건으로 바꾼다(붙여넣기는 [`crate::editor::Clipboard`]가 막고 사건을 낸다, #97). 화면(`main.rs`)은 창 사건을 여기에 넘기고 돌려받은 사건을 로그에 남긴다.
//!
//! 앱이 할 수 있는 것은 창을 전체화면으로 만들고 빠져나가면 다시 돌리는 데까지다. Alt+Tab·Win 키·작업 관리자,
//! 다른 모니터·다른 기기는 막지 못한다(막으려 들지 않는다) — 대신 포커스 이탈·복귀와 전체화면 이탈을 기록하고,
//! 판단은 고객사가 한다(spec).

use promptlab_shared::log::LogEvent;

/// 잠금 상태. 잠그지 않았을 때(연습·문항 밖)는 아무 사건도 내지 않는다.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lockdown {
    active: bool,
    /// 포커스를 잃은 앱 시각(유닉스 밀리초).
    lost_at: Option<i64>,
    fullscreen: bool,
}

/// 전체화면 신호를 받은 뒤 화면이 할 일.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullscreenChange {
    /// 남길 사건.
    pub event: Option<LogEvent>,
    /// 창을 다시 전체화면으로 돌린다.
    pub restore: bool,
}

impl Lockdown {
    pub fn new() -> Self {
        Self::default()
    }

    /// 응시를 시작하며 잠근다. 화면은 창을 전체화면으로 돌린다.
    pub fn engage(&mut self) {
        self.active = true;
        self.lost_at = None;
    }

    /// 잠금을 푼다(제출·문항 끝). 화면은 전체화면을 푼다.
    pub fn release(&mut self) {
        *self = Self::default();
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 창 포커스가 바뀌었다. 잃으면 [`LogEvent::FocusLost`], 되찾으면 비운 시간과 [`LogEvent::FocusReturned`].
    pub fn focus_changed(&mut self, focused: bool, now_ms: i64) -> Option<LogEvent> {
        if !self.active {
            return None;
        }
        match (focused, self.lost_at) {
            (false, None) => {
                self.lost_at = Some(now_ms);
                Some(LogEvent::FocusLost)
            }
            (true, Some(at)) => {
                self.lost_at = None;
                Some(LogEvent::FocusReturned {
                    away_ms: u64::try_from(now_ms - at).unwrap_or(0),
                })
            }
            _ => None,
        }
    }

    /// 창의 전체화면 여부를 알린다(창 크기가 바뀔 때마다). 잠근 동안 빠져나가면 [`LogEvent::FullscreenExited`]를
    /// 남기고 다시 전체화면으로 돌린다. 전체화면에 들어가기 전의 크기 변화는 사건이 아니다.
    pub fn fullscreen_changed(&mut self, fullscreen: bool) -> FullscreenChange {
        let was = std::mem::replace(&mut self.fullscreen, fullscreen);
        let exited = self.active && was && !fullscreen;
        FullscreenChange {
            event: exited.then_some(LogEvent::FullscreenExited),
            restore: self.active && !fullscreen,
        }
    }
}
