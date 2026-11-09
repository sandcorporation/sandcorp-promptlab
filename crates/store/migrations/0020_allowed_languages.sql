-- 코딩 C(#96): 시험이 허용하는 언어(1개 이상, 기본 4개 전부). 응시자는 이 중에서 고르고 서버는 밖의 언어 제출을 422로 거른다.
ALTER TABLE exams ADD COLUMN allowed_languages jsonb NOT NULL
    DEFAULT '["python","javascript","java","cpp"]'::jsonb
    CHECK (jsonb_typeof(allowed_languages) = 'array' AND jsonb_array_length(allowed_languages) >= 1);
