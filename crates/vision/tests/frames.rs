//! 녹화 영상 프레임으로 실제 분석기(YuNet)의 판정을 본다. 프레임 출처·라이선스는 `frames/SOURCES.md`.

use promptlab_vision::{
    FACE_MODEL, FACE_MODEL_BYTES, Face, Frame, FrameAnalyzer, Verdict, YuNet, judge, verify_model,
};

fn frame(name: &str) -> Frame {
    let path = format!("{}/tests/frames/{name}", env!("CARGO_MANIFEST_DIR"));
    Frame::from_jpeg(&std::fs::read(path).unwrap()).unwrap()
}

fn verdict(yunet: &mut YuNet, name: &str) -> Verdict {
    yunet.analyze(&frame(name)).unwrap().verdict
}

#[test]
fn recorded_frames_are_judged() {
    let mut yunet = YuNet::load().unwrap();
    for name in ["frontal-1.jpg", "frontal-2.jpg", "frontal-3.jpg"] {
        assert_eq!(verdict(&mut yunet, name), Verdict::Ok, "{name}");
    }
    for name in ["noface-1.jpg", "noface-2.jpg"] {
        assert_eq!(verdict(&mut yunet, name), Verdict::NoFace, "{name}");
    }
    assert_eq!(
        verdict(&mut yunet, "multiple-1.jpg"),
        Verdict::MultipleFaces { count: 4 }
    );
    for name in ["away-1.jpg", "away-2.jpg"] {
        assert_eq!(verdict(&mut yunet, name), Verdict::GazeAway, "{name}");
    }
}

#[test]
fn bundled_model_matches_pin() {
    verify_model(FACE_MODEL_BYTES).unwrap();
    assert_eq!(FACE_MODEL_BYTES.len() as u64, FACE_MODEL.size);
    let mut changed = FACE_MODEL_BYTES.to_vec();
    changed[1000] ^= 1;
    assert!(verify_model(&changed).is_err());
}

#[test]
fn analysis_is_fast_enough_to_share_the_cpu() {
    // 응시 중 3초에 한 장 분석한다. 디버그 빌드에서도 한 장이 1초 안이면 한 코어의 일부만 쓴다.
    let mut yunet = YuNet::load().unwrap();
    let f = frame("frontal-1.jpg");
    let t = std::time::Instant::now();
    yunet.analyze(&f).unwrap();
    assert!(t.elapsed().as_millis() < 1_000, "{:?}", t.elapsed());
}

#[test]
fn not_a_jpeg_is_an_error() {
    assert!(Frame::from_jpeg(b"not a jpeg").is_err());
    assert!(Frame::from_rgb(2, 2, vec![0; 5]).is_err());
}

fn face(score: f32, width: f32, nose: [f32; 2]) -> Face {
    Face {
        score,
        bbox: [0.4, 0.3, width, width * 1.3],
        landmarks: [[0.45, 0.4], [0.55, 0.4], nose, [0.46, 0.5], [0.54, 0.5]],
    }
}

#[test]
fn rules_ignore_weak_or_tiny_faces() {
    let front = face(0.9, 0.15, [0.5, 0.5]);
    assert_eq!(judge(&[front]), Verdict::Ok);
    assert_eq!(judge(&[front, face(0.5, 0.15, [0.5, 0.5])]), Verdict::Ok);
    assert_eq!(judge(&[front, face(0.9, 0.02, [0.5, 0.5])]), Verdict::Ok);
    assert_eq!(
        judge(&[front, face(0.9, 0.1, [0.5, 0.5])]),
        Verdict::MultipleFaces { count: 2 }
    );
    assert_eq!(judge(&[face(0.9, 0.15, [0.6, 0.5])]), Verdict::GazeAway);
    assert_eq!(judge(&[face(0.9, 0.15, [0.5, 0.56])]), Verdict::GazeAway);
    assert_eq!(judge(&[]), Verdict::NoFace);
}
