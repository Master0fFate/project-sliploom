use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use super::{
    CheckedStream, IDLE_TIMEOUT, MAX_CHUNK, MAX_NAME_BYTES, Receipt, TlsStream, check_deadline,
    check_directory, filename, frame_header, publish_file, read_frame, write_frame,
};
use crate::{MAX_BYTES, Result, invalid};

#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::trust::{ReceiveDestination, ReceiveStaging};

pub const MAX_FILES: usize = 32;
pub const MAX_BATCH_BYTES: u64 = 256 * 1024 * 1024;
pub(super) const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const BATCH_OFFER: u8 = 0x09;
pub(super) const APPROVE: u8 = 0x0a;
pub(super) const DECLINE: u8 = 0x07;
pub(super) const NAME: u8 = 0x08;
pub(super) const DATA: u8 = 0x04;
pub(super) const END: u8 = 0x05;
pub(super) const COMMITTED: u8 = 0x06;
pub(super) const END_LEN: usize = 42;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchOffer {
    pub count: usize,
    pub max_bytes: u64,
    pub has_unknown_sizes: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchOutcome {
    pub total: usize,
    pub confirmed: usize,
    pub unconfirmed: bool,
    pub status: BatchStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchStatus {
    Complete,
    Declined,
    Cancelled,
    Failed,
    TooLarge,
    Unsupported,
}

pub(super) enum Destination {
    Path(PathBuf),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Trusted(ReceiveDestination),
}

impl Destination {
    fn stage(&self) -> io::Result<Staging<'_>> {
        match self {
            Self::Path(path) => {
                check_directory(path)?;
                Ok(Staging::Path(path, NamedTempFile::new_in(path)?))
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Trusted(directory) => Ok(Staging::Trusted(directory.stage()?)),
        }
    }
}

enum Staging<'a> {
    Path(&'a Path, NamedTempFile),
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Trusted(ReceiveStaging<'a>),
}

impl Staging<'_> {
    fn file(&mut self) -> &mut std::fs::File {
        match self {
            Self::Path(_, staging) => staging.as_file_mut(),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Trusted(staging) => staging.file(),
        }
    }

    fn publish(
        self,
        stream: &impl CheckedStream,
        stem: &str,
        extension: Option<&str>,
        publish_gate: Option<&Mutex<()>>,
    ) -> io::Result<PathBuf> {
        match self {
            Self::Path(path, staging) => {
                publish_file(stream, path, staging, stem, extension, publish_gate)
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            Self::Trusted(staging) => staging.publish(
                stream,
                stem,
                extension,
                publish_gate.ok_or_else(|| invalid("trusted receive requires a publish gate"))?,
            ),
        }
    }
}

pub(super) fn receive(
    stream: &mut TlsStream<'_>,
    folder: &Destination,
    approve: impl FnOnce(BatchOffer) -> bool,
    mut saved: impl FnMut(Receipt),
    mut progress: impl FnMut(u64),
    publish_gate: Option<&Mutex<()>>,
) -> Result<()> {
    let (offer, lengths) = match read_offer(stream) {
        Ok(offer) => offer,
        Err(error) => {
            let _ = write_frame(stream, DECLINE, &[]);
            return Err(error.into());
        }
    };
    stream.check()?;
    let approval_deadline = Instant::now() + APPROVAL_TIMEOUT;
    let approved = approve(offer);
    let now = Instant::now();
    check_deadline(stream.sock.cancelled, stream.sock.deadline)?;
    stream.sock.idle_deadline = now + IDLE_TIMEOUT;
    if !approved || now >= approval_deadline {
        write_frame(stream, DECLINE, &[])?;
        return Ok(());
    }
    write_frame(stream, APPROVE, &[])?;

    let mut bytes = 0;
    for (index, size) in lengths.into_iter().enumerate() {
        let (path, length, end) =
            receive_item(stream, folder, index as u16, size, publish_gate, |n| {
                progress(bytes + n);
            })?;
        let ack = write_frame(stream, COMMITTED, &end);
        saved(Receipt {
            path,
            bytes: length,
            confirmed: ack.is_ok(),
        });
        ack?;
        bytes += length;
    }
    Ok(())
}

fn read_offer(stream: &mut impl CheckedStream) -> io::Result<(BatchOffer, Vec<Option<u64>>)> {
    let (kind, length) = frame_header(stream)?;
    let length = length as usize;
    if kind != BATCH_OFFER || !(8..=8 * MAX_FILES).contains(&length) || !length.is_multiple_of(8) {
        return Err(invalid("invalid BATCH_OFFER frame"));
    }
    let mut payload = [0; MAX_FILES * 8];
    stream.read_exact(&mut payload[..length])?;
    let mut lengths = Vec::with_capacity(length / 8);
    let mut max_bytes = 0;
    for encoded in payload[..length].as_chunks::<8>().0 {
        let length = u64::from_be_bytes(*encoded);
        let size = (length != u64::MAX).then_some(length);
        let reservation = size.unwrap_or(MAX_BYTES);
        if reservation > MAX_BYTES || max_bytes + reservation > MAX_BATCH_BYTES {
            return Err(invalid("batch exceeds allowed length"));
        }
        max_bytes += reservation;
        lengths.push(size);
    }
    Ok((
        BatchOffer {
            count: lengths.len(),
            max_bytes,
            has_unknown_sizes: lengths.iter().any(Option::is_none),
        },
        lengths,
    ))
}

fn receive_item(
    stream: &mut impl CheckedStream,
    folder: &Destination,
    index: u16,
    declared: Option<u64>,
    publish_gate: Option<&Mutex<()>>,
    mut progress: impl FnMut(u64),
) -> Result<(PathBuf, u64, [u8; END_LEN])> {
    let (kind, length) = frame_header(stream)?;
    if kind != NAME || length as usize > MAX_NAME_BYTES {
        return Err(invalid("expected batch NAME frame").into());
    }
    let mut name = [0; MAX_NAME_BYTES];
    let name = &mut name[..length as usize];
    stream.read_exact(name)?;
    let name = str::from_utf8(name).map_err(|_| invalid("invalid NAME frame encoding"))?;
    let (stem, extension) = filename::batch_parts(name);
    let limit = declared.unwrap_or(MAX_BYTES);

    stream.check()?;
    let mut staging = folder.stage()?;
    let mut chunk = [0; MAX_CHUNK];
    let mut bytes = 0_u64;
    let mut digest = Sha256::new();
    let end = loop {
        match frame_header(stream)? {
            (DATA, count) if (1..=MAX_CHUNK as u32).contains(&count) => {
                let total = bytes + u64::from(count);
                if total > limit {
                    return Err(invalid("transfer exceeds allowed length").into());
                }
                let data = &mut chunk[..count as usize];
                stream.read_exact(data)?;
                stream.check()?;
                staging.file().write_all(data)?;
                digest.update(data);
                bytes = total;
                progress(bytes);
            }
            (END, length) if length as usize == END_LEN => {
                let mut end = [0; END_LEN];
                stream.read_exact(&mut end)?;
                if declared.is_some_and(|size| size != bytes)
                    || end != end_payload(index, bytes, digest.finalize().into())
                {
                    return Err(invalid("transfer integrity check failed").into());
                }
                break end;
            }
            _ => return Err(invalid("unexpected batch transfer frame").into()),
        }
    };
    stream.check()?;
    staging.file().sync_all()?;
    let path = staging.publish(stream, stem, extension, publish_gate)?;
    Ok((path, bytes, end))
}

pub(super) fn end_payload(index: u16, bytes: u64, digest: [u8; 32]) -> [u8; END_LEN] {
    let mut end = [0; END_LEN];
    end[..2].copy_from_slice(&index.to_be_bytes());
    end[2..10].copy_from_slice(&bytes.to_be_bytes());
    end[10..].copy_from_slice(&digest);
    end
}

pub(super) fn read_committed(stream: &mut impl CheckedStream) -> io::Result<[u8; END_LEN]> {
    read_frame(stream, COMMITTED)
}
