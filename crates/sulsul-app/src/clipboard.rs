//! 시스템 클립보드 ([`Clipboard`] 구현).
//!
//! Windows에서는 클립보드의 형식을 하나씩 읽어 바이트로 떠 둡니다. 글·서식 있는 글·HTML·그림(DIB, PNG)·
//! 파일 목록처럼 메모리 블록으로 된 형식과 확장 메타파일(EMF)을 되돌릴 수 있습니다. 그림 핸들(비트맵, 팔레트)은
//! Windows가 DIB에서 다시 만들어 주므로 따로 뜨지 않고, 앱 전용 형식(`CF_PRIVATEFIRST` 등)은 되돌릴 수 없어 건너뜁니다.
//! 다른 OS는 아직 지원하지 않습니다.

use crate::dictation::paste::{Clipboard, ClipboardError};

/// 시스템 클립보드
#[derive(Debug, Default)]
pub struct SystemClipboard;

#[cfg(not(windows))]
pub struct Snapshot;

#[cfg(not(windows))]
impl Clipboard for SystemClipboard {
    type Snapshot = Snapshot;

    fn sequence(&self) -> u32 {
        0
    }

    fn snapshot(&mut self) -> Result<Snapshot, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn restore(&mut self, _snapshot: &Snapshot) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn set_text(&mut self, _text: &str, _keep_in_history: bool) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported)
    }
}

#[cfg(windows)]
mod windows {
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{GlobalFree, HANDLE};
    use windows_sys::Win32::Graphics::Gdi::{
        DeleteEnhMetaFile, GetEnhMetaFileBits, SetEnhMetaFileBits,
    };
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
        GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows_sys::Win32::System::Ole::{
        CF_BITMAP, CF_DSPBITMAP, CF_DSPENHMETAFILE, CF_DSPMETAFILEPICT, CF_ENHMETAFILE,
        CF_GDIOBJFIRST, CF_GDIOBJLAST, CF_METAFILEPICT, CF_OWNERDISPLAY, CF_PALETTE,
        CF_PRIVATEFIRST, CF_PRIVATELAST, CF_UNICODETEXT,
    };

    use super::{Clipboard, ClipboardError, SystemClipboard};

    /// 다른 앱이 클립보드를 열고 있을 때 다시 시도하는 횟수와 간격
    const OPEN_ATTEMPTS: u32 = 10;
    const OPEN_RETRY: Duration = Duration::from_millis(20);
    /// 이 형식을 함께 넣으면 Windows 클립보드 기록과 클립보드 관리 앱이 그 내용을 새 복사로 기록하지 않습니다.
    /// 원래 내용을 되돌릴 때 기록에 같은 항목이 하나 더 생기지 않게, 커서 위치에 넣으려고 잠깐 넣은 글이
    /// 기록에 남지 않게 씁니다.
    const EXCLUDE_FROM_HISTORY: &str = "ExcludeClipboardContentFromMonitorProcessing";

    /// 떠 둔 클립보드 내용 (넣었던 순서대로)
    pub struct Snapshot {
        items: Vec<Item>,
    }

    enum Item {
        /// 메모리 블록으로 된 형식
        Memory { format: u32, bytes: Vec<u8> },
        /// 확장 메타파일
        EnhMetaFile(Vec<u8>),
    }

    /// 열어 둔 클립보드. 버리면 닫습니다.
    struct Open;

    impl Open {
        fn new() -> Result<Self, ClipboardError> {
            for attempt in 0..OPEN_ATTEMPTS {
                // SAFETY: 창 없이 엽니다. 닫을 때까지 다른 앱은 클립보드를 열지 못합니다.
                if unsafe { OpenClipboard(std::ptr::null_mut()) } != 0 {
                    return Ok(Self);
                }
                if attempt + 1 < OPEN_ATTEMPTS {
                    std::thread::sleep(OPEN_RETRY);
                }
            }
            Err(ClipboardError::Busy)
        }

        fn empty(&self) -> Result<(), ClipboardError> {
            // SAFETY: 클립보드를 열어 둔 상태입니다.
            if unsafe { EmptyClipboard() } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }

        /// `bytes`를 메모리 블록으로 만들어 넣습니다.
        fn put_memory(&self, format: u32, bytes: &[u8]) -> Result<(), ClipboardError> {
            // SAFETY: 새로 할당한 블록에 크기만큼 복사합니다. 넣는 데 성공하면 블록은 클립보드의 것이 되고,
            // 실패하면 우리가 풉니다.
            unsafe {
                let block = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
                if block.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                let target = GlobalLock(block);
                if target.is_null() {
                    let error = std::io::Error::last_os_error();
                    GlobalFree(block);
                    return Err(error.into());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast::<u8>(), bytes.len());
                GlobalUnlock(block);
                if SetClipboardData(format, block as HANDLE).is_null() {
                    let error = std::io::Error::last_os_error();
                    GlobalFree(block);
                    return Err(error.into());
                }
            }
            Ok(())
        }

        fn put_enh_metafile(&self, bytes: &[u8]) -> Result<(), ClipboardError> {
            // SAFETY: 떠 둔 EMF 바이트로 새 메타파일을 만들어 넣습니다. 실패하면 우리가 지웁니다.
            unsafe {
                let metafile = SetEnhMetaFileBits(bytes.len() as u32, bytes.as_ptr());
                if metafile.is_null() {
                    return Err(std::io::Error::last_os_error().into());
                }
                if SetClipboardData(u32::from(CF_ENHMETAFILE), metafile as HANDLE).is_null() {
                    let error = std::io::Error::last_os_error();
                    DeleteEnhMetaFile(metafile);
                    return Err(error.into());
                }
            }
            Ok(())
        }
    }

    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: `Open::new`에서 연 클립보드를 닫습니다.
            unsafe { CloseClipboard() };
        }
    }

    /// 뜰 수 없는 형식 (그림 핸들, 앱 전용 형식)
    fn skipped(format: u32) -> bool {
        let gdi = [
            CF_BITMAP,
            CF_METAFILEPICT,
            CF_PALETTE,
            CF_OWNERDISPLAY,
            CF_DSPBITMAP,
            CF_DSPMETAFILEPICT,
            CF_DSPENHMETAFILE,
        ];
        gdi.iter().any(|f| u32::from(*f) == format)
            || (u32::from(CF_PRIVATEFIRST)..=u32::from(CF_PRIVATELAST)).contains(&format)
            || (u32::from(CF_GDIOBJFIRST)..=u32::from(CF_GDIOBJLAST)).contains(&format)
    }

    fn registered_format(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: 널로 끝나는 UTF-16 문자열입니다.
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    }

    /// 클립보드의 `format` 형식을 읽습니다. 읽을 수 없으면 `None`.
    fn read(_open: &Open, format: u32) -> Option<Item> {
        // SAFETY: 클립보드를 열어 둔 상태에서 받은 핸들은 닫기 전까지 유효합니다. 메모리 블록은 잠근 크기만큼만 읽습니다.
        unsafe {
            let handle = GetClipboardData(format);
            if handle.is_null() {
                return None;
            }
            if format == u32::from(CF_ENHMETAFILE) {
                let size = GetEnhMetaFileBits(handle, 0, std::ptr::null_mut());
                if size == 0 {
                    return None;
                }
                let mut bytes = vec![0u8; size as usize];
                if GetEnhMetaFileBits(handle, size, bytes.as_mut_ptr()) == 0 {
                    return None;
                }
                return Some(Item::EnhMetaFile(bytes));
            }
            let size = GlobalSize(handle);
            let source = GlobalLock(handle);
            if source.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(source.cast::<u8>(), size).to_vec();
            GlobalUnlock(handle);
            Some(Item::Memory { format, bytes })
        }
    }

    impl Clipboard for SystemClipboard {
        type Snapshot = Snapshot;

        fn sequence(&self) -> u32 {
            // SAFETY: 인자가 없는 함수입니다.
            unsafe { GetClipboardSequenceNumber() }
        }

        fn snapshot(&mut self) -> Result<Snapshot, ClipboardError> {
            let open = Open::new()?;
            let mut items = Vec::new();
            let mut format = 0;
            loop {
                // SAFETY: 클립보드를 열어 둔 상태입니다.
                format = unsafe { EnumClipboardFormats(format) };
                if format == 0 {
                    break;
                }
                if skipped(format) {
                    continue;
                }
                match read(&open, format) {
                    Some(item) => items.push(item),
                    None => tracing::debug!("clipboard format {format} could not be read"),
                }
            }
            tracing::debug!(formats = items.len(), "saved the clipboard");
            Ok(Snapshot { items })
        }

        fn restore(&mut self, snapshot: &Snapshot) -> Result<(), ClipboardError> {
            let open = Open::new()?;
            open.empty()?;
            for item in &snapshot.items {
                let result = match item {
                    Item::Memory { format, bytes } => open.put_memory(*format, bytes),
                    Item::EnhMetaFile(bytes) => open.put_enh_metafile(bytes),
                };
                // 한 형식을 넣지 못해도 나머지는 넣습니다.
                if let Err(error) = result {
                    tracing::debug!("could not restore a clipboard format: {error}");
                }
            }
            if !snapshot.items.is_empty() {
                exclude_from_history(&open);
            }
            Ok(())
        }

        fn set_text(&mut self, text: &str, keep_in_history: bool) -> Result<(), ClipboardError> {
            // Windows의 글 형식은 줄바꿈이 CRLF입니다.
            let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
            let bytes: Vec<u8> = text
                .encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_ne_bytes)
                .collect();
            let open = Open::new()?;
            open.empty()?;
            open.put_memory(u32::from(CF_UNICODETEXT), &bytes)?;
            if !keep_in_history {
                exclude_from_history(&open);
            }
            Ok(())
        }
    }

    /// 열어 둔 클립보드의 지금 내용을 클립보드 기록·동기화에서 뺍니다. 실패해도 내용은 그대로 둡니다.
    fn exclude_from_history(open: &Open) {
        let exclude = registered_format(EXCLUDE_FROM_HISTORY);
        if exclude != 0
            && let Err(error) = open.put_memory(exclude, &0u32.to_ne_bytes())
        {
            tracing::debug!("could not exclude the clipboard content from history: {error}");
        }
    }
}
