use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const REMOTE: &str = "localabstract:org.projectseam.android.cable.v1";
const COMMAND_TIMEOUT: Duration = Duration::from_millis(750);
const PROBE_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_OUTPUT: usize = 16 * 1024;
const ADB: &str = if cfg!(windows) { "adb.exe" } else { "adb" };
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub(crate) struct AdbBridge {
    adb: PathBuf,
    peer: UsbPeer,
    target: String,
}

#[derive(PartialEq, Eq)]
struct UsbPeer {
    serial: String,
    transport_id: u64,
}

impl AdbBridge {
    pub(crate) fn open(port: u16) -> io::Result<Self> {
        if port == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid cable port",
            ));
        }
        let adb = locate_adb()?;
        let peer = UsbPeer::selected(&adb)?;
        let target = format!("tcp:{port}");
        let routes = peer.run(&adb, &["reverse", "--list"])?;
        if route_target(&routes)?.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "ADB cable route already exists",
            ));
        }
        if UsbPeer::selected(&adb)? != peer {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "ADB USB peer changed",
            ));
        }
        peer.run(&adb, &["reverse", "--no-rebind", REMOTE, &target])?;
        Ok(Self { adb, peer, target })
    }

    pub(crate) fn connected(&self) -> bool {
        if !matches!(UsbPeer::selected(&self.adb), Ok(peer) if peer == self.peer) {
            return false;
        }
        self.peer
            .run(&self.adb, &["reverse", "--list"])
            .is_ok_and(|routes| route_target(&routes).ok().flatten() == Some(self.target.as_str()))
    }
}

impl Drop for AdbBridge {
    fn drop(&mut self) {
        if self.connected() {
            let _ = self.peer.run(&self.adb, &["reverse", "--remove", REMOTE]);
        }
    }
}

impl UsbPeer {
    fn selected(adb: &Path) -> io::Result<Self> {
        let unavailable =
            || io::Error::new(io::ErrorKind::NotConnected, "no unique ready ADB USB peer");
        let serial = run(adb, &["-d", "get-serialno"])?;
        let serial = serial.trim();
        if serial.is_empty() || serial == "unknown" || serial.split_ascii_whitespace().count() != 1
        {
            return Err(unavailable());
        }
        let devices = run(adb, &["devices", "-l"])?;
        let mut transport_id = None;
        for line in devices.lines() {
            let mut fields = line.split_ascii_whitespace();
            if fields.next() != Some(serial) {
                continue;
            }
            if transport_id.is_some() || fields.next() != Some("device") {
                return Err(unavailable());
            }
            let mut ids = fields.filter_map(|field| field.strip_prefix("transport_id:"));
            transport_id = ids
                .next()
                .and_then(|id| id.parse::<u64>().ok())
                .filter(|id| *id != 0);
            if transport_id.is_none() || ids.next().is_some() {
                return Err(unavailable());
            }
        }
        Ok(Self {
            serial: serial.to_owned(),
            transport_id: transport_id.ok_or_else(unavailable)?,
        })
    }

    fn run(&self, adb: &Path, args: &[&str]) -> io::Result<String> {
        let id = self.transport_id.to_string();
        let mut command = vec!["-t", &id];
        command.extend_from_slice(args);
        run(adb, &command)
    }
}

fn route_target(routes: &str) -> io::Result<Option<&str>> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid ADB reverse list");
    let mut target = None;
    for line in routes.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(_transport) = fields.next() else {
            continue;
        };
        let name = fields.next().ok_or_else(invalid)?;
        let endpoint = fields.next().ok_or_else(invalid)?;
        if fields.next().is_some() {
            return Err(invalid());
        }
        if name == REMOTE && target.replace(endpoint).is_some() {
            return Err(invalid());
        }
    }
    Ok(target)
}

fn locate_adb() -> io::Result<PathBuf> {
    let cwd = env::current_dir().and_then(fs::canonicalize).ok();
    let find = |path: PathBuf| {
        if !path.is_absolute() {
            return None;
        }
        let path = fs::canonicalize(path).ok()?;
        if path.parent() == cwd.as_deref() || !path.is_file() {
            return None;
        }
        #[cfg(unix)]
        if fs::metadata(&path).ok()?.permissions().mode() & 0o111 == 0 {
            return None;
        }
        Some(path)
    };
    for name in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = env::var_os(name)
            && let Some(adb) = find(PathBuf::from(sdk).join("platform-tools").join(ADB))
        {
            return Ok(adb);
        }
    }
    if let (Some(_), Some(path)) = (cwd.as_ref(), env::var_os("PATH"))
        && let Some(adb) = env::split_paths(&path).find_map(|dir| find(dir.join(ADB)))
    {
        return Ok(adb);
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        let (sdk, bins) = (
            "Library/Android/sdk",
            ["/opt/homebrew/bin", "/usr/local/bin"],
        );
        #[cfg(target_os = "linux")]
        let (sdk, bins) = ("Android/Sdk", ["/usr/local/bin", "/usr/bin"]);
        if let Some(home) = env::var_os("HOME")
            && let Some(adb) = find(
                PathBuf::from(home)
                    .join(sdk)
                    .join("platform-tools")
                    .join(ADB),
            )
        {
            return Ok(adb);
        }
        if let Some(adb) = bins
            .into_iter()
            .find_map(|dir| find(Path::new(dir).join(ADB)))
        {
            return Ok(adb);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "ADB platform-tools not found",
    ))
}

fn local_server() -> io::Result<()> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 5037));
    let mut conn = TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)?;
    conn.set_read_timeout(Some(PROBE_TIMEOUT))?;
    conn.set_write_timeout(Some(PROBE_TIMEOUT))?;
    conn.write_all(b"000Chost:version")?;
    let mut reply = [0; 12];
    conn.read_exact(&mut reply)?;
    if &reply != b"OKAY00040029" {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "incompatible local ADB server",
        ));
    }
    Ok(())
}

fn run(adb: &Path, args: &[&str]) -> io::Result<String> {
    local_server()
        .map_err(|err| io::Error::new(err.kind(), "a compatible local ADB server is required"))?;
    let mut command = Command::new(adb);
    command
        .current_dir(
            adb.parent()
                .ok_or_else(|| io::Error::other("invalid ADB location"))?,
        )
        .args(["-H", "127.0.0.1", "-P", "5037"])
        .args(args)
        .env_remove("ADB_SERVER_SOCKET")
        .env_remove("ANDROID_ADB_SERVER_ADDRESS")
        .env_remove("ANDROID_ADB_SERVER_PORT")
        .env_remove("ANDROID_SERIAL")
        .env_remove("ADB_TRACE")
        .env_remove("ADB_VENDOR_KEYS")
        .env("ADB_MDNS", "0")
        .env("ADB_MDNS_AUTO_CONNECT", "")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let mut child = command
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), "could not start ADB"))?;
    let stdout = child.stdout.take().expect("piped ADB stdout");
    let (tx, rx) = mpsc::sync_channel(1);
    let outcome = (|| {
        thread::Builder::new().spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut bytes);
            let _ = tx.send(result.map(|_| bytes));
        })?;
        let timed_out = || io::Error::new(io::ErrorKind::TimedOut, "ADB command timed out");
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(timed_out());
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(io::Error::other("ADB command failed"));
                }
                let bytes = rx.recv_timeout(remaining).map_err(|_| timed_out())??;
                if bytes.len() > MAX_OUTPUT {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "oversized ADB response",
                    ));
                }
                return String::from_utf8(bytes).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid ADB response")
                });
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    })();
    if outcome.is_err() {
        let _ = child.kill();
        let _ = child.try_wait();
    }
    outcome
}
