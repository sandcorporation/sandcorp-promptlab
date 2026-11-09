//! 웹캠 감독 규칙(#13): 수집 항목 안내·동의, 기준 얼굴 사진, 스냅샷 상한.
//!
//! - 응시 앱이 웹캠 프레임을 **기기 안에서** 분석하고(`promptlab-vision`), 서버에는 저해상도 JPEG 사진만 올린다:
//!   기준 사진 한 장, 감지 사건 순간 한 장, 문항을 푸는 동안 [`PERIODIC_SNAPSHOT_SECS`]마다 한 장.
//!   **영상(전체 녹화)은 올리지 않는다** — 서버는 `image/jpeg` 한 장씩만 받고 [`check_snapshot`]으로 크기·해상도를 막는다.
//! - 동의: 사람 응시자는 [`NOTICE_VERSION`]의 안내에 동의하고 기준 사진을 올려야 문항을 시작할 수 있다(서버 강제).
//!   스모크 기계 주체(#28)는 사람이 아니라 면제한다(VM에 카메라가 없고 개인정보가 없다).
//! - 보유: 서버가 촬영 뒤 보유 기간(기본 [`DEFAULT_RETENTION_DAYS`]일, `PROMPTLAB_WEBCAM_RETENTION_DAYS`)이 지난 사진을 지운다.
//! - 판단은 고객사 — 감지 사건은 의심 신호로만 보인다.

use serde::{Deserialize, Serialize};

/// 안내문 판. 문구를 바꾸면 올리고, 옛 판 동의로는 시작하지 못한다(다시 동의).
pub const NOTICE_VERSION: &str = "2026-11-06-2";

/// 사진 보유 기간 기본값(일, 대표 결정 2026-11-06). 서버 설정으로 바꾼다.
pub const DEFAULT_RETENTION_DAYS: u32 = 30;

/// 주기 스냅샷 간격(초).
pub const PERIODIC_SNAPSHOT_SECS: u64 = 90;

/// 같은 종류 감지 사건의 스냅샷 최소 간격(초).
pub const ALERT_SNAPSHOT_GAP_SECS: u64 = 30;

/// 사진 한 장의 최대 크기(바이트).
pub const MAX_PHOTO_BYTES: usize = 64 * 1024;

/// 사진 한 장의 최대 가로·세로(픽셀). 저해상도만 받는다.
pub const MAX_PHOTO_SIDE: u16 = 320;

/// 문항 하나에 받는 스냅샷 상한(90초 주기로 1시간 40장 + 감지 사건).
pub const MAX_SNAPSHOTS_PER_ITEM: i64 = 200;

/// 개인정보 처리방침(privacy-and-terms 저장소, 주소 고정).
pub const PRIVACY_POLICY_URL: &str =
    "https://sandcorporation.github.io/privacy-and-terms/promptlab/privacy/";

/// 동의 화면에 보이는 안내문.
pub fn notice(retention_days: u32) -> String {
    format!(
        "이 시험은 부정행위 확인을 위해 웹캠을 씁니다.\n\
         \n\
         · 수집 항목: 시작할 때 얼굴 사진 1장(기준 사진), 문항을 푸는 동안 {periodic}초마다 1장과 \
         이상(얼굴 없음·여러 명·시선 이탈)을 감지한 순간 1장의 저해상도 사진, 감지 사건의 종류와 시각.\n\
         · 영상과 음성은 녹화하지도 저장하지도 않습니다. 얼굴 분석은 이 PC 안에서만 합니다.\n\
         · 실시간 열람: 시험을 연 회사의 관리자·감독관이 응시 중에 웹캠 영상(음성 없음)을 실시간으로 볼 수 있습니다. \
         감독관이 볼 때만 전송되고 그동안 화면에 「감독관이 실시간으로 보는 중」이 표시되며, 녹화하거나 저장하지 않습니다.\n\
         · 사진은 시험을 연 회사의 관리자·감독관만 보며, 촬영 후 {retention_days}일 뒤 자동으로 삭제됩니다.\n\
         · 감지 결과는 참고 신호일 뿐이며, 판단은 시험을 연 회사가 합니다.\n\
         · 사진 열람·삭제 요청은 시험을 연 회사에 하십시오.\n\
         · 동의하지 않으면 이 시험을 볼 수 없습니다. 다른 응시 방법은 시험을 연 회사에 문의하세요.\n\
         · 개인정보 처리방침: {policy}",
        periodic = PERIODIC_SNAPSHOT_SECS,
        policy = PRIVACY_POLICY_URL,
    )
}

/// 스냅샷을 찍은 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    /// 주기 스냅샷.
    Periodic,
    /// 얼굴이 없다.
    NoFace,
    /// 얼굴이 둘 이상.
    MultipleFaces,
    /// 화면을 보지 않는다.
    GazeAway,
}

impl SnapshotReason {
    /// DB·쿼리 문자열.
    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotReason::Periodic => "periodic",
            SnapshotReason::NoFace => "no_face",
            SnapshotReason::MultipleFaces => "multiple_faces",
            SnapshotReason::GazeAway => "gaze_away",
        }
    }

    /// 문자열에서.
    pub fn parse(s: &str) -> Option<Self> {
        [
            SnapshotReason::Periodic,
            SnapshotReason::NoFace,
            SnapshotReason::MultipleFaces,
            SnapshotReason::GazeAway,
        ]
        .into_iter()
        .find(|r| r.as_str() == s)
    }

    /// 화면 이름.
    pub fn label(self) -> &'static str {
        match self {
            SnapshotReason::Periodic => "주기",
            SnapshotReason::NoFace => "얼굴 없음",
            SnapshotReason::MultipleFaces => "여러 명",
            SnapshotReason::GazeAway => "시선 이탈",
        }
    }
}

/// 사진을 받지 않는 까닭.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoRejection {
    /// [`MAX_PHOTO_BYTES`]보다 크다.
    TooLarge,
    /// JPEG가 아니다(영상 등).
    NotJpeg,
    /// 가로·세로가 [`MAX_PHOTO_SIDE`]를 넘는다.
    TooManyPixels,
}

impl PhotoRejection {
    /// 응답 문구.
    pub fn message(self) -> &'static str {
        match self {
            PhotoRejection::TooLarge => "사진이 너무 크다(64KB까지)",
            PhotoRejection::NotJpeg => "JPEG 사진 한 장만 받는다",
            PhotoRejection::TooManyPixels => "저해상도(320×320 이하) 사진만 받는다",
        }
    }
}

/// 받을 수 있는 사진(JPEG 한 장, 저해상도)인가. 해상도는 JPEG 프레임 머리(SOF)에서 읽는다.
pub fn check_snapshot(bytes: &[u8]) -> Result<(u16, u16), PhotoRejection> {
    if bytes.len() > MAX_PHOTO_BYTES {
        return Err(PhotoRejection::TooLarge);
    }
    let (w, h) = jpeg_dimensions(bytes).ok_or(PhotoRejection::NotJpeg)?;
    if w == 0 || h == 0 || w > MAX_PHOTO_SIDE || h > MAX_PHOTO_SIDE {
        return Err(PhotoRejection::TooManyPixels);
    }
    Ok((w, h))
}

/// JPEG의 가로·세로. JPEG가 아니면 `None`.
pub fn jpeg_dimensions(bytes: &[u8]) -> Option<(u16, u16)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    loop {
        // 표지 앞의 채움 0xFF 를 건너뛴다.
        while *bytes.get(i)? == 0xFF && *bytes.get(i + 1)? == 0xFF {
            i += 1;
        }
        if *bytes.get(i)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(i + 1)?;
        let len = u16::from_be_bytes([*bytes.get(i + 2)?, *bytes.get(i + 3)?]) as usize;
        // SOF0~SOF15(DHT C4·JPG C8·DAC CC 제외).
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let h = u16::from_be_bytes([*bytes.get(i + 5)?, *bytes.get(i + 6)?]);
            let w = u16::from_be_bytes([*bytes.get(i + 7)?, *bytes.get(i + 8)?]);
            return Some((w, h));
        }
        if marker == 0xDA || marker == 0xD9 || len < 2 {
            return None;
        }
        i += 2 + len;
    }
}

/// 배정의 웹캠 조건(앱이 동의 화면에 쓴다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebcamTerms {
    /// 지금 안내문 판.
    pub notice_version: String,
    /// 사진 보유 기간(일).
    pub retention_days: u32,
    /// 지금 판에 동의한 시각(유닉스 초). 없으면 아직.
    pub consented_at: Option<i64>,
    /// 기준 사진을 올렸는가.
    pub baseline_uploaded: bool,
    /// 이 응시에 웹캠이 필요한가(스모크 기계 주체는 아니다).
    pub required: bool,
}

/// `POST /assignments/{id}/consent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRequest {
    /// 동의한 안내문 판([`NOTICE_VERSION`]이어야 한다).
    pub notice_version: String,
}

/// 감독관이 보는 사진 하나의 정보(사진 바이트는 따로 받는다).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotMeta {
    /// 사진 id. 사진은 `…/webcam/photos/{id}`.
    pub id: String,
    /// 문항 번호(0부터).
    pub position: u32,
    pub reason: SnapshotReason,
    /// 앱이 붙인 참조. 과정 로그의 웹캠 사건(`snapshot`)이 이 값을 가리킨다.
    pub client_ref: Option<String>,
    /// 서버가 받은 시각(유닉스 밀리초).
    pub at_ms: i64,
    /// 앱 시계로 찍은 시각(유닉스 밀리초, 신고값).
    pub app_at_ms: Option<i64>,
}

/// 기준 사진 정보.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaselineMeta {
    /// 사진 id.
    pub id: String,
    pub at_ms: i64,
}

/// `GET /orgs/{id}/exams/{exam}/invites/{invite}/webcam` — 감독관이 보는 응시 하나의 웹캠 사진.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebcamView {
    pub candidate: String,
    /// 동의한 안내문 판·시각(유닉스 초).
    pub consent: Option<(String, i64)>,
    pub baseline: Option<BaselineMeta>,
    /// 시간순.
    pub snapshots: Vec<SnapshotMeta>,
    /// 보유 기간(일) — 이보다 오래된 사진은 지워져 없다.
    pub retention_days: u32,
}

/// 스냅샷 업로드 응답.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotStored {
    pub id: String,
}
