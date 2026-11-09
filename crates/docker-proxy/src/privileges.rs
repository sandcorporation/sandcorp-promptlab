//! root로 시작했으면 소켓의 그룹만 남기고 nobody(65534)로 내린다.
//!
//! 소켓 그룹(호스트 `docker` 그룹 gid)은 호스트마다 달라 이미지·compose에 박을 수 없다. 그래서 compose는 이 컨테이너를
//! root + `cap_drop: [ALL]` + `cap_add: [SETUID, SETGID]`로 띄우고, 여기서 소켓 파일의 gid를 읽어 보조 그룹으로 둔 뒤
//! uid·gid를 65534로 바꾼다. root가 아닌 uid로 바뀌면 커널이 남은 capability(SETUID·SETGID)를 모두 거둔다 — 그 뒤로는
//! 소켓 그룹 권한 하나만 남는다. root가 아닌 채로 시작했으면(개발·시험) 아무것도 하지 않는다.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// 내려앉을 uid·gid(nobody).
pub const NOBODY: u32 = 65534;

pub fn drop_to_socket_group(socket: &Path) -> Result<(), String> {
    // SAFETY: 인자 없는 libc 조회.
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    let gid = std::fs::metadata(socket)
        .map_err(|e| format!("docker 소켓 {}: {e}", socket.display()))?
        .gid();
    let groups = [gid as libc::gid_t];
    // SAFETY: 길이 1 배열을 넘긴다. 순서: 보조 그룹 → gid → uid(uid를 먼저 바꾸면 나머지를 못 바꾼다).
    unsafe {
        if libc::setgroups(1, groups.as_ptr()) != 0 {
            return Err(format!("setgroups: {}", std::io::Error::last_os_error()));
        }
        if libc::setgid(NOBODY) != 0 {
            return Err(format!("setgid: {}", std::io::Error::last_os_error()));
        }
        if libc::setuid(NOBODY) != 0 {
            return Err(format!("setuid: {}", std::io::Error::last_os_error()));
        }
        // 되돌릴 수 없어야 한다.
        if libc::setuid(0) == 0 {
            return Err("root로 되돌아갈 수 있다".into());
        }
    }
    tracing::info!(uid = NOBODY, socket_gid = gid, "권한 내림");
    Ok(())
}
