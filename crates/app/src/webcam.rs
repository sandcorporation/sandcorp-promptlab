//! 응시 중 웹캠 감시(#13): 분석기([`FrameAnalyzer`]) 판정을 과정 로그 사건과 올릴 스냅샷으로 바꾼다.
//!
//! - 화면(bin)은 3초마다 웹캠에서 저해상도 JPEG 한 장을 받아 [`Watch::frame`]에 넣고, 돌려받은 [`Planned`]대로
//!   사건을 기록하고 그 JPEG를 올린다. 영상은 어디에도 모으지 않는다 — 올리는 것은 [`Photo`] 한 장씩이다.
//! - 같은 판정이 [`DEBOUNCE_FRAMES`]번 이어져야 사건으로 친다(한 장짜리 오검출을 거른다). 같은 종류 사건은
//!   [`ALERT_SNAPSHOT_GAP_SECS`]에 한 번, 주기 스냅샷은 [`PERIODIC_SNAPSHOT_SECS`]마다.
//! - 분석은 한 장에 한 스레드 수십 ms(릴리스) — 3초 주기면 한 코어의 1~2%라 로컬 LLM 추론을 굶기지 않는다.

use promptlab_shared::inference::MIN_RAM_BYTES;
use promptlab_shared::log::LogEvent;
use promptlab_shared::webcam::{
    ALERT_SNAPSHOT_GAP_SECS, PERIODIC_SNAPSHOT_SECS, PhotoRejection, SnapshotReason, check_snapshot,
};
use promptlab_vision::{Frame, FrameAnalyzer, Verdict, VisionError};

/// 분석 주기(초).
pub const ANALYZE_EVERY_SECS: u64 = 3;
/// 사건으로 치기 전에 같은 판정이 이어져야 하는 장수.
pub const DEBOUNCE_FRAMES: u32 = 2;
/// 점검에서 권하는 최소 CPU 스레드(로컬 모델이 느려진다 — 막지는 않는다).
pub const RECOMMENDED_CPU_THREADS: usize = 4;

/// 서버에 올릴 수 있는 사진 한 장(JPEG, 저해상도). 만들 때 [`check_snapshot`]으로 확인한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Photo(Vec<u8>);

impl Photo {
    /// JPEG 한 장을 확인한다. 영상·큰 사진은 만들 수 없다.
    pub fn new(jpeg: Vec<u8>) -> Result<Self, PhotoRejection> {
        check_snapshot(&jpeg)?;
        Ok(Self(jpeg))
    }

    /// 바이트.
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// 사건 하나와, 그때 올릴 스냅샷.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// 과정 로그에 남길 사건.
    pub event: LogEvent,
    /// 올릴 스냅샷(참조, 까닭). 참조는 사건의 `snapshot`과 같다.
    pub snapshot: Option<(String, SnapshotReason)>,
}

/// 문항 하나를 푸는 동안의 감시.
#[derive(Debug)]
pub struct Watch {
    prefix: String,
    next: u64,
    last_periodic_ms: Option<i64>,
    streak: Option<(Option<SnapshotReason>, u32)>,
    current: Option<SnapshotReason>,
    last_alert_ms: Vec<(SnapshotReason, i64)>,
    camera_down: bool,
}

fn reason_of(v: Verdict) -> Option<SnapshotReason> {
    match v {
        Verdict::Ok => None,
        Verdict::NoFace => Some(SnapshotReason::NoFace),
        Verdict::MultipleFaces { .. } => Some(SnapshotReason::MultipleFaces),
        Verdict::GazeAway => Some(SnapshotReason::GazeAway),
    }
}

impl Watch {
    /// 감시를 시작한다. `prefix`는 스냅샷 참조 앞부분(배정 안에서 겹치지 않게 — 문항·시작 시각).
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            next: 0,
            last_periodic_ms: None,
            streak: None,
            current: None,
            last_alert_ms: Vec::new(),
            camera_down: false,
        }
    }

    fn reference(&mut self) -> String {
        self.next += 1;
        format!("{}-{}", self.prefix, self.next)
    }

    /// 분석한 프레임 하나의 판정.
    pub fn frame(&mut self, at_ms: i64, verdict: Verdict) -> Vec<Planned> {
        self.camera_down = false;
        let mut out = Vec::new();
        let reason = reason_of(verdict);
        let count = match self.streak {
            Some((r, n)) if r == reason => n + 1,
            _ => 1,
        };
        self.streak = Some((reason, count));
        if count >= DEBOUNCE_FRAMES && self.current != reason {
            self.current = reason;
            if let Some(r) = reason {
                let gap_ok = self
                    .last_alert_ms
                    .iter()
                    .find(|(k, _)| *k == r)
                    .is_none_or(|(_, last)| at_ms - last >= ALERT_SNAPSHOT_GAP_SECS as i64 * 1000);
                if gap_ok {
                    self.last_alert_ms.retain(|(k, _)| *k != r);
                    self.last_alert_ms.push((r, at_ms));
                    let snapshot = self.reference();
                    let event = match verdict {
                        Verdict::MultipleFaces { count } => LogEvent::MultipleFaces {
                            count,
                            snapshot: Some(snapshot.clone()),
                        },
                        Verdict::GazeAway => LogEvent::GazeAway {
                            snapshot: Some(snapshot.clone()),
                        },
                        _ => LogEvent::NoFace {
                            snapshot: Some(snapshot.clone()),
                        },
                    };
                    out.push(Planned {
                        event,
                        snapshot: Some((snapshot, r)),
                    });
                }
            }
        }
        let due = self
            .last_periodic_ms
            .is_none_or(|last| at_ms - last >= PERIODIC_SNAPSHOT_SECS as i64 * 1000);
        if due {
            self.last_periodic_ms = Some(at_ms);
            let snapshot = self.reference();
            out.push(Planned {
                event: LogEvent::Snapshot {
                    snapshot: snapshot.clone(),
                },
                snapshot: Some((snapshot, SnapshotReason::Periodic)),
            });
        }
        out
    }

    /// 웹캠에서 프레임을 받지 못했다(장치 분리·권한 거부). 끊길 때 한 번만 사건.
    pub fn unavailable(&mut self, reason: &str) -> Vec<Planned> {
        if self.camera_down {
            return Vec::new();
        }
        self.camera_down = true;
        self.streak = None;
        self.current = None;
        vec![Planned {
            event: LogEvent::CameraUnavailable {
                reason: reason.chars().take(200).collect(),
            },
            snapshot: None,
        }]
    }

    /// JPEG 한 장을 분석해 [`Watch::frame`]에 넣는다(풀지 못하면 웹캠 끊김으로 친다).
    pub fn analyze(
        &mut self,
        analyzer: &mut dyn FrameAnalyzer,
        at_ms: i64,
        jpeg: &[u8],
    ) -> Result<Vec<Planned>, VisionError> {
        let frame = Frame::from_jpeg(jpeg)?;
        let verdict = analyzer.analyze(&frame)?.verdict;
        Ok(self.frame(at_ms, verdict))
    }
}

/// 웹캠 점검 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraCheck {
    /// 아직 안 해 봤다.
    NotTried,
    /// 쓸 수 없다(까닭).
    Unavailable(String),
    /// 한 장 받아 분석했다.
    Seen(Verdict),
}

/// 응시 시작 전 점검(PC 사양·웹캠·모델).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Precheck {
    pub cpu_threads: usize,
    /// 로컬 모델 파일이 있고 고정 sha256과 맞다.
    pub model_ready: bool,
    pub camera: CameraCheck,
    /// 시스템 메모리(바이트, 모르면 `None` — 경고하지 않는다).
    pub ram_bytes: Option<u64>,
}

/// 바이트 → GB(반올림, 점검 문구용).
fn gib(bytes: u64) -> u64 {
    (bytes + (1 << 29)) >> 30
}

impl Precheck {
    /// 메모리가 최소 사양([`MIN_RAM_BYTES`])보다 작다.
    pub fn ram_low(&self) -> bool {
        self.ram_bytes.is_some_and(|b| b < MIN_RAM_BYTES)
    }

    /// 시작을 막는 문제들(비면 통과). 기준 사진은 얼굴 하나가 정면으로 보일 때만 찍는다.
    pub fn blockers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.model_ready {
            out.push(
                "AI 모델을 아직 받지 않았다 — 장치 점검의 「AI 모델」에서 받아 달라".to_owned(),
            );
        }
        match &self.camera {
            CameraCheck::NotTried => out.push("웹캠을 아직 확인하지 않았다".to_owned()),
            CameraCheck::Unavailable(why) => out.push(format!("웹캠을 쓸 수 없다: {why}")),
            CameraCheck::Seen(Verdict::Ok) => {}
            CameraCheck::Seen(Verdict::NoFace) => {
                out.push("얼굴이 보이지 않는다 — 카메라 앞에 앉아 달라".to_owned())
            }
            CameraCheck::Seen(Verdict::MultipleFaces { count }) => {
                out.push(format!("얼굴이 {count}개 보인다 — 혼자 응시해 달라"))
            }
            CameraCheck::Seen(Verdict::GazeAway) => out.push("화면을 정면으로 봐 달라".to_owned()),
        }
        out
    }

    /// 막지는 않지만 알릴 것.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.cpu_threads < RECOMMENDED_CPU_THREADS {
            out.push(format!(
                "CPU 스레드가 {}개다(권장 {RECOMMENDED_CPU_THREADS}개 이상) — 로컬 모델 답이 느릴 수 있다",
                self.cpu_threads
            ));
        }
        if self.ram_low() {
            out.push(format!(
                "메모리가 {}GB다(권장 {}GB 이상) — AI 도우미가 느리거나 올라가지 않을 수 있다",
                gib(self.ram_bytes.unwrap_or(0)),
                gib(MIN_RAM_BYTES)
            ));
        }
        out
    }
}
