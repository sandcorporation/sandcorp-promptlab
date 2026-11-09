-- AI 활용 평가(#99, spec #93 결정 7): 시험 제출만 평가한다(개인 연습은 과정 로그가 없다). 하루 한도(daily_limit)는 없앤다.
-- 출시 전이라 옛 대화 평가 줄은 비운다(0019가 제출을 비워 이미 비어 있다).
DELETE FROM submission_evaluations;
DROP INDEX IF EXISTS submission_evaluations_practice_idx;
ALTER TABLE submission_evaluations
    DROP CONSTRAINT submission_evaluations_status_check,
    DROP COLUMN practice,
    DROP COLUMN candidate_sub,
    ALTER COLUMN org_id SET NOT NULL;
ALTER TABLE submission_evaluations ADD CONSTRAINT submission_evaluations_status_check
    CHECK (status IN ('queued', 'evaluating', 'done', 'failed'));
