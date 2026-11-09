-- 응시 가능 시간(#14). 채용 시험에만 쓰고(규칙은 promptlab_shared::live), 기본은 열림·범위 없음이라
-- 이전에 만든 시험은 그대로 응시할 수 있다. 실시간 보기(WebRTC) 시그널링은 서버 메모리에만 두고 테이블이 없다.
ALTER TABLE exams
    ADD COLUMN window_open      boolean     NOT NULL DEFAULT true,
    ADD COLUMN window_starts_at timestamptz,
    ADD COLUMN window_ends_at   timestamptz,
    ADD CONSTRAINT exams_window_range CHECK (
        window_starts_at IS NULL OR window_ends_at IS NULL OR window_starts_at < window_ends_at);
