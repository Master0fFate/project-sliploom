use std::net::Ipv4Addr;

#[derive(Clone)]
pub struct Peer {
    pub id: [u8; 32],
    pub address: Ipv4Addr,
    pub label: String,
}

pub struct PairPrompt {
    pub code: String,
    pub label: String,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Trust;

#[cfg(not(windows))]
mod unsupported {
    use std::{io, net::Ipv4Addr, path::Path, sync::atomic::AtomicBool};

    use super::{PairPrompt, Peer};
    use crate::{
        Result,
        lan::{Receipt, sender::SendInvite},
    };

    #[derive(Clone)]
    pub struct Trust(());

    fn unavailable<T>() -> Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "trusted Wi-Fi is unavailable on this platform",
        )
        .into())
    }

    impl Trust {
        pub fn open(_: &Path) -> Result<Self> {
            unavailable()
        }

        pub fn arm_pair(&self) -> Result<String> {
            unavailable()
        }

        pub fn cancel_pair(&self) {}

        pub fn peers(&self) -> Vec<Peer> {
            Vec::new()
        }

        pub fn forget(&self, _: [u8; 32]) -> Result<()> {
            unavailable()
        }

        pub fn serve(
            &self,
            _: Ipv4Addr,
            _: &Path,
            _: &AtomicBool,
            _: impl Fn(PairPrompt) -> bool,
            _: impl Fn(Receipt),
        ) -> Result<()> {
            unavailable()
        }

        pub fn pair(
            &self,
            _: &str,
            _: &AtomicBool,
            _: impl FnOnce(PairPrompt) -> bool,
        ) -> Result<Peer> {
            unavailable()
        }

        pub fn ping(&self, _: &Peer, _: &AtomicBool) -> bool {
            false
        }

        pub fn open_receive(&self, _: &Peer, _: &AtomicBool) -> Result<SendInvite> {
            unavailable()
        }
    }
}

#[cfg(not(windows))]
pub use unsupported::Trust;
