//! 응시 화면(#74·#97)의 UI 밖 로직: 창 폭에 따른 배치, 키보드 단축키 → 동작, 숫자·점수 글자.
//! 화면(`ui/`)은 이것을 그리기만 한다 — 시험은 `tests/workspace.rs`.

/// 응시 중 화면이 3칸(문제 | 편집기 | AI 패널)이 되는 최소 창 폭(px).
pub const THREE_PANE_MIN_WIDTH: u32 = 1440;
/// 응시 전 점검이 2칸(카드 둘씩)이 되는 최소 창 폭(px). 미만이면 1칸.
pub const CHECK_TWO_COLUMN_MIN_WIDTH: u32 = 1280;
/// 왼쪽 내비게이션이 아이콘만 남는 최대 창 폭(px).
pub const NAV_COLLAPSE_MAX_WIDTH: u32 = 1280;

/// 응시 중 화면의 칸 배치.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneLayout {
    /// 문제 380 | 편집기 가변 | AI 패널 360.
    Three,
    /// 문제 칸이 48px 막대로 접힘(눌러 펼친다) | 편집기 | AI 패널 320.
    Folded,
}

impl PaneLayout {
    /// 창 폭(px)으로 배치를 고른다.
    pub fn for_width(width: u32) -> Self {
        if width >= THREE_PANE_MIN_WIDTH {
            Self::Three
        } else {
            Self::Folded
        }
    }
}

/// 문제 칸 최소 폭(px): 문제 설명·예제 `pre`가 줄바꿈으로 읽히는 하한(이보다 좁으면 함수 서명이 잘린다).
pub const MIN_PROBLEM_WIDTH: u32 = 280;
/// 편집기 칸 최소 폭(px): 도구 막대(언어 140 + 단추 둘·제출 88씩 + 여백)가 한 줄에 들어가고 코드 줄이 40자 안팎 보이는 하한.
pub const MIN_WORK_WIDTH: u32 = 480;
/// AI 칸 최소 폭(px): 입력칸·진행 줄·diff 제안 단추가 줄바꿈 없이 들어가는 하한.
pub const MIN_AI_WIDTH: u32 = 300;
/// 접힌 문제 막대 폭(px) — 접힘 모드에서 문제 칸 자리.
pub const FOLDED_RAIL_WIDTH: u32 = 48;
/// 처음 폭(px): 넓은 모드의 문제·AI, 접힘 모드의 AI.
pub const DEFAULT_PROBLEM_WIDTH: u32 = 380;
pub const DEFAULT_AI_WIDTH: u32 = 360;
pub const DEFAULT_FOLDED_AI_WIDTH: u32 = 320;
/// 손잡이에서 화살표 키 한 번에 움직이는 폭(px).
pub const KEY_STEP: u32 = 16;

/// 칸 폭 둘(문제·AI). 편집기는 나머지 전부다. 접힘 모드에서 `problem`은 막대 폭이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneWidths {
    pub problem: u32,
    pub ai: u32,
}

impl PaneWidths {
    /// 편집기 칸에 남는 폭.
    pub fn work(self, total: u32) -> u32 {
        total.saturating_sub(self.problem + self.ai)
    }
}

/// AI 칸이 아주 좁은 창에서 내려갈 수 있는 바닥(px). 창이 `MIN_WINDOW_WIDTH` 아래일 때만 쓴다 —
/// 이때는 편집기 최소 폭(도구 막대의 실행·예제 테스트·제출이 가려지지 않는 폭)을 AI 최소 폭보다 앞세운다.
pub const HARD_MIN_AI_WIDTH: u32 = 240;
/// 창 최소 크기(논리 px, `main.rs`): 접힘 막대 48 + 편집기 480 + AI 300을 넣고 조금 남는다.
pub const MIN_WINDOW_WIDTH: u32 = 1024;
pub const MIN_WINDOW_HEIGHT: u32 = 700;
/// 처음 창 크기(논리 px).
pub const DEFAULT_WINDOW_WIDTH: u32 = 1280;
pub const DEFAULT_WINDOW_HEIGHT: u32 = 800;

/// 왼쪽 칸 폭 `left`일 때 AI 칸 폭의 (아래, 위) 한계. 위 = 편집기 최소 폭을 남기는 폭,
/// 아래 = `MIN_AI_WIDTH`(위가 더 작으면 위로 내려가되 `HARD_MIN_AI_WIDTH` 밑으론 안 간다).
fn ai_bounds(total: u32, left: u32) -> (u32, u32) {
    let hi = total.saturating_sub(left + MIN_WORK_WIDTH);
    let lo = MIN_AI_WIDTH.min(hi).max(HARD_MIN_AI_WIDTH);
    (lo, hi.max(lo))
}

/// 창 폭 `total`에 맞춰 폭을 고친다: 칸마다 최소 폭을 지키고, 편집기 최소 폭을 남기도록 합이 넘치면
/// 최소 폭 위로 남는 몫의 비율대로 문제·AI를 줄인다. 접힘 모드의 문제 칸은 막대 폭 고정.
pub fn fit(total: u32, layout: PaneLayout, w: PaneWidths) -> PaneWidths {
    match layout {
        PaneLayout::Folded => {
            let (lo, hi) = ai_bounds(total, FOLDED_RAIL_WIDTH);
            PaneWidths {
                problem: FOLDED_RAIL_WIDTH,
                ai: w.ai.clamp(lo, hi),
            }
        }
        PaneLayout::Three => {
            let (mut p, mut a) = (w.problem.max(MIN_PROBLEM_WIDTH), w.ai.max(MIN_AI_WIDTH));
            let avail = total.saturating_sub(MIN_WORK_WIDTH);
            if p + a > avail {
                let (ep, ea) = (p - MIN_PROBLEM_WIDTH, a - MIN_AI_WIDTH);
                let need = p + a - avail;
                let sp = if ep + ea == 0 {
                    0
                } else {
                    (u64::from(need) * u64::from(ep) / u64::from(ep + ea)) as u32
                };
                let sp = sp.min(ep);
                let sa = (need - sp).min(ea);
                // 반올림으로 남은 한두 px는 문제 칸에서 더 덜어낸다.
                let sp = (need - sa).min(ep);
                p -= sp;
                a -= sa;
            }
            PaneWidths { problem: p, ai: a }
        }
    }
}

/// 문제|편집기 경계를 `problem_width`(창 왼쪽에서 경계까지)로 끌었다. 넓은 모드만 — 접힘 모드는 그대로.
pub fn drag_problem(
    total: u32,
    layout: PaneLayout,
    w: PaneWidths,
    problem_width: i64,
) -> PaneWidths {
    if layout != PaneLayout::Three {
        return fit(total, layout, w);
    }
    let w = fit(total, layout, w);
    let max = i64::from(total) - i64::from(MIN_WORK_WIDTH) - i64::from(w.ai);
    let max = max.max(i64::from(MIN_PROBLEM_WIDTH));
    PaneWidths {
        problem: problem_width.clamp(i64::from(MIN_PROBLEM_WIDTH), max) as u32,
        ai: w.ai,
    }
}

/// 편집기|AI 경계를 `ai_width`(경계에서 창 오른쪽까지)로 끌었다.
pub fn drag_ai(total: u32, layout: PaneLayout, w: PaneWidths, ai_width: i64) -> PaneWidths {
    let w = fit(total, layout, w);
    let (lo, hi) = ai_bounds(total, w.problem);
    PaneWidths {
        problem: w.problem,
        ai: ai_width.clamp(i64::from(lo), i64::from(hi)) as u32,
    }
}

/// 설정 파일 이름(앱 데이터 폴더 안).
pub const LAYOUT_FILE: &str = "ui-layout.json";
/// 설정 파일 형식 판. 모르는 판은 기본값으로 시작한다(옛 판 호환은 두지 않는다).
pub const LAYOUT_FORMAT: u32 = 1;

/// 기억하는 칸 폭. 넓은 모드(문제·AI)와 접힘 모드(AI만)를 따로 기억한다 — 창 폭이 다르면 알맞은 폭도 다르다.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LayoutPrefs {
    pub version: u32,
    pub three: PaneWidths,
    pub folded_ai: u32,
}

impl Default for LayoutPrefs {
    fn default() -> Self {
        Self {
            version: LAYOUT_FORMAT,
            three: PaneWidths {
                problem: DEFAULT_PROBLEM_WIDTH,
                ai: DEFAULT_AI_WIDTH,
            },
            folded_ai: DEFAULT_FOLDED_AI_WIDTH,
        }
    }
}

impl LayoutPrefs {
    /// 이 배치에서 쓰는 (맞추기 전) 폭.
    pub fn widths(&self, layout: PaneLayout) -> PaneWidths {
        match layout {
            PaneLayout::Three => self.three,
            PaneLayout::Folded => PaneWidths {
                problem: FOLDED_RAIL_WIDTH,
                ai: self.folded_ai,
            },
        }
    }

    /// 끌기 결과를 이 배치 몫으로 담는다.
    pub fn set(&mut self, layout: PaneLayout, w: PaneWidths) {
        match layout {
            PaneLayout::Three => self.three = w,
            PaneLayout::Folded => self.folded_ai = w.ai,
        }
    }

    /// 글에서 읽는다. 깨졌거나 모르는 판이면 기본값.
    pub fn parse(text: &str) -> Self {
        match serde_json::from_str::<Self>(text) {
            Ok(p) if p.version == LAYOUT_FORMAT => p,
            _ => Self::default(),
        }
    }

    /// 파일에서 읽는다. 없거나 못 읽으면 기본값.
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .map(|t| Self::parse(&t))
            .unwrap_or_default()
    }

    /// 파일에 쓴다(끌기가 끝났을 때만 부른다). 실패는 부른 쪽이 로그만 남기고 넘어간다.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }
}

/// 응시 전 점검 페이지가 1칸인가.
pub fn check_single_column(width: u32) -> bool {
    width < CHECK_TWO_COLUMN_MIN_WIDTH
}

/// 왼쪽 내비게이션이 아이콘만 남는가.
pub fn nav_collapsed(width: u32) -> bool {
    width <= NAV_COLLAPSE_MAX_WIDTH
}

/// 키 입력 하나(화면의 키 이벤트·Monaco 명령에서 옮겨 온다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput<'a> {
    /// `KeyboardEvent.key` 꼴: `"Enter"`, `"t"`, `"1"` …
    pub key: &'a str,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

/// 단축키가 일으키는 동작.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Ctrl+Enter — 예제 테스트.
    RunExamples,
    /// Ctrl+Shift+Enter — 입력 칸의 인자로 실행.
    RunInput,
}

/// 키 입력 → 동작. 아무것도 아닌 키는 `None`(편집기·웹뷰 기본 동작에 맡긴다). macOS는 Cmd를 Ctrl처럼 받는다.
pub fn shortcut(input: KeyInput<'_>) -> Option<Shortcut> {
    if input.alt || !(input.ctrl || input.meta) || input.key != "Enter" {
        return None;
    }
    Some(if input.shift {
        Shortcut::RunInput
    } else {
        Shortcut::RunExamples
    })
}

/// 단축키 안내(편집기 도구 막대 아래).
pub const SHORTCUT_HINT: &str = "Ctrl+Enter 예제 테스트 · Ctrl+Shift+Enter 실행";

/// 천 단위 쉼표(`1,240`).
pub fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 상태를 계속 받아 와야 하는가: 채점 대기 중이거나, 숨은 케이스 채점은 끝났지만 점수 계산(대화 평가, #99)이 아직이다.
pub fn should_poll(phase: &crate::session::Phase) -> bool {
    use crate::session::Phase;
    use promptlab_shared::evaluation::ScoreBasis;
    match phase {
        Phase::Grading { .. } => true,
        Phase::Graded { score, .. } => score
            .as_ref()
            .is_some_and(|s| s.basis == ScoreBasis::Pending),
        _ => false,
    }
}

/// 점수 글자: 정수면 소수점 없이, 아니면 소수 첫째 자리(`78`, `72.5`).
pub fn score_text(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < f64::EPSILON {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// AI 대화가 맨 아래를 따라가는 거리(px, #116): 맨 아래에서 이 안이면 「맨 아래에 있다」.
pub const FOLLOW_BOTTOM_PX: f64 = 40.0;

/// 스크롤 위치가 맨 아래 근처인가(`scroll_height - scroll_top - client_height <= FOLLOW_BOTTOM_PX`).
pub fn near_bottom(scroll_top: f64, scroll_height: f64, client_height: f64) -> bool {
    scroll_height - scroll_top - client_height <= FOLLOW_BOTTOM_PX
}

/// AI 대화 칸의 자동 스크롤(#116): 응시자가 맨 아래 근처에 있으면 새 메시지·스트리밍 조각마다 맨 아래로 내려가고,
/// 위로 올려 읽는 중이면 따라가지 않고 「새 메시지 ↓」를 띄운다. 응시자가 직접 보낸 메시지는 언제나 따라간다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollFollow {
    following: bool,
    unseen: bool,
}

impl Default for ScrollFollow {
    fn default() -> Self {
        Self {
            following: true,
            unseen: false,
        }
    }
}

impl ScrollFollow {
    /// 응시자가 스크롤했다(위치는 [`near_bottom`]으로).
    pub fn on_scroll(&mut self, near_bottom: bool) {
        self.following = near_bottom;
        if near_bottom {
            self.unseen = false;
        }
    }

    /// 내용이 늘었다. 맨 아래로 내려야 하면 `true`(아니면 「새 메시지」 표시를 켠다).
    pub fn on_content(&mut self) -> bool {
        if !self.following {
            self.unseen = true;
        }
        self.following
    }

    /// 응시자가 메시지를 보냈거나 「새 메시지 ↓」를 눌렀다 — 다시 따라간다(맨 아래로 내린다).
    pub fn jump(&mut self) {
        self.following = true;
        self.unseen = false;
    }

    pub fn following(&self) -> bool {
        self.following
    }

    /// 「새 메시지 ↓」 단추를 보일까.
    pub fn unseen(&self) -> bool {
        self.unseen
    }
}
