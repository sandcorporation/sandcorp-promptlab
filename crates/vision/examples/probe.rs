//! 폴더의 JPEG 프레임마다 판정·얼굴·자세를 찍는다(시험 프레임 고를 때 쓴다).
//! `cargo run --release -p promptlab-vision --example probe -- <폴더>`
use promptlab_vision::{Frame, FrameAnalyzer, YuNet};

fn main() {
    let dir = std::env::args().nth(1).expect("폴더");
    let mut yunet = YuNet::load().expect("모델");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for p in paths {
        let frame = Frame::from_jpeg(&std::fs::read(&p).unwrap()).unwrap();
        let t = std::time::Instant::now();
        let a = yunet.analyze(&frame).unwrap();
        let faces: Vec<String> = a
            .faces
            .iter()
            .map(|f| {
                let (y, p) = f.pose();
                format!("s{:.2} w{:.2} yaw{y:+.2} pitch{p:+.2}", f.score, f.bbox[2])
            })
            .collect();
        println!(
            "{} {:?} {}ms {}",
            p.file_name().unwrap().to_string_lossy(),
            a.verdict,
            t.elapsed().as_millis(),
            faces.join(" | ")
        );
    }
}
