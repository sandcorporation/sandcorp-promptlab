//! 응시 중 웹캠 감시(#13): 녹화 영상 프레임 → 분석기(트레잇 뒤) → 과정 로그 사건·올릴 스냅샷.
//! 프레임은 `crates/vision/tests/frames/`(출처·라이선스는 그 폴더의 SOURCES.md).

use promptlab_app::webcam::{CameraCheck, DEBOUNCE_FRAMES, Photo, Planned, Precheck, Watch};
use promptlab_shared::log::LogEvent;
use promptlab_shared::webcam::{PhotoRejection, SnapshotReason};
use promptlab_vision::{Analysis, Frame, FrameAnalyzer, Verdict, VisionError, YuNet};

fn frame(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../vision/tests/frames/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap()
}

const T0: i64 = 1_767_225_600_000;

fn kinds(planned: &[Planned]) -> Vec<(&'static str, Option<SnapshotReason>)> {
    planned
        .iter()
        .map(|p| (p.event.label(), p.snapshot.as_ref().map(|s| s.1)))
        .collect()
}

#[test]
fn recorded_session_becomes_events_and_snapshots() {
    // 3초마다 한 장: 정면 → 자리 비움 → 여러 명 → 옆을 봄 → 정면.
    let script = [
        "frontal-1.jpg",
        "frontal-2.jpg",
        "noface-1.jpg",
        "noface-2.jpg",
        "noface-1.jpg",
        "frontal-3.jpg",
        "multiple-1.jpg",
        "multiple-1.jpg",
        "away-1.jpg",
        "away-2.jpg",
        "frontal-1.jpg",
    ];
    let mut yunet = YuNet::load().unwrap();
    let mut watch = Watch::new("0-1767225600");
    let mut all = Vec::new();
    for (i, name) in script.iter().enumerate() {
        let at = T0 + i as i64 * 3_000;
        all.extend(watch.analyze(&mut yunet, at, &frame(name)).unwrap());
    }
    assert_eq!(
        kinds(&all),
        [
            ("주기 스냅샷", Some(SnapshotReason::Periodic)),
            ("얼굴 없음", Some(SnapshotReason::NoFace)),
            ("여러 명", Some(SnapshotReason::MultipleFaces)),
            ("시선 이탈", Some(SnapshotReason::GazeAway)),
        ]
    );
    assert!(matches!(
        all[2].event,
        LogEvent::MultipleFaces { count: 4, .. }
    ));
    // 사건의 스냅샷 참조 = 올릴 스냅샷의 참조(대시보드가 이어 붙인다). 참조는 겹치지 않는다.
    for p in &all {
        let (r, _) = p.snapshot.as_ref().unwrap();
        let in_event = match &p.event {
            LogEvent::NoFace { snapshot }
            | LogEvent::GazeAway { snapshot }
            | LogEvent::MultipleFaces { snapshot, .. } => snapshot.clone().unwrap(),
            LogEvent::Snapshot { snapshot } => snapshot.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(&in_event, r);
        assert!(r.starts_with("0-1767225600-"));
    }
    let mut refs: Vec<_> = all.iter().map(|p| p.snapshot.clone().unwrap().0).collect();
    refs.dedup();
    assert_eq!(refs.len(), all.len());
}

/// 정해 둔 판정을 차례로 내는 분석기.
struct Scripted(Vec<Verdict>);

impl FrameAnalyzer for Scripted {
    fn analyze(&mut self, _: &Frame) -> Result<Analysis, VisionError> {
        Ok(Analysis {
            faces: Vec::new(),
            verdict: self.0.remove(0),
        })
    }
}

#[test]
fn analyzer_sits_behind_the_trait() {
    let mut fake = Scripted(vec![Verdict::GazeAway; 2]);
    let mut watch = Watch::new("x");
    let jpeg = frame("frontal-1.jpg");
    let first = watch.analyze(&mut fake, T0, &jpeg).unwrap();
    assert_eq!(
        kinds(&first),
        [("주기 스냅샷", Some(SnapshotReason::Periodic))]
    );
    let second = watch.analyze(&mut fake, T0 + 3_000, &jpeg).unwrap();
    assert_eq!(
        kinds(&second),
        [("시선 이탈", Some(SnapshotReason::GazeAway))]
    );
    assert!(watch.analyze(&mut fake, T0, b"not a jpeg").is_err());
}

#[test]
fn one_odd_frame_is_not_an_event_and_alerts_are_spaced() {
    let mut w = Watch::new("x");
    let mut at = T0;
    let mut step = |w: &mut Watch, v: Verdict| {
        at += 3_000;
        w.frame(at, v)
    };
    step(&mut w, Verdict::Ok);
    // 한 장짜리 오검출은 사건이 아니다.
    assert!(step(&mut w, Verdict::NoFace).is_empty());
    assert!(step(&mut w, Verdict::Ok).is_empty());
    for _ in 0..DEBOUNCE_FRAMES - 1 {
        assert!(step(&mut w, Verdict::NoFace).is_empty());
    }
    assert_eq!(
        kinds(&step(&mut w, Verdict::NoFace)),
        [("얼굴 없음", Some(SnapshotReason::NoFace))]
    );
    // 계속 비어 있으면 다시 남기지 않는다.
    assert!(step(&mut w, Verdict::NoFace).is_empty());
    // 잠깐 돌아왔다 30초 안에 다시 비우면 사건 스냅샷을 또 찍지 않는다.
    step(&mut w, Verdict::Ok);
    step(&mut w, Verdict::Ok);
    step(&mut w, Verdict::NoFace);
    assert!(step(&mut w, Verdict::NoFace).is_empty());
}

#[test]
fn periodic_snapshot_every_ninety_seconds_and_camera_loss_once() {
    let mut w = Watch::new("x");
    let periodic = |p: &[Planned]| {
        p.iter()
            .filter(|p| matches!(p.event, LogEvent::Snapshot { .. }))
            .count()
    };
    let mut n = 0;
    for i in 0..=60 {
        n += periodic(&w.frame(T0 + i * 3_000, Verdict::Ok));
    }
    // 0초, 90초, 180초.
    assert_eq!(n, 3);
    let lost = w.unavailable("장치 분리");
    assert!(
        matches!(&lost[0].event, LogEvent::CameraUnavailable { reason } if reason == "장치 분리")
    );
    assert!(lost[0].snapshot.is_none());
    assert!(w.unavailable("장치 분리").is_empty());
    w.frame(T0 + 200_000, Verdict::Ok);
    assert_eq!(w.unavailable("권한 거부").len(), 1);
}

#[test]
fn only_single_low_res_jpegs_can_be_uploaded() {
    assert!(Photo::new(frame("frontal-1.jpg")).is_ok());
    let webm = [&[0x1A, 0x45, 0xDF, 0xA3][..], &[0u8; 1024][..]].concat();
    assert_eq!(Photo::new(webm), Err(PhotoRejection::NotJpeg));
    let big = [frame("frontal-1.jpg"), vec![0u8; 70 * 1024]].concat();
    assert_eq!(Photo::new(big), Err(PhotoRejection::TooLarge));
}

#[test]
fn precheck_needs_model_and_one_face() {
    let mut p = Precheck {
        cpu_threads: 2,
        model_ready: true,
        camera: CameraCheck::Seen(Verdict::Ok),
        ram_bytes: None,
    };
    assert!(p.blockers().is_empty());
    assert_eq!(p.warnings().len(), 1);
    // RAM 16GB 미만이면 경고만(막지 않는다), 모르면 경고하지 않는다.
    p.ram_bytes = Some(8 << 30);
    assert!(p.ram_low());
    assert_eq!(p.warnings().len(), 2);
    assert!(p.warnings()[1].contains("8GB"));
    assert!(p.blockers().is_empty());
    p.ram_bytes = Some(32 << 30);
    assert!(!p.ram_low());
    assert_eq!(p.warnings().len(), 1);
    p.camera = CameraCheck::Seen(Verdict::MultipleFaces { count: 2 });
    assert_eq!(p.blockers().len(), 1);
    p.camera = CameraCheck::Unavailable("권한 거부".into());
    p.model_ready = false;
    assert_eq!(p.blockers().len(), 2);
    p.camera = CameraCheck::NotTried;
    p.cpu_threads = 8;
    assert!(p.warnings().is_empty());
    assert!(!p.blockers().is_empty());
}
