//! Sandbox-only archive codec. Decoding never extracts to disk; creation
//! writes only the parent-owned output. Diagnostics and secrets stay private.
use gust_ops::archive::protocol::{self, Frame, Member, ReadRequest};
use std::ffi::{c_char, c_int, c_long, c_void, CStr, CString};
use std::io::{Read, Write};

#[link(name = "archive")]
unsafe extern "C" {
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    fn archive_read_new() -> *mut c_void;
    fn archive_read_support_filter_all(a: *mut c_void) -> c_int;
    fn archive_read_support_format_all(a: *mut c_void) -> c_int;
    fn archive_read_add_passphrase(a: *mut c_void, password: *const c_char) -> c_int;
    fn archive_read_open_filename(a: *mut c_void, path: *const c_char, block: usize) -> c_int;
    fn archive_read_next_header(a: *mut c_void, entry: *mut *mut c_void) -> c_int;
    fn archive_read_data(a: *mut c_void, data: *mut c_void, len: usize) -> isize;
    fn archive_read_data_skip(a: *mut c_void) -> c_int;
    fn archive_read_free(a: *mut c_void) -> c_int;
    fn archive_error_string(a: *mut c_void) -> *const c_char;
    fn archive_entry_is_encrypted(entry: *mut c_void) -> c_int;
    fn archive_entry_pathname(entry: *mut c_void) -> *const c_char;
    fn archive_entry_mode(entry: *mut c_void) -> u32;
    fn archive_entry_size(entry: *mut c_void) -> i64;
    fn archive_entry_mtime_is_set(entry: *mut c_void) -> c_int;
    fn archive_entry_mtime(entry: *mut c_void) -> i64;
    fn archive_entry_mtime_nsec(entry: *mut c_void) -> c_long;
    fn archive_entry_hardlink(entry: *mut c_void) -> *const c_char;
    fn archive_entry_symlink(entry: *mut c_void) -> *const c_char;
}

struct Archive(*mut c_void);
impl Drop for Archive {
    fn drop(&mut self) {
        // SAFETY: the reader is owned here and freed exactly once.
        unsafe { archive_read_free(self.0) };
    }
}

unsafe fn bytes(ptr: *const c_char) -> Result<Vec<u8>, ()> {
    if ptr.is_null() {
        return Err(());
    }
    // SAFETY: libarchive owns a terminated string valid until the next header.
    let value = unsafe { CStr::from_ptr(ptr) }.to_bytes();
    if value.len() > protocol::MAX_PATH {
        return Err(());
    }
    Ok(value.to_vec())
}

/// libarchive exposes passphrase verification failures through its fixed
/// library diagnostics, not a localized command prompt. Classify here and
/// emit only typed outcomes; never return the diagnostic or secret bytes.
unsafe fn failure_frame(archive: *mut c_void, has_password: bool) -> Frame {
    // SAFETY: this is the current reader's terminated diagnostic string.
    let message = unsafe { archive_error_string(archive) };
    if message.is_null() {
        return Frame::Failed;
    }
    let message = unsafe { CStr::from_ptr(message) }.to_bytes();
    let contains = |word: &[u8]| message.windows(word.len()).any(|part| part == word);
    if has_password && (contains(b"Incorrect passphrase") || contains(b"Incorrect password")) {
        Frame::IncorrectPassword
    } else if !has_password
        && (contains(b"Passphrase required")
            || contains(b"passphrase required")
            || contains(b"password required"))
    {
        Frame::PasswordRequired
    } else {
        Frame::Failed
    }
}

fn run() -> Result<(), ()> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut len = [0; 4];
    input.read_exact(&mut len).map_err(|_| ())?;
    let len = u32::from_le_bytes(len) as usize;
    if len > protocol::MAX_FRAME {
        return Err(());
    }
    let mut request = vec![0; len];
    input.read_exact(&mut request).map_err(|_| ())?;
    let mut request_value: ReadRequest = serde_json::from_slice(&request).map_err(|_| ())?;
    protocol::wipe(&mut request);
    let has_password = !request_value.password.is_empty();
    let password = CString::new(std::mem::take(&mut request_value.password)).map_err(|_| ())?;
    // SAFETY: all calls use this live owned reader and valid buffers; no
    // libarchive pointer escapes the helper or survives its owning reader.
    unsafe {
        let archive = Archive(archive_read_new());
        if archive.0.is_null() {
            return Err(());
        }
        // Do not enable external program filters (which can execute programs).
        if archive_read_support_filter_all(archive.0) < 0
            || archive_read_support_format_all(archive.0) < 0
        {
            return Err(());
        }
        if !password.as_bytes().is_empty()
            && archive_read_add_passphrase(archive.0, password.as_ptr()) != 0
        {
            return Err(());
        }
        let mut secret = password.into_bytes_with_nul();
        // libarchive copies the passphrase into its own private reader state.
        protocol::wipe(&mut secret);
        if archive_read_open_filename(archive.0, c"/input".as_ptr(), 64 * 1024) != 0 {
            return Err(());
        }
        loop {
            let mut entry = std::ptr::null_mut();
            let status = archive_read_next_header(archive.0, &mut entry);
            if status == 1 {
                protocol::write_frame(&mut output, &Frame::EndArchive).map_err(|_| ())?;
                return Ok(());
            }
            // Warnings are not proof of a complete or unambiguous parse.
            if status != 0 || entry.is_null() {
                protocol::write_frame(&mut output, &failure_frame(archive.0, has_password))
                    .map_err(|_| ())?;
                return Ok(());
            }
            let encrypted = archive_entry_is_encrypted(entry) > 0;
            let hard = archive_entry_hardlink(entry);
            let link = if !hard.is_null() {
                hard
            } else {
                archive_entry_symlink(entry)
            };
            let size = archive_entry_size(entry);
            if size < 0 {
                return Err(());
            }
            let member = Member {
                name: bytes(archive_entry_pathname(entry))?,
                mode: archive_entry_mode(entry),
                size: size as u64,
                link: if link.is_null() {
                    None
                } else {
                    Some(bytes(link)?)
                },
                hardlink: !hard.is_null(),
                mtime: if archive_entry_mtime_is_set(entry) > 0 {
                    Some((
                        archive_entry_mtime(entry),
                        u32::try_from(archive_entry_mtime_nsec(entry)).map_err(|_| ())?,
                    ))
                } else {
                    None
                },
            };
            protocol::write_frame(&mut output, &Frame::Header(member)).map_err(|_| ())?;
            let mut decision = [0];
            input.read_exact(&mut decision).map_err(|_| ())?;
            match decision[0] {
                b'S' => {
                    if archive_read_data_skip(archive.0) != 0 {
                        return Err(());
                    }
                }
                b'R' => {
                    if encrypted && !has_password {
                        protocol::write_frame(&mut output, &Frame::PasswordRequired)
                            .map_err(|_| ())?;
                        return Ok(());
                    }
                    let mut data = [0; protocol::DATA_CHUNK];
                    loop {
                        let count =
                            archive_read_data(archive.0, data.as_mut_ptr().cast(), data.len());
                        if count < 0 {
                            protocol::write_frame(
                                &mut output,
                                &failure_frame(archive.0, has_password),
                            )
                            .map_err(|_| ())?;
                            return Ok(());
                        }
                        if count == 0 {
                            break;
                        }
                        protocol::write_frame(
                            &mut output,
                            &Frame::Data(data[..count as usize].to_vec()),
                        )
                        .map_err(|_| ())?;
                    }
                }
                _ => return Err(()),
            }
            protocol::write_frame(&mut output, &Frame::EndMember).map_err(|_| ())?;
        }
    }
}

fn main() {
    // Linux LC_CTYPE=0. This isolated, single-threaded codec owns its locale;
    // LC_ALL in the environment alone does not initialize native conversion.
    // SAFETY: called before processing input, with a static terminated string.
    if unsafe { setlocale(0, c"C.UTF-8".as_ptr()).is_null() } {
        std::process::exit(2);
    }
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--create")) {
        if write_archive().is_err() {
            let _ = protocol::write_message(
                &mut std::io::stdout().lock(),
                &protocol::WriteReply::Failed,
            );
            std::process::exit(1);
        }
        return;
    }
    if run().is_err() {
        let _ = protocol::write_frame(&mut std::io::stdout().lock(), &Frame::Failed);
        std::process::exit(1);
    }
    let _ = std::io::stdout().flush();
}

#[link(name = "archive")]
unsafe extern "C" {
    fn archive_write_new() -> *mut c_void;
    fn archive_write_set_format_pax(a: *mut c_void) -> c_int;
    fn archive_write_add_filter_zstd(a: *mut c_void) -> c_int;
    fn archive_write_set_format_zip(a: *mut c_void) -> c_int;
    fn archive_write_set_format_7zip(a: *mut c_void) -> c_int;
    fn archive_write_set_options(a: *mut c_void, options: *const c_char) -> c_int;
    fn archive_write_set_passphrase(a: *mut c_void, password: *const c_char) -> c_int;
    fn archive_write_open_filename(a: *mut c_void, path: *const c_char) -> c_int;
    fn archive_write_header(a: *mut c_void, entry: *mut c_void) -> c_int;
    fn archive_write_data(a: *mut c_void, data: *const c_void, len: usize) -> isize;
    fn archive_write_finish_entry(a: *mut c_void) -> c_int;
    fn archive_write_close(a: *mut c_void) -> c_int;
    fn archive_write_free(a: *mut c_void) -> c_int;
    fn archive_entry_new() -> *mut c_void;
    fn archive_entry_free(entry: *mut c_void);
    fn archive_entry_set_pathname(entry: *mut c_void, name: *const c_char);
    fn archive_entry_set_mode(entry: *mut c_void, mode: u32);
    fn archive_entry_set_size(entry: *mut c_void, size: i64);
    fn archive_entry_set_mtime(entry: *mut c_void, seconds: i64, nanoseconds: c_long);
    fn archive_entry_set_symlink(entry: *mut c_void, target: *const c_char);
}
struct WriterArchive(*mut c_void);
impl Drop for WriterArchive {
    fn drop(&mut self) {
        unsafe {
            archive_write_free(self.0);
        }
    }
}
struct Entry(*mut c_void);
impl Drop for Entry {
    fn drop(&mut self) {
        unsafe {
            archive_entry_free(self.0);
        }
    }
}
fn write_archive() -> Result<(), ()> {
    use protocol::{WriteFormat, WriteFrame, WriteReply, WriteRequest};
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    let mut request: WriteRequest = protocol::read_message(&mut input).map_err(|_| ())?;
    let secret = CString::new(std::mem::take(&mut request.password)).map_err(|_| ())?;
    // SAFETY: each native object and all backing strings are owned for every
    // call; no pointers escape this isolated writer process.
    unsafe {
        let archive = WriterArchive(archive_write_new());
        if archive.0.is_null() {
            return Err(());
        }
        let format = match request.format {
            WriteFormat::TarZstd => {
                if archive_write_set_format_pax(archive.0) != 0 {
                    return Err(());
                }
                if archive_write_set_options(archive.0, c"pax:hdrcharset=BINARY".as_ptr()) != 0 {
                    return Err(());
                }
                archive_write_add_filter_zstd(archive.0)
            }
            WriteFormat::Zip => archive_write_set_format_zip(archive.0),
            WriteFormat::SevenZip => archive_write_set_format_7zip(archive.0),
        };
        if format != 0 {
            return Err(());
        }
        if !secret.as_bytes().is_empty()
            && (!matches!(request.format, WriteFormat::Zip)
                || archive_write_set_passphrase(archive.0, secret.as_ptr()) != 0
                || archive_write_set_options(archive.0, c"zip:encryption=aes256".as_ptr()) != 0)
        {
            return Err(());
        }
        let mut secret = secret.into_bytes_with_nul();
        protocol::wipe(&mut secret);
        if archive_write_open_filename(archive.0, c"/output".as_ptr()) != 0 {
            return Err(());
        }
        protocol::write_message(&mut output, &WriteReply::Ready).map_err(|_| ())?;
        let mut remaining: Option<u64> = None;
        let mut seen = 0;
        let mut metadata = 0usize;
        loop {
            let frame: WriteFrame = protocol::read_message(&mut input).map_err(|_| ())?;
            match frame {
                WriteFrame::Header(member) if remaining.is_none() => {
                    let utf8 = std::str::from_utf8(&member.name).is_ok()
                        && member
                            .link
                            .as_ref()
                            .is_none_or(|target| std::str::from_utf8(target).is_ok());
                    if matches!(request.format, WriteFormat::SevenZip) && !utf8 {
                        return Err(());
                    }
                    if matches!(request.format, WriteFormat::Zip) {
                        // ZIP has a per-member UTF-8 flag. Opaque Unix names
                        // must retain their bytes without claiming UTF-8.
                        let locale = if utf8 { c"C.UTF-8" } else { c"C" };
                        if setlocale(0, locale.as_ptr()).is_null() {
                            return Err(());
                        }
                    }
                    seen += 1;
                    metadata = metadata.saturating_add(
                        member.name.len() + member.link.as_ref().map_or(0, Vec::len) + 128,
                    );
                    if seen > protocol::MAX_HEADERS
                        || metadata > protocol::MAX_METADATA
                        || member.name.len() > protocol::MAX_PATH
                        || member.size > i64::MAX as u64
                    {
                        return Err(());
                    }
                    let name = CString::new(member.name).map_err(|_| ())?;
                    let target = member.link.map(CString::new).transpose().map_err(|_| ())?;
                    let entry = Entry(archive_entry_new());
                    if entry.0.is_null() {
                        return Err(());
                    }
                    archive_entry_set_pathname(entry.0, name.as_ptr());
                    archive_entry_set_mode(entry.0, member.mode & !0o6000);
                    archive_entry_set_size(entry.0, member.size as i64);
                    if let Some((seconds, nanoseconds)) = member.mtime {
                        if !request.format.supports_timestamp(seconds, nanoseconds) {
                            return Err(());
                        }
                        archive_entry_set_mtime(entry.0, seconds, nanoseconds.into());
                    }
                    if let Some(target) = &target {
                        archive_entry_set_symlink(entry.0, target.as_ptr());
                    }
                    if archive_write_header(archive.0, entry.0) != 0 {
                        return Err(());
                    }
                    remaining = Some(member.size);
                }
                WriteFrame::Data(bytes)
                    if remaining.is_some()
                        && !bytes.is_empty()
                        && bytes.len() <= protocol::DATA_CHUNK =>
                {
                    let left = remaining.as_mut().ok_or(())?;
                    if bytes.len() as u64 > *left
                        || archive_write_data(archive.0, bytes.as_ptr().cast(), bytes.len())
                            != bytes.len() as isize
                    {
                        return Err(());
                    }
                    *left -= bytes.len() as u64;
                }
                WriteFrame::EndMember if remaining == Some(0) => {
                    if archive_write_finish_entry(archive.0) != 0 {
                        return Err(());
                    }
                    remaining = None;
                }
                WriteFrame::Finish if remaining.is_none() => {
                    if archive_write_close(archive.0) != 0 {
                        return Err(());
                    }
                    protocol::write_message(&mut output, &WriteReply::Done).map_err(|_| ())?;
                    return Ok(());
                }
                _ => return Err(()),
            }
            protocol::write_message(&mut output, &WriteReply::Ready).map_err(|_| ())?;
        }
    }
}
