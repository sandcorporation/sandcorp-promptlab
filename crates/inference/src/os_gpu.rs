//! OS가 세는 GPU 전용 메모리 사용량(#116 루프 3). llama.cpp(Vulkan 메모리 예산)는 Windows에서 다른 프로세스가
//! 쓰는 VRAM을 빼지 못해(다른 앱이 3.3GB를 쓰는데 여유 7181MiB) 7B 전 층을 골랐고, WDDM이 가중치를 시스템 메모리로
//! 내려 PCIe로 돌았다(2.7 tok/s). 작업 관리자의 「전용 GPU 메모리」와 같은 PDH 카운터로 모든 프로세스의 사용량을 읽는다.

/// 모든 프로세스가 쓰는 GPU 전용 메모리(바이트). 어댑터가 여럿이면(내장 + 외장) 가장 큰 값 — 넉넉히 빼는 쪽이다.
/// Windows가 아니거나 읽지 못하면 `None`.
#[cfg(windows)]
pub fn dedicated_usage_bytes() -> Option<u64> {
    use windows_sys::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_LARGE, PDH_MORE_DATA, PdhAddEnglishCounterW,
        PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    const ERROR_SUCCESS: u32 = 0;
    let path: Vec<u16> = r"\GPU Adapter Memory(*)\Dedicated Usage"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: PDH 호출 규약대로 — 쿼리 핸들은 이 함수 안에서 열고 닫고, 배열 버퍼는 PDH가 알려 준 크기(바이트)로
    // 8바이트 정렬해 잡는다. 항목의 이름 포인터는 같은 버퍼를 가리키지만 읽지 않는다.
    unsafe {
        let mut query = std::mem::zeroed();
        if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != ERROR_SUCCESS {
            return None;
        }
        let result = (|| {
            let mut counter = std::mem::zeroed();
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != ERROR_SUCCESS {
                return None;
            }
            // 사용량은 순간값 카운터라 한 번 모으면 된다.
            if PdhCollectQueryData(query) != ERROR_SUCCESS {
                return None;
            }
            let (mut bytes, mut count) = (0u32, 0u32);
            let status = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_LARGE,
                &mut bytes,
                &mut count,
                std::ptr::null_mut(),
            );
            if status != PDH_MORE_DATA || bytes == 0 {
                return None;
            }
            let mut buf = vec![0u64; (bytes as usize).div_ceil(8)];
            let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(counter, PDH_FMT_LARGE, &mut bytes, &mut count, items)
                != ERROR_SUCCESS
            {
                return None;
            }
            (0..count as usize)
                .map(|i| &*items.add(i))
                .filter(|item| item.FmtValue.CStatus == ERROR_SUCCESS)
                .map(|item| item.FmtValue.Anonymous.largeValue.max(0) as u64)
                .max()
        })();
        PdhCloseQuery(query);
        result
    }
}

#[cfg(not(windows))]
pub fn dedicated_usage_bytes() -> Option<u64> {
    None
}
