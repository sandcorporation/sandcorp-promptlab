//! 판정기용 docker 소켓 프록시(#95). docker 소켓은 이 프로세스만 갖고, 판정기(`promptlab-judge`의 docker CLI)는
//! `DOCKER_HOST=tcp://docker-proxy:2375`로 이것을 거친다.
//!
//! - 허용 목록 방식: [`rules::classify`]가 받는 호출만, [`rules::check_create`]가 만들기 본문을 판정기 상수와 대조.
//! - 중계: [`proxy`](attach의 HTTP 업그레이드 양방향 복사 포함).
//! - 러너 이미지 미리 받기: [`pull`].
//! - 권한 내리기: [`privileges`].

pub mod privileges;
pub mod proxy;
pub mod pull;
pub mod rules;

pub use proxy::{Owned, Proxy, serve};
