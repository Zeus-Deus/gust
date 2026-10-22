//! The only archive decoder. The caller must put this process in the sandbox.
//! It never extracts to disk and never prints library diagnostics or secrets.
use gust_ops::archive::protocol::{self, Frame, Member, ReadRequest};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::io::{Read, Write};

#[link(name = "archive")]
unsafe extern "C" {
    fn archive_read_new() -> *mut c_void;
    fn archive_read_support_filter_all(a: *mut c_void) -> c_int;
    fn archive_read_support_format_all(a: *mut c_void) -> c_int;
    fn archive_read_add_passphrase(a: *mut c_void, password: *const c_char) -> c_int;
    fn archive_read_open_filename(a: *mut c_void, path: *const c_char, block: usize) -> c_int;
    fn archive_read_next_header(a: *mut c_void, entry: *mut *mut c_void) -> c_int;
    fn archive_read_data(a: *mut c_void, data: *mut c_void, len: usize) -> isize;
    fn archive_read_data_skip(a: *mut c_void) -> c_int;
    fn archive_read_free(a: *mut c_void) -> c_int;
    fn archive_entry_pathname(entry: *mut c_void) -> *const c_char;
    fn archive_entry_mode(entry: *mut c_void) -> u32;
    fn archive_entry_size(entry: *mut c_void) -> i64;
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
                return Err(());
            }
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
                    let mut data = [0; protocol::DATA_CHUNK];
                    loop {
                        let count =
                            archive_read_data(archive.0, data.as_mut_ptr().cast(), data.len());
                        if count < 0 {
                            return Err(());
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
    if run().is_err() {
        let _ = protocol::write_frame(&mut std::io::stdout().lock(), &Frame::Failed);
        std::process::exit(1);
    }
    let _ = std::io::stdout().flush();
}
