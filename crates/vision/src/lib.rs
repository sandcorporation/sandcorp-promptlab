//! 웹캠 프레임 **로컬 분석**(#13): 얼굴 없음·여러 명·시선 이탈.
//!
//! - [`FrameAnalyzer`] 트레잇 뒤에 분석기를 둔다. 응시 앱은 트레잇만 보고, 시험은 녹화 영상 프레임
//!   (`tests/frames/`, 출처·라이선스는 그 폴더의 `SOURCES.md`)을 실제 분석기에 넣는다(spec #1 Testing Decisions 3).
//! - 구현은 [`YuNet`]: OpenCV Zoo의 YuNet 얼굴 검출 ONNX(MIT, 약 227KB) — 얼굴 상자와 눈·코·입 5점을 낸다.
//!   모델은 **앱에 구워 넣는다**(`include_bytes!`, [`FACE_MODEL`]에 출처·커밋·sha256 고정, 로드할 때 검증).
//!   작아서 내려받기 단계·캐시·네트워크 실패를 두지 않는 쪽이 낫다. 바꿔 치기는 앱 판 허용 목록이 막는다.
//! - ONNX 런타임은 tract(순수 Rust): onnxruntime(`ort`)처럼 네이티브 DLL을 설치 파일에 더하지 않고,
//!   Windows x64 빌드에 C/C++ 도구가 더 필요 없다. 이 크기의 모델은 한 코어로 320×256 한 장에 수십 ms.
//! - 판정 규칙([`judge`])은 모델과 떼어 둔 순수 함수다. 판단은 고객사 — 여기서는 신호만 만든다.

use std::io::Cursor;

use sha2::{Digest, Sha256};
use tract_onnx::prelude::*;

/// 고정한 얼굴 검출 모델.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaceModelPin {
    /// 원본 저장소.
    pub repo: &'static str,
    /// 저장소 안 경로.
    pub path: &'static str,
    /// 저장소 커밋(파일을 마지막으로 바꾼 커밋).
    pub revision: &'static str,
    /// 파일 sha256(소문자 16진).
    pub sha256: &'static str,
    /// 파일 크기(바이트).
    pub size: u64,
    /// 라이선스(전문은 `crates/vision/models/LICENSE-yunet`).
    pub license: &'static str,
}

/// 응시 앱이 쓰는 얼굴 검출 모델. YuNet 2023mar(OpenCV Zoo).
pub const FACE_MODEL: FaceModelPin = FaceModelPin {
    repo: "https://github.com/opencv/opencv_zoo",
    path: "models/face_detection_yunet/face_detection_yunet_2023mar.onnx",
    revision: "f12e12798e8314f7c074a6656816c048dcc95b7a",
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    size: 232_589,
    license: "MIT (Copyright (c) 2020 Shiqi Yu)",
};

/// 앱에 구워 넣은 모델 바이트.
pub static FACE_MODEL_BYTES: &[u8] = include_bytes!("../models/face_detection_yunet_2023mar.onnx");

/// 모델 입력 크기. 프레임은 비율을 지켜 이 안에 줄이고 남는 곳은 0으로 채운다(32의 배수여야 한다).
pub const INPUT_WIDTH: u32 = 320;
/// 모델 입력 높이.
pub const INPUT_HEIGHT: u32 = 256;

/// 얼굴로 칠 최소 점수.
pub const MIN_SCORE: f32 = 0.6;
/// 얼굴로 칠 최소 크기(얼굴 상자 너비 / 프레임 너비). 멀리 지나가는 작은 얼굴·잡음을 거른다.
pub const MIN_FACE_WIDTH: f32 = 0.05;
/// 시선 이탈 문턱(좌우): |코 x − 두 눈 가운데 x| / 두 눈 사이 거리.
pub const YAW_LIMIT: f32 = 0.4;
/// 시선 이탈 문턱(아래·위): (코 y − 두 눈 가운데 y) / 두 눈 사이 거리 가 이 범위 밖.
pub const PITCH_RANGE: (f32, f32) = (0.5, 1.35);

/// 분석기 오류.
#[derive(Debug, thiserror::Error)]
pub enum VisionError {
    /// 프레임을 풀지 못했다.
    #[error("프레임을 풀지 못했다: {0}")]
    Decode(String),
    /// 모델이 고정값과 다르거나 올리지 못했다.
    #[error("얼굴 검출 모델 오류: {0}")]
    Model(String),
}

/// RGB8 프레임 한 장.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    width: u32,
    height: u32,
    rgb: Vec<u8>,
}

impl Frame {
    /// RGB8 바이트(행 우선)로 만든다. 길이가 맞지 않으면 오류.
    pub fn from_rgb(width: u32, height: u32, rgb: Vec<u8>) -> Result<Self, VisionError> {
        if width == 0 || height == 0 || rgb.len() != (width * height * 3) as usize {
            return Err(VisionError::Decode("RGB 길이가 크기와 맞지 않는다".into()));
        }
        Ok(Self { width, height, rgb })
    }

    /// JPEG 한 장을 푼다(웹캠 캡처는 JPEG로 온다).
    pub fn from_jpeg(bytes: &[u8]) -> Result<Self, VisionError> {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
            .map_err(|e| VisionError::Decode(e.to_string()))?
            .to_rgb8();
        let (width, height) = img.dimensions();
        Ok(Self {
            width,
            height,
            rgb: img.into_raw(),
        })
    }

    /// 너비(픽셀).
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 높이(픽셀).
    pub fn height(&self) -> u32 {
        self.height
    }
}

/// 검출한 얼굴 하나. 좌표는 프레임 크기로 나눈 0~1 값.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    /// 점수(0~1).
    pub score: f32,
    /// 상자 왼쪽 위 x·y, 너비·높이.
    pub bbox: [f32; 4],
    /// 오른눈·왼눈·코·입 오른쪽·입 왼쪽(사진 속 사람 기준).
    pub landmarks: [[f32; 2]; 5],
}

impl Face {
    /// 정면에서 벗어난 정도: (좌우, 아래위). [`YAW_LIMIT`]·[`PITCH_RANGE`]와 견준다.
    pub fn pose(&self) -> (f32, f32) {
        let [re, le, nose, _, _] = self.landmarks;
        let mid = [(re[0] + le[0]) / 2.0, (re[1] + le[1]) / 2.0];
        let eyes = ((re[0] - le[0]).powi(2) + (re[1] - le[1]).powi(2))
            .sqrt()
            .max(1e-6);
        ((nose[0] - mid[0]) / eyes, (nose[1] - mid[1]) / eyes)
    }

    /// 시선 이탈로 볼 자세인가.
    pub fn looking_away(&self) -> bool {
        let (yaw, pitch) = self.pose();
        yaw.abs() > YAW_LIMIT || pitch < PITCH_RANGE.0 || pitch > PITCH_RANGE.1
    }
}

/// 프레임 한 장의 판정.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 얼굴 하나, 정면.
    Ok,
    /// 얼굴이 없다.
    NoFace,
    /// 얼굴이 둘 이상.
    MultipleFaces {
        /// 얼굴 수.
        count: u32,
    },
    /// 얼굴 하나인데 화면을 보지 않는다.
    GazeAway,
}

/// 분석 결과.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    /// 문턱을 넘은 얼굴들(점수 높은 순).
    pub faces: Vec<Face>,
    /// 판정.
    pub verdict: Verdict,
}

/// 판정 규칙: 점수·크기 문턱을 넘은 얼굴 수와 자세로 정한다.
pub fn judge(faces: &[Face]) -> Verdict {
    let counted: Vec<&Face> = faces.iter().filter(|f| counts(f)).collect();
    match counted.as_slice() {
        [] => Verdict::NoFace,
        [one] if one.looking_away() => Verdict::GazeAway,
        [_] => Verdict::Ok,
        many => Verdict::MultipleFaces {
            count: many.len() as u32,
        },
    }
}

fn counts(f: &Face) -> bool {
    f.score >= MIN_SCORE && f.bbox[2] >= MIN_FACE_WIDTH
}

/// 프레임 분석기. 응시 앱은 이 트레잇만 본다.
pub trait FrameAnalyzer: Send {
    /// 프레임 한 장을 분석한다.
    fn analyze(&mut self, frame: &Frame) -> Result<Analysis, VisionError>;
}

type Plan = std::sync::Arc<TypedRunnableModel>;

/// YuNet 얼굴 검출(ONNX, tract). 한 스레드에서 돈다.
pub struct YuNet {
    plan: Plan,
    /// 출력 위치: stride 8·16·32 마다 (cls, obj, bbox, kps).
    outputs: [[usize; 4]; 3],
}

impl std::fmt::Debug for YuNet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YuNet").finish_non_exhaustive()
    }
}

const STRIDES: [u32; 3] = [8, 16, 32];

/// 바이트가 고정 모델인가(sha256·크기).
pub fn verify_model(bytes: &[u8]) -> Result<(), VisionError> {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if bytes.len() as u64 != FACE_MODEL.size || hex != FACE_MODEL.sha256 {
        return Err(VisionError::Model(format!("sha256 불일치: {hex}")));
    }
    Ok(())
}

impl YuNet {
    /// 구워 넣은 모델을 검증하고 올린다(수백 ms — 응시 시작 전 점검에서 한 번).
    pub fn load() -> Result<Self, VisionError> {
        verify_model(FACE_MODEL_BYTES)?;
        let model_err = |e: TractError| VisionError::Model(format!("{e:?}"));
        let mut model = tract_onnx::onnx()
            // 내보낸 모델에 640×640 중간 모양이 박혀 있다 — 무시하고 입력 크기에서 다시 추론한다.
            .with_ignore_value_info(true)
            .with_ignore_output_shapes(true)
            .model_for_read(&mut Cursor::new(FACE_MODEL_BYTES))
            .map_err(model_err)?
            .with_input_fact(
                0,
                f32::fact([1, 3, INPUT_HEIGHT as usize, INPUT_WIDTH as usize]).into(),
            )
            .map_err(model_err)?;
        let names: Vec<String> = model
            .output_outlets()
            .map_err(model_err)?
            .iter()
            .map(|o| {
                model
                    .outlet_label(*o)
                    .map(String::from)
                    .unwrap_or_else(|| model.node(o.node).name.clone())
            })
            .collect();
        let find = |prefix: &str, stride: u32| {
            names
                .iter()
                .position(|n| *n == format!("{prefix}_{stride}"))
                .ok_or_else(|| {
                    VisionError::Model(format!("출력 {prefix}_{stride} 없음: {names:?}"))
                })
        };
        let mut outputs = [[0usize; 4]; 3];
        for (i, s) in STRIDES.iter().enumerate() {
            outputs[i] = [
                find("cls", *s)?,
                find("obj", *s)?,
                find("bbox", *s)?,
                find("kps", *s)?,
            ];
        }
        model.auto_outputs().map_err(model_err)?;
        let plan = model
            .into_optimized()
            .map_err(model_err)?
            .into_runnable()
            .map_err(model_err)?;
        Ok(Self { plan, outputs })
    }

    fn input(frame: &Frame) -> (Tensor, f32) {
        let scale = (INPUT_WIDTH as f32 / frame.width as f32)
            .min(INPUT_HEIGHT as f32 / frame.height as f32);
        let w = ((frame.width as f32 * scale).round() as u32).clamp(1, INPUT_WIDTH);
        let h = ((frame.height as f32 * scale).round() as u32).clamp(1, INPUT_HEIGHT);
        let src = image::RgbImage::from_raw(frame.width, frame.height, frame.rgb.clone())
            .expect("Frame 길이는 만들 때 확인했다");
        let small = image::imageops::resize(&src, w, h, image::imageops::FilterType::Triangle);
        let (iw, ih) = (INPUT_WIDTH as usize, INPUT_HEIGHT as usize);
        let tensor = tract_ndarray::Array4::from_shape_fn((1, 3, ih, iw), |(_, c, y, x)| {
            if x as u32 >= w || y as u32 >= h {
                return 0.0;
            }
            // 모델은 OpenCV 관례(BGR, 0~255)로 학습했다.
            small.get_pixel(x as u32, y as u32)[2 - c] as f32
        })
        .into_tensor();
        (tensor, scale)
    }
}

impl FrameAnalyzer for YuNet {
    fn analyze(&mut self, frame: &Frame) -> Result<Analysis, VisionError> {
        let (input, scale) = Self::input(frame);
        let out = self
            .plan
            .run(tvec!(input.into()))
            .map_err(|e| VisionError::Model(e.to_string()))?;
        let view = |i: usize| -> Result<Vec<f32>, VisionError> {
            Ok(out[i]
                .to_plain_array_view::<f32>()
                .map_err(|e| VisionError::Model(e.to_string()))?
                .iter()
                .copied()
                .collect())
        };
        let (fw, fh) = (frame.width as f32, frame.height as f32);
        let mut faces = Vec::new();
        for (si, stride) in STRIDES.iter().enumerate() {
            let [ci, oi, bi, ki] = self.outputs[si];
            let (cls, obj, bbox, kps) = (view(ci)?, view(oi)?, view(bi)?, view(ki)?);
            let cols = (INPUT_WIDTH / stride) as usize;
            let s = *stride as f32;
            for idx in 0..cls.len() {
                let score = (cls[idx].clamp(0.0, 1.0) * obj[idx].clamp(0.0, 1.0)).sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let (c, r) = ((idx % cols) as f32, (idx / cols) as f32);
                let b = &bbox[idx * 4..idx * 4 + 4];
                let (cx, cy) = ((c + b[0]) * s, (r + b[1]) * s);
                let (w, h) = (b[2].exp() * s, b[3].exp() * s);
                // 입력 좌표 → 원래 프레임 비율(0~1).
                let to = |x: f32, y: f32| [x / scale / fw, y / scale / fh];
                let tl = to(cx - w / 2.0, cy - h / 2.0);
                let k = &kps[idx * 10..idx * 10 + 10];
                let mut landmarks = [[0.0; 2]; 5];
                for (n, lm) in landmarks.iter_mut().enumerate() {
                    *lm = to((c + k[2 * n]) * s, (r + k[2 * n + 1]) * s);
                }
                faces.push(Face {
                    score,
                    bbox: [tl[0], tl[1], w / scale / fw, h / scale / fh],
                    landmarks,
                });
            }
        }
        let faces = nms(faces, 0.3);
        let verdict = judge(&faces);
        Ok(Analysis { faces, verdict })
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn nms(mut faces: Vec<Face>, limit: f32) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Face> = Vec::new();
    for f in faces {
        if kept.iter().all(|k| iou(&k.bbox, &f.bbox) < limit) {
            kept.push(f);
        }
    }
    kept
}
