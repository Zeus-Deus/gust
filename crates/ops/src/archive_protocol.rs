//! Bounded wire protocol shared by the sandbox helper and its parent.
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 64 * 1024;
pub const DATA_CHUNK: usize = 4096;
pub const MAX_PATH: usize = 4096;
pub const MAX_HEADERS: usize = 200_000;
pub const MAX_METADATA: usize = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
pub struct ReadRequest {
    pub password: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Frame {
    Header(Member),
    Data(Vec<u8>),
    EndMember,
    EndArchive,
    Failed,
    PasswordRequired,
    IncorrectPassword,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub name: Vec<u8>,
    pub mode: u32,
    pub size: u64,
    pub link: Option<Vec<u8>>,
    pub hardlink: bool,
}

pub fn write_frame(writer: &mut impl std::io::Write, frame: &Frame) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(frame)?;
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::other("archive frame exceeds limit"));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

/// Keep secret erasure observable even when its allocation is about to drop.
pub fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: each pointer is a live exclusive byte of this slice.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub enum WriteFormat {
    TarZstd,
    Zip,
    SevenZip,
}
#[derive(Serialize, Deserialize)]
pub struct WriteRequest {
    pub format: WriteFormat,
    pub password: Vec<u8>,
}
#[derive(Serialize, Deserialize)]
pub enum WriteFrame {
    Header(Member),
    Data(Vec<u8>),
    EndMember,
    Finish,
}
#[derive(Serialize, Deserialize)]
pub enum WriteReply {
    Ready,
    Done,
    Failed,
}

pub fn read_message<T: serde::de::DeserializeOwned>(
    reader: &mut impl std::io::Read,
) -> std::io::Result<T> {
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(std::io::Error::other("archive frame exceeds limit"));
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let result = serde_json::from_slice(&bytes).map_err(std::io::Error::from);
    wipe(&mut bytes);
    result
}
pub fn write_message<T: Serialize>(
    writer: &mut impl std::io::Write,
    message: &T,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_FRAME {
        wipe(&mut bytes);
        return Err(std::io::Error::other("archive frame exceeds limit"));
    }
    let result = writer
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|_| writer.write_all(&bytes))
        .and_then(|_| writer.flush());
    wipe(&mut bytes);
    result
}
