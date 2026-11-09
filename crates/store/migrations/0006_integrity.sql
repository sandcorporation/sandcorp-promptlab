-- 잠금·무결성(#12). 새 테이블만 더한다(기존 행은 그대로). 규칙은 promptlab_shared::integrity.

-- 허용 목록에 없는 앱 판·모델로 문항 시작을 시도해 거부한 기록(의심 신호 「모델 무결성 실패」).
-- 시작이 거부돼 exam_item_starts 행이 없을 수 있으므로 초대에 붙인다. 같은 신고값은 한 줄에 횟수를 센다.
CREATE TABLE exam_integrity_rejections (
    invite_id    uuid        NOT NULL REFERENCES exam_invites (id) ON DELETE CASCADE,
    position     integer     NOT NULL CHECK (position >= 0),
    app_version  text        NOT NULL,
    model_sha256 text        NOT NULL,
    app_ok       boolean     NOT NULL,
    model_ok     boolean     NOT NULL,
    count        integer     NOT NULL CHECK (count > 0),
    first_at     timestamptz NOT NULL,
    last_at      timestamptz NOT NULL,
    PRIMARY KEY (invite_id, position, app_version, model_sha256)
);

-- 공개 예시 재실행 대조 큐(채점 워커가 채점 큐가 빌 때만 꺼낸다). 과정 로그의 example_run 사건 하나가 한 줄,
-- 같은 (예시, 프롬프트, 출력)은 fingerprint 로 한 번만 넣는다. 서버 재실행 출력은 DB에만 둔다(응답에 싣지 않는다).
CREATE TABLE exam_example_rechecks (
    invite_id    uuid        NOT NULL,
    position     integer     NOT NULL,
    seq          bigint      NOT NULL,
    fingerprint  text        NOT NULL,
    status       text        NOT NULL DEFAULT 'pending'
                             CHECK (status IN ('pending', 'running', 'done', 'failed')),
    outcome      text        CHECK (outcome IN ('match', 'minor', 'mismatch')),
    similarity   real,
    server_reply text,
    failure      text,
    enqueued_at  timestamptz NOT NULL,
    checked_at   timestamptz,
    PRIMARY KEY (invite_id, position, seq),
    UNIQUE (invite_id, position, fingerprint),
    FOREIGN KEY (invite_id, position, seq) REFERENCES exam_log_events (invite_id, position, seq) ON DELETE CASCADE,
    CHECK ((status = 'done') = (outcome IS NOT NULL))
);
CREATE INDEX exam_example_rechecks_pending_idx ON exam_example_rechecks (invite_id, position) WHERE status = 'pending';
