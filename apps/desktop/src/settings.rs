#[cfg(unix)]
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

const RECEIVE_FOLDER: &str = "receive-folder";
const MAX_PATH_BYTES: usize = 128 * 1024;

#[cfg(target_os = "windows")]
pub(crate) const UNAVAILABLE: &str =
    "Saved folder unavailable. Using the default if available. Choose a save folder again.";
#[cfg(unix)]
pub(crate) const UNAVAILABLE: &str = "The saved folder is unavailable. Choose a save folder again.";
pub(crate) const NOT_REMEMBERED: &str =
    "Couldn’t remember this save folder. It’ll be used for this run only.";

#[cfg(target_os = "windows")]
fn with_directory<T>(create: bool, action: impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
    let directory = crate::cable_ui::open_settings_directory(create)?;
    action(&directory.path)
}

#[cfg(unix)]
fn settings_directory() -> Option<PathBuf> {
    fn absolute_env(name: &str) -> Option<PathBuf> {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    #[cfg(target_os = "macos")]
    return absolute_env("HOME").map(|home| home.join("Library/Application Support/Sliploom"));
    #[cfg(not(target_os = "macos"))]
    return absolute_env("XDG_CONFIG_HOME")
        .or_else(|| absolute_env("HOME").map(|home| home.join(".config")))
        .map(|base| base.join("sliploom"));
}

#[cfg(unix)]
fn with_directory<T>(create: bool, action: impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
    let directory =
        settings_directory().ok_or_else(|| io::Error::other("no absolute settings directory"))?;
    if create {
        fs::create_dir_all(&directory)?;
    }
    action(&directory)
}

pub(crate) fn validate(path: PathBuf) -> io::Result<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        crate::cable_ui::open_destination(Some(&path), false).map(|folder| folder.path)
    }
    #[cfg(unix)]
    {
        use std::path::Component;

        let folder: PathBuf = path.components().collect();
        if !folder.is_absolute()
            || !matches!(folder.components().next_back(), Some(Component::Normal(_)))
            || !fs::symlink_metadata(&folder)?.is_dir()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid save folder",
            ));
        }
        let canonical = fs::canonicalize(&folder)?;
        if is_settings_destination(&canonical) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved settings folder",
            ));
        }
        Ok(path)
    }
}

#[cfg(unix)]
fn is_settings_destination(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Some(settings) = settings_directory().and_then(|directory| fs::metadata(directory).ok())
    else {
        return false;
    };
    path.ancestors().any(|ancestor| {
        fs::metadata(ancestor).is_ok_and(|metadata| {
            metadata.dev() == settings.dev() && metadata.ino() == settings.ino()
        })
    })
}

pub(crate) fn load() -> io::Result<Option<PathBuf>> {
    let path = match with_directory(false, |directory| {
        read_path(&directory.join(RECEIVE_FOLDER))
    }) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    validate(path).map(Some)
}

fn read_path(path: &Path) -> io::Result<PathBuf> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    let regular = metadata.is_file();
    #[cfg(target_os = "windows")]
    let regular = {
        use std::os::windows::fs::MetadataExt;
        use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        regular && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
    };
    if !regular || metadata.len() > MAX_PATH_BYTES as u64 {
        return Err(invalid_preference());
    }
    let mut text = String::new();
    file.take(MAX_PATH_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    if text.is_empty() || text.len() > MAX_PATH_BYTES || text.contains('\0') {
        return Err(invalid_preference());
    }
    Ok(PathBuf::from(text))
}

pub(crate) fn save(path: &Path) -> io::Result<()> {
    let text = path.to_str().ok_or_else(invalid_preference)?;
    if text.len() > MAX_PATH_BYTES {
        return Err(invalid_preference());
    }
    with_directory(true, |directory| {
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(text.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(directory.join(RECEIVE_FOLDER))
            .map_err(|error| error.error)?;
        Ok(())
    })
}

fn invalid_preference() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid receive folder preference",
    )
}
