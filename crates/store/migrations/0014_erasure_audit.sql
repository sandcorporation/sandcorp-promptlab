-- 고객사 관리자의 응시자 데이터 삭제 감사 로그(#55). 규칙은 promptlab_shared::erasure.
-- 이메일 원문·지운 내용은 남기지 않는다: 응시자는 subject_hash(조직 id로 묶은 sha256)로만, 배정 id는 지워진 행을 가리키는
-- 무작위 uuid라 FK를 걸지 않는다. 보관 기간(3년)이 지나면 #54 주기 삭제가 지운다.
CREATE TABLE erasure_audit (
    id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    org_id       uuid        NOT NULL REFERENCES orgs (id) ON DELETE CASCADE,
    exam_id      uuid        REFERENCES exams (id) ON DELETE SET NULL,
    invite_id    uuid        NOT NULL,
    subject_hash text        NOT NULL,
    erased_by    text        NOT NULL REFERENCES users (sub),
    erased_at    timestamptz NOT NULL,
    reason       text,
    -- 종류별 지운 건수(promptlab_shared::retention::Erased).
    counts       jsonb       NOT NULL,
    -- 같은 배정은 한 번만 기록한다(다시 요청하면 이 기록을 돌려준다 — 멱등).
    UNIQUE (org_id, invite_id)
);
CREATE INDEX erasure_audit_org_idx ON erasure_audit (org_id, erased_at DESC);
CREATE INDEX erasure_audit_subject_idx ON erasure_audit (org_id, subject_hash);
CREATE INDEX erasure_audit_at_idx ON erasure_audit (erased_at);
