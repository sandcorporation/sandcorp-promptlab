-- 시험·응시 배정 초대(#8). 새 테이블과 submissions 열 추가만 한다(기존 행은 그대로).

-- 시험 = 조직의 문항 묶음. 용도가 채용이면 응시자에게 점수를 공개하지 않는다(reveal_scores 는 사내 진단만).
CREATE TABLE exams (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    title         text        NOT NULL CHECK (length(btrim(title)) > 0),
    purpose       text        NOT NULL CHECK (purpose IN ('recruiting', 'internal')),
    reveal_scores boolean     NOT NULL DEFAULT false,
    created_by    text        NOT NULL REFERENCES users (sub),
    created_at    timestamptz NOT NULL,
    CHECK (purpose = 'internal' OR NOT reveal_scores)
);
CREATE INDEX exams_org_idx ON exams (org_id, created_at);

-- 문항: 문제은행의 문제 하나와 이 문항의 제한.
CREATE TABLE exam_items (
    exam_id         uuid    NOT NULL REFERENCES exams (id) ON DELETE CASCADE,
    position        integer NOT NULL CHECK (position >= 0),
    problem_id      text    NOT NULL,
    time_limit_secs integer NOT NULL CHECK (time_limit_secs > 0),
    max_turns       integer NOT NULL CHECK (max_turns > 0),
    PRIMARY KEY (exam_id, position)
);

-- 응시 배정 초대. 조직 구성원 초대(org_invites)와 다르다: 받은 사람은 조직에 들어가지 않고 이 시험을 배정받는다.
-- 토큰 원문은 만들 때 한 번만 응답(과 메일)에 싣고 여기에는 sha256만 둔다. email 은 소문자로 다듬어 넣는다.
CREATE TABLE exam_invites (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    exam_id       uuid        NOT NULL REFERENCES exams (id) ON DELETE CASCADE,
    email         text        NOT NULL,
    token_hash    bytea       NOT NULL UNIQUE,
    created_by    text        NOT NULL REFERENCES users (sub),
    created_at    timestamptz NOT NULL,
    expires_at    timestamptz NOT NULL,
    -- 링크로 들어와 배정을 받은 사람. 받은 뒤에는 다른 사람이 같은 링크로 받지 못한다.
    candidate_sub text        REFERENCES users (sub),
    accepted_at   timestamptz,
    delivery      text        NOT NULL DEFAULT 'pending'
                              CHECK (delivery IN ('pending', 'sent', 'failed', 'not_configured')),
    UNIQUE (exam_id, email),
    CHECK ((candidate_sub IS NULL) = (accepted_at IS NULL))
);
CREATE INDEX exam_invites_candidate_idx ON exam_invites (candidate_sub) WHERE candidate_sub IS NOT NULL;

-- 문항 응시 시작 시각(서버 시계). 제한 시간을 서버에서 재는 기준이다.
CREATE TABLE exam_item_starts (
    invite_id  uuid        NOT NULL REFERENCES exam_invites (id) ON DELETE CASCADE,
    position   integer     NOT NULL,
    started_at timestamptz NOT NULL,
    PRIMARY KEY (invite_id, position)
);

-- 시험 응시의 제출은 배정·문항에 속한다. 문항마다 한 번만 낸다.
ALTER TABLE submissions
    ADD COLUMN invite_id uuid REFERENCES exam_invites (id),
    ADD COLUMN item_position integer;
CREATE UNIQUE INDEX submissions_invite_item_idx ON submissions (invite_id, item_position)
    WHERE invite_id IS NOT NULL;
