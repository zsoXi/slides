//! Linux directory-relative I/O. Owned directory descriptors pin the destination inode.
use std::ffi::{CString, c_char, c_int};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

const DIRECTORY_FLAGS: c_int = 0x800 | 0x20000 | 0x10000 | 0x80000; // NONBLOCK, NOFOLLOW, DIRECTORY, CLOEXEC
unsafe extern "C" {
    fn openat(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    fn mkdirat(dirfd: c_int, path: *const c_char, mode: u32) -> c_int;
    fn renameat(
        olddirfd: c_int,
        oldpath: *const c_char,
        newdirfd: c_int,
        newpath: *const c_char,
    ) -> c_int;
}
fn cpath(path: &std::ffi::OsStr) -> io::Result<CString> {
    CString::new(path.as_bytes()).map_err(|_| io::ErrorKind::InvalidInput.into())
}

pub(crate) fn parent(base: &Path, relative: &str, create: bool) -> io::Result<(File, PathBuf)> {
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(DIRECTORY_FLAGS)
        .open(base)?;
    let parts: Vec<_> = Path::new(relative)
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => Some(p),
            _ => None, // resolve_path has already rejected absolute and parent components.
        })
        .collect();
    let (leaf, parents) = parts.split_last().ok_or(io::ErrorKind::InvalidInput)?;
    for part in parents {
        let name = cpath(part)?;
        if create {
            // SAFETY: CString is NUL-terminated and live; dirfd is borrowed from an owned File.
            let result = unsafe { mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o777) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
        }
        // SAFETY: read-only openat takes no mode argument; all pointers and dirfd remain live.
        let fd = unsafe { openat(directory.as_raw_fd(), name.as_ptr(), DIRECTORY_FLAGS) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful openat returns a fresh owned descriptor, transferred exactly once.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(leaf);
    Ok((directory, path))
}

pub(crate) fn persist(directory: &File, temporary: &Path, destination: &Path) -> io::Result<()> {
    let from = cpath(temporary.file_name().ok_or(io::ErrorKind::InvalidInput)?)?;
    let to = cpath(destination.file_name().ok_or(io::ErrorKind::InvalidInput)?)?;
    // SAFETY: both names are single components with no NUL, and the same owned directory
    // descriptor stays open across the call. renameat never follows a destination symlink.
    if unsafe {
        renameat(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
