-- 조직 문제와 풀림 미리 돌리기(#16). 새 테이블만 더한다(기존 행은 그대로).

-- 조직 문제: 출제자가 만든 문제 원본(promptlab_shared::authoring::ProblemSource, 숨은 입력·기준 프롬프트 포함).
-- 그 조직의 시험에서만 쓴다. id 는 'org-' 접두(내장 문제은행 id 와 겹치지 않는다).
CREATE TABLE org_problems (
    id         text        PRIMARY KEY CHECK (id LIKE 'org-%'),
    org_id     uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    source     jsonb       NOT NULL,
    created_by text        NOT NULL REFERENCES users (sub),
    created_at timestamptz NOT NULL,
    updated_by text        NOT NULL REFERENCES users (sub),
    updated_at timestamptz NOT NULL
);
CREATE INDEX org_problems_org_idx ON org_problems (org_id, updated_at DESC);

-- 풀림 미리 돌리기 큐. 채점 워커가 채점 큐가 빌 때 숨은 입력을 **하나씩** 돌린다(한 걸음마다 채점 큐를 다시 본다).
-- cases 는 지금까지 돌린 결과 배열(PreviewCase: 통과·모델 출력·까닭) — 출제자에게만 응답으로 나간다.
CREATE TABLE problem_previews (
    id               uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id           uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    requested_by     text        NOT NULL REFERENCES users (sub),
    source           jsonb       NOT NULL,
    reference_prompt text        NOT NULL,
    total            integer     NOT NULL CHECK (total > 0),
    cases            jsonb       NOT NULL DEFAULT '[]'::jsonb,
    status           text        NOT NULL DEFAULT 'queued'
                                 CHECK (status IN ('queued', 'running', 'done', 'failed')),
    failure_reason   text,
    created_at       timestamptz NOT NULL DEFAULT now(),
    finished_at      timestamptz
);
CREATE INDEX problem_previews_active_idx ON problem_previews (created_at) WHERE status IN ('queued', 'running');
