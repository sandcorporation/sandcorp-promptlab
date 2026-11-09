-- 운영자 화면(#19): 시범 조직 승인, 운영자 문제은행 문항·내리기, 허용 목록 변경 이력. 더하기만 한다.

-- 시범 조직 승인. 승인 전 조직은 시험을 만들 수 없고 문항을 시작할 수 없다.
-- 이 마이그레이션 전에 있던 조직(운영 중인 조직·스모크 조직)은 모두 승인된 것으로 둔다 — 지금 쓰는 조직을 막지 않는다.
ALTER TABLE orgs
    ADD COLUMN approved_at timestamptz,
    ADD COLUMN approved_by text REFERENCES users (sub);
UPDATE orgs SET approved_at = created_at WHERE approved_at IS NULL;

-- 운영자 문제은행 문항(내장 문제은행 = 저장소 파일에 더하는 우리 문항). 모든 조직의 문제은행에 들어간다.
-- 연습/시험 구분은 만들 때 정하고 바꾸지 않는다(시험 문항을 연습으로 돌리면 숨은 입력 통과율이 새는 통로가 된다).
-- id 는 'bank-' 접두(내장 문제 id·'org-' 조직 문제와 겹치지 않는다).
CREATE TABLE bank_problems (
    id         text        PRIMARY KEY CHECK (id LIKE 'bank-%'),
    practice   boolean     NOT NULL,
    source     jsonb       NOT NULL,
    created_by text        NOT NULL REFERENCES users (sub),
    created_at timestamptz NOT NULL,
    updated_by text        NOT NULL REFERENCES users (sub),
    updated_at timestamptz NOT NULL
);

-- 내린 문항(내장·운영자 문항 모두). 새 시험·연습 목록에서 빠지고, 이미 만든 시험·과거 결과는 그대로 문제를 찾는다.
CREATE TABLE problem_retirements (
    problem_id text        PRIMARY KEY,
    reason     text        NOT NULL DEFAULT '',
    retired_by text        NOT NULL REFERENCES users (sub),
    retired_at timestamptz NOT NULL
);

-- 허용 목록 변경 이력(덧붙이기만). 값마다 마지막 변경이 지금 상태다:
-- allow = 상수 목록에 더함, block = 막음(상수에 있어도), reset = 상수 기본값으로 되돌림.
CREATE TABLE integrity_allowlist_changes (
    id         bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind       text        NOT NULL CHECK (kind IN ('app_version', 'model_sha256')),
    value      text        NOT NULL CHECK (length(value) BETWEEN 1 AND 128),
    action     text        NOT NULL CHECK (action IN ('allow', 'block', 'reset')),
    note       text        NOT NULL DEFAULT '',
    changed_by text        NOT NULL REFERENCES users (sub),
    changed_at timestamptz NOT NULL
);
CREATE INDEX integrity_allowlist_changes_value_idx ON integrity_allowlist_changes (kind, value, id DESC);
