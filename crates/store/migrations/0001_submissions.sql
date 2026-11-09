-- 제출과 채점 큐. 큐는 이 테이블 자체다: 워커가 status='queued'인 가장 오래된 행을
-- SELECT … FOR UPDATE SKIP LOCKED 로 집어 'grading'으로 바꾼다(별도 브로커 없음).
CREATE TABLE submissions (
    id             uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    problem_id     text        NOT NULL,
    prompt         text        NOT NULL,
    turns          integer     NOT NULL CHECK (turns >= 0),
    elapsed_secs   integer     NOT NULL CHECK (elapsed_secs >= 0),
    status         text        NOT NULL DEFAULT 'queued'
                               CHECK (status IN ('queued', 'grading', 'done', 'failed')),
    -- 밖으로 나가는 채점 리포트(grading::GradeReport). 숨은 입력·모델 출력 없음.
    report         jsonb,
    -- 응시자에게 보여도 되는 실패 이유.
    failure_reason text,
    -- 서버 안에서만 보는 실패 상세(검사 규칙 내용이 들어갈 수 있다 — API로 내보내지 않는다).
    failure_detail text,
    attempts       integer     NOT NULL DEFAULT 0,
    created_at     timestamptz NOT NULL DEFAULT now(),
    started_at     timestamptz,
    finished_at    timestamptz,
    CHECK ((status = 'done') = (report IS NOT NULL)),
    CHECK ((status = 'failed') = (failure_reason IS NOT NULL))
);

-- 큐에서 꺼낼 때 기다리는 행만 오래된 순으로 훑는다.
CREATE INDEX submissions_queued_idx ON submissions (created_at) WHERE status = 'queued';
