use std::borrow::{Borrow, BorrowMut};
use std::ops::{Deref, DerefMut};
use std::os::unix::io::{AsRawFd, RawFd};

use anyhow::{format_err, Error};
use nix::dir;
use nix::dir::Dir;
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;

use regex::Regex;

#[cfg(feature = "timer")]
use crate::error::SysError;

pub type DirLockGuard = Dir;

/// This wraps nix::dir::Entry with the parent directory's file descriptor.
pub struct ReadDirEntry {
    entry: dir::Entry,
    parent_fd: RawFd,
}

impl From<ReadDirEntry> for dir::Entry {
    fn from(this: ReadDirEntry) -> dir::Entry {
        this.entry
    }
}

impl Deref for ReadDirEntry {
    type Target = dir::Entry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}

impl DerefMut for ReadDirEntry {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry
    }
}

impl AsRef<dir::Entry> for ReadDirEntry {
    fn as_ref(&self) -> &dir::Entry {
        &self.entry
    }
}

impl AsMut<dir::Entry> for ReadDirEntry {
    fn as_mut(&mut self) -> &mut dir::Entry {
        &mut self.entry
    }
}

impl Borrow<dir::Entry> for ReadDirEntry {
    fn borrow(&self) -> &dir::Entry {
        &self.entry
    }
}

impl BorrowMut<dir::Entry> for ReadDirEntry {
    fn borrow_mut(&mut self) -> &mut dir::Entry {
        &mut self.entry
    }
}

impl ReadDirEntry {
    /// Get the parent directory's file descriptor.
    #[inline]
    pub fn parent_fd(&self) -> RawFd {
        self.parent_fd
    }

    /// Get the file name as a `&str`.
    ///
    /// # Safety
    ///
    /// It is up to the user to ensure that the file name is valid utf-8 *before* calling this
    /// method.
    pub unsafe fn file_name_utf8_unchecked(&self) -> &str {
        unsafe { std::str::from_utf8_unchecked(self.file_name().to_bytes()) }
    }
}

// Since Tied<T, U> implements Deref to U, a Tied<Dir, Iterator> already implements Iterator.
// This is simply a wrapper with a shorter type name mapping nix::Error to anyhow::Error.
/// Wrapper over a pair of `nix::dir::Dir` and `nix::dir::Iter`, returned by `read_subdir()`.
pub struct ReadDir {
    iter: nix::dir::OwningIter,
    dir_fd: RawFd,
}

impl Iterator for ReadDir {
    type Item = Result<ReadDirEntry, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|res| {
            res.map(|entry| ReadDirEntry {
                entry,
                parent_fd: self.dir_fd,
            })
            .map_err(Error::from)
        })
    }
}

/// Create an iterator over sub directory entries.
/// This uses `openat` on `dirfd`, so `path` can be relative to that or an absolute path.
pub fn read_subdir<P: ?Sized + nix::NixPath>(dirfd: RawFd, path: &P) -> nix::Result<ReadDir> {
    let dir = Dir::openat(Some(dirfd), path, OFlag::O_RDONLY, Mode::empty())?;
    let fd = dir.as_raw_fd();
    let iter = dir.into_iter();
    Ok(ReadDir { iter, dir_fd: fd })
}

/// Scan through a directory with a regular expression. This is simply a shortcut filtering the
/// results of `read_subdir`. Non-UTF8 compatible file names are silently ignored.
pub fn scan_subdir<'a, P: ?Sized + nix::NixPath>(
    dirfd: RawFd,
    path: &P,
    regex: &'a regex::Regex,
) -> Result<impl Iterator<Item = Result<ReadDirEntry, Error>> + 'a, nix::Error> {
    Ok(read_subdir(dirfd, path)?.filter_file_name_regex(regex))
}

/// Scan directory for matching file names with a callback.
///
/// Scan through all directory entries and call `callback()` function
/// if the entry name matches the regular expression. This function
/// used unix `openat()`, so you can pass absolute or relative file
/// names. This function simply skips non-UTF8 encoded names.
pub fn scandir<P, F>(
    dirfd: RawFd,
    path: &P,
    regex: &regex::Regex,
    mut callback: F,
) -> Result<(), Error>
where
    F: FnMut(RawFd, &str, nix::dir::Type) -> Result<(), Error>,
    P: ?Sized + nix::NixPath,
{
    for entry in scan_subdir(dirfd, path, regex)? {
        let entry = entry?;
        let file_type = match entry.file_type() {
            Some(file_type) => file_type,
            None => get_file_type(entry.parent_fd(), entry.file_name())?,
        };

        callback(
            entry.parent_fd(),
            unsafe { entry.file_name_utf8_unchecked() },
            file_type,
        )?;
    }
    Ok(())
}

/// Helper trait to provide a combinators for directory entry iterators.
pub trait FileIterOps<T, E>
where
    Self: Sized + Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
    E: Into<Error> + Send + Sync,
{
    /// Filter by file type. This is more convenient than using the `filter` method alone as this
    /// also includes error handling and handling of files without a type (via an error).
    fn filter_file_type(self, ty: dir::Type) -> FileTypeFilter<Self, T, E> {
        FileTypeFilter { inner: self, ty }
    }

    /// Filter by file name. Note that file names which aren't valid utf-8 will be treated as if
    /// they do not match the pattern.
    fn filter_file_name_regex(self, regex: &Regex) -> FileNameRegexFilter<Self, T, E> {
        FileNameRegexFilter { inner: self, regex }
    }
}

impl<I, T, E> FileIterOps<T, E> for I
where
    I: Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
    E: Into<Error> + Send + Sync,
{
}

/// This filters files from its inner iterator by a file type. Files with no type produce an error.
pub struct FileTypeFilter<I, T, E>
where
    I: Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
    E: Into<Error> + Send + Sync,
{
    inner: I,
    ty: nix::dir::Type,
}

impl<I, T, E> Iterator for FileTypeFilter<I, T, E>
where
    I: Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
    E: Into<Error> + Send + Sync,
{
    type Item = Result<T, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self.inner.next()?.map_err(|e| e.into());
            match item {
                Ok(ref entry) => match entry.borrow().file_type() {
                    Some(ty) => {
                        if ty == self.ty {
                            return Some(item);
                        } else {
                            continue;
                        }
                    }
                    None => return Some(Err(format_err!("unable to detect file type"))),
                },
                Err(_) => return Some(item),
            }
        }
    }
}

/// This filters files by name via a Regex. Files whose file name aren't valid utf-8 are skipped
/// silently.
pub struct FileNameRegexFilter<'a, I, T, E>
where
    I: Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
{
    inner: I,
    regex: &'a Regex,
}

impl<I, T, E> Iterator for FileNameRegexFilter<'_, I, T, E>
where
    I: Iterator<Item = Result<T, E>>,
    T: Borrow<dir::Entry>,
{
    type Item = Result<T, E>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let item = self.inner.next()?;
            match item {
                Ok(ref entry) => {
                    if let Ok(name) = entry.borrow().file_name().to_str() {
                        if self.regex.is_match(name) {
                            return Some(item);
                        }
                    }
                    // file did not match regex or isn't valid utf-8
                    continue;
                }
                Err(_) => return Some(item),
            }
        }
    }
}

#[cfg(feature = "timer")]
/// Attempt to acquire a shared flock on the given path, 'what' and
/// 'would_block_message' are used for error formatting.
pub fn lock_dir_noblock_shared(
    path: &std::path::Path,
    what: &str,
    would_block_msg: &str,
) -> Result<DirLockGuard, Error> {
    do_lock_dir_noblock(path, what, would_block_msg, false)
}

#[cfg(feature = "timer")]
/// Attempt to acquire an exclusive flock on the given path, 'what' and
/// 'would_block_message' are used for error formatting.
pub fn lock_dir_noblock(
    path: &std::path::Path,
    what: &str,
    would_block_msg: &str,
) -> Result<DirLockGuard, Error> {
    do_lock_dir_noblock(path, what, would_block_msg, true)
}

#[cfg(feature = "timer")]
fn do_lock_dir_noblock(
    path: &std::path::Path,
    what: &str,
    would_block_msg: &str,
    exclusive: bool,
) -> Result<DirLockGuard, Error> {
    let mut handle = Dir::open(path, OFlag::O_RDONLY, Mode::empty()).map_err(|err| {
        format_err!(
            "unable to open {} directory {:?} for locking - {}",
            what,
            path,
            err
        )
    })?;

    // acquire in non-blocking mode, no point in waiting here since other
    // backups could still take a very long time
    crate::fs::lock_file(
        &mut handle,
        exclusive,
        Some(std::time::Duration::from_nanos(0)),
    )
    .map_err(|err| {
        format_err!(
            "unable to acquire lock on {} directory {:?} - {}",
            what,
            path,
            if err.would_block() {
                String::from(would_block_msg)
            } else {
                err.to_string()
            }
        )
    })?;

    Ok(handle)
}

/// extracts [`nix::dir::Type`] from a [`struct@nix::sys::stat::FileStat`] if possible
pub fn file_type_from_file_stat(stat: &nix::sys::stat::FileStat) -> Option<nix::dir::Type> {
    use nix::dir::Type;

    // mask out all unnecessary bits
    Some(match stat.st_mode & libc::S_IFMT {
        libc::S_IFSOCK => Type::Socket,
        libc::S_IFLNK => Type::Symlink,
        libc::S_IFREG => Type::File,
        libc::S_IFBLK => Type::BlockDevice,
        libc::S_IFDIR => Type::Directory,
        libc::S_IFCHR => Type::CharacterDevice,
        libc::S_IFIFO => Type::Fifo,
        _ => return None,
    })
}

/// Returns the file type of the `path` in the `parent_fd`
///
/// calls [`nix::sys::stat::fstatat`] to determine it
pub fn get_file_type<P: ?Sized + nix::NixPath>(
    parent_fd: RawFd,
    path: &P,
) -> Result<nix::dir::Type, Error> {
    let stat = nix::sys::stat::fstatat(
        Some(parent_fd),
        path,
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    )?;
    let file_type =
        file_type_from_file_stat(&stat).ok_or_else(|| format_err!("unable to detect file type"))?;
    Ok(file_type)
}
