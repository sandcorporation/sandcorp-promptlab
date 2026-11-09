-- 과정 로그(#9). 새 테이블만 더한다(기존 행은 그대로). 형식·검증 규칙은 promptlab_shared::log.

-- 응시 문항마다 등록된 앱 설치 공개키(Ed25519, base64url). 다시 설치·키 분실이면 새 키가 더해진다.
CREATE TABLE exam_log_keys (
    invite_id     uuid        NOT NULL,
    position      integer     NOT NULL,
    public_key    text        NOT NULL,
    registered_at timestamptz NOT NULL,
    -- 등록 순서(같은 초에 둘이 등록돼도 순서가 정해지게).
    ord           bigint      GENERATED ALWAYS AS IDENTITY,
    PRIMARY KEY (invite_id, position, public_key),
    FOREIGN KEY (invite_id, position) REFERENCES exam_item_starts (invite_id, position) ON DELETE CASCADE
);

-- 서명·체인을 검증해 받은 사건. body 는 앱이 서명한 원문 그대로(다시 직렬화하지 않는다), hash = sha256(body).
CREATE TABLE exam_log_events (
    invite_id   uuid        NOT NULL,
    position    integer     NOT NULL,
    seq         bigint      NOT NULL CHECK (seq >= 0),
    body        text        NOT NULL,
    sig         text        NOT NULL,
    hash        text        NOT NULL,
    received_at timestamptz NOT NULL,
    PRIMARY KEY (invite_id, position, seq),
    FOREIGN KEY (invite_id, position) REFERENCES exam_item_starts (invite_id, position) ON DELETE CASCADE
);

-- 검증에 실패해 거부한 업로드(다시보기 신호). 문항마다 한 줄에 횟수와 마지막 까닭만 둔다.
CREATE TABLE exam_log_rejections (
    invite_id   uuid        NOT NULL,
    position    integer     NOT NULL,
    count       integer     NOT NULL CHECK (count > 0),
    last_reason text        NOT NULL,
    last_at     timestamptz NOT NULL,
    PRIMARY KEY (invite_id, position),
    FOREIGN KEY (invite_id, position) REFERENCES exam_item_starts (invite_id, position) ON DELETE CASCADE
);
