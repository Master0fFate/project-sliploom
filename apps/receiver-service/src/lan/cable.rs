use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use super::{
    ACCEPT, AUTH_TIMEOUT, BoundedTcp, Cancellation, CheckedStream, IDLE_TIMEOUT, POLL_INTERVAL,
    Receipt, TRANSFER_TIMEOUT, destination, read_offer, receive_file,
};
use crate::{Result, invalid};

pub const SOCKET_NAME: &str = "org.projectseam.android.cable.v1";

const VERSION: [u8; 5] = *b"SEAM\x03";
const DECLINE: [u8; 5] = [0x07, 0, 0, 0, 0];
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy)]
pub struct Offer {
    pub is_pdf: bool,
    pub bytes: Option<u64>,
}

pub struct CableReceiver {
    listener: TcpListener,
    folder: PathBuf,
    port: u16,
}

impl CableReceiver {
    pub fn bind(folder: &Path) -> Result<Self> {
        Self::bind_loopback(folder).map_err(|_| "cable receiver could not start".into())
    }

    fn bind_loopback(folder: &Path) -> io::Result<Self> {
        let folder = destination(folder)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            listener,
            folder,
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn receive_next(
        &self,
        worker_cancel: &AtomicBool,
        connection_cancel: &AtomicBool,
        approve: impl FnMut(Offer) -> bool,
        progress: impl FnMut(u64),
    ) -> Result<Option<Receipt>> {
        let cancelled = Cancellation {
            worker: worker_cancel,
            connection: Some(connection_cancel),
        };
        match self.receive_request(cancelled, approve, progress) {
            Err(_) if cancelled.is_cancelled() => Ok(None),
            Err(_) => Err("cable receive failed".into()),
            received => received,
        }
    }

    fn receive_request(
        &self,
        cancelled: Cancellation<'_>,
        mut approve: impl FnMut(Offer) -> bool,
        progress: impl FnMut(u64),
    ) -> Result<Option<Receipt>> {
        let socket = loop {
            if cancelled.is_cancelled() {
                return Ok(None);
            }
            match self.listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(POLL_INTERVAL);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        };
        socket.set_nonblocking(true)?;
        let now = Instant::now();
        let mut stream = BoundedTcp {
            socket,
            cancelled,
            deadline: now + AUTH_TIMEOUT,
            idle_deadline: now + IDLE_TIMEOUT,
        };

        let mut version = [0; VERSION.len()];
        if stream.read(&mut version[..1])? == 0 {
            return Ok(None);
        }
        stream.read_exact(&mut version[1..])?;
        if version != VERSION {
            return Err(invalid("unsupported cable envelope").into());
        }
        if stream.poll_io(|socket| socket.peek(&mut [0]))? == 0 {
            return Ok(None);
        }
        let offer = read_offer(&mut stream)?;

        let approval_deadline = Instant::now() + APPROVAL_TIMEOUT;
        let approved = approve(offer);
        let now = Instant::now();
        stream.idle_deadline = now + IDLE_TIMEOUT;
        if !approved || now >= approval_deadline {
            stream.deadline = now + AUTH_TIMEOUT;
            let _ = stream.write_all(&DECLINE);
            return Ok(None);
        }
        stream.deadline = now + TRANSFER_TIMEOUT;
        loop {
            stream.check()?;
            match stream.socket.peek(&mut [0]) {
                Ok(0) => return Err(invalid("cable peer disconnected").into()),
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
        }
        stream.write_all(&ACCEPT)?;
        stream.flush()?;
        receive_file(&mut stream, &self.folder, offer, progress).map(Some)
    }
}
