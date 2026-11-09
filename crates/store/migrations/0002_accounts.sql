-- 계정·조직(테넌트)·역할(#7). 사람은 sandcorp-auth 토큰의 `sub`(바뀌지 않는 키)로 묶는다 — 이메일로 묶지 않는다.

CREATE TABLE users (
    sub        text        PRIMARY KEY,
    -- 대시보드 로그인 때 ID 토큰의 표시 이름(없을 수 있다).
    name       text,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE orgs (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text        NOT NULL CHECK (length(btrim(name)) > 0),
    created_by text        NOT NULL REFERENCES users (sub),
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
    org_id     uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    sub        text        NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    role       text        NOT NULL CHECK (role IN ('admin', 'author', 'proctor', 'viewer')),
    joined_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, sub)
);
CREATE INDEX memberships_sub_idx ON memberships (sub);

-- 동료 초대 링크. 토큰 원문은 만들 때 한 번만 응답에 싣고, 여기에는 sha256만 둔다.
CREATE TABLE org_invites (
    token_hash  bytea       PRIMARY KEY,
    org_id      uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    role        text        NOT NULL CHECK (role IN ('admin', 'author', 'proctor', 'viewer')),
    created_by  text        NOT NULL REFERENCES users (sub),
    created_at  timestamptz NOT NULL DEFAULT now(),
    expires_at  timestamptz NOT NULL,
    accepted_by text        REFERENCES users (sub),
    accepted_at timestamptz
);

-- 대시보드 로그인 진행 중 상태(서버 쪽 세션): state·PKCE code_verifier. 브라우저 쿠키에는 id만, 여기에는 그 해시.
CREATE TABLE login_states (
    id_hash    bytea       PRIMARY KEY,
    state      text        NOT NULL,
    verifier   text        NOT NULL,
    next_path  text        NOT NULL,
    expires_at timestamptz NOT NULL
);

-- 대시보드 세션. 쿠키 값의 sha256만 둔다. 12시간 이하(auth 문서 7절).
CREATE TABLE sessions (
    id_hash    bytea       PRIMARY KEY,
    sub        text        NOT NULL REFERENCES users (sub) ON DELETE CASCADE,
    -- 대표(acct 없음) 계정인가. 일반 사용자는 false.
    is_owner   boolean     NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

-- 제출은 낸 사람(sub)과 (있으면) 조직에 속한다. 시범(#4) 때의 행은 낸 사람이 없다('') — 아무도 보지 못한다.
ALTER TABLE submissions
    ADD COLUMN org_id uuid REFERENCES orgs (id),
    ADD COLUMN candidate_sub text NOT NULL DEFAULT '';
ALTER TABLE submissions ALTER COLUMN candidate_sub DROP DEFAULT;
CREATE INDEX submissions_org_idx ON submissions (org_id, created_at) WHERE org_id IS NOT NULL;
CREATE INDEX submissions_candidate_idx ON submissions (candidate_sub, created_at);
