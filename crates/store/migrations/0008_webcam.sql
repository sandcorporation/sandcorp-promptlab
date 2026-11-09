-- 웹캠 감독(#13). 새 테이블만 더한다. 규칙은 promptlab_shared::webcam.

-- 수집 항목 안내 동의. 배정(초대)마다 마지막 동의 하나(안내문 판이 바뀌면 다시 동의해 덮어쓴다).
CREATE TABLE exam_webcam_consents (
    invite_id      uuid        PRIMARY KEY REFERENCES exam_invites (id) ON DELETE CASCADE,
    notice_version text        NOT NULL,
    consented_at   timestamptz NOT NULL
);

-- 웹캠 사진: 기준 사진(position NULL, 배정마다 하나)과 스냅샷(문항마다). 저해상도 JPEG 한 장씩(64KB 이하).
-- 영상은 받지 않는다. 보유 기간이 지나면 서버가 지운다(taken_at 기준).
CREATE TABLE exam_webcam_photos (
    id          uuid        PRIMARY KEY,
    invite_id   uuid        NOT NULL REFERENCES exam_invites (id) ON DELETE CASCADE,
    position    integer,
    reason      text        NOT NULL CHECK (reason IN ('baseline', 'periodic', 'no_face', 'multiple_faces', 'gaze_away')),
    client_ref  text,
    app_at_ms   bigint,
    taken_at    timestamptz NOT NULL,
    jpeg        bytea       NOT NULL CHECK (octet_length(jpeg) <= 65536),
    CHECK ((reason = 'baseline') = (position IS NULL))
);
CREATE UNIQUE INDEX exam_webcam_photos_baseline ON exam_webcam_photos (invite_id) WHERE reason = 'baseline';
CREATE UNIQUE INDEX exam_webcam_photos_ref ON exam_webcam_photos (invite_id, client_ref) WHERE client_ref IS NOT NULL;
CREATE INDEX exam_webcam_photos_invite ON exam_webcam_photos (invite_id, taken_at);
CREATE INDEX exam_webcam_photos_taken ON exam_webcam_photos (taken_at);
