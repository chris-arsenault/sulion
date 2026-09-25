//! Directory-relative installation. Every untrusted path component is opened
//! without following symlinks; downloads never write through the final filename.
use std::{
    ffi::CString,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
};
use uuid::Uuid;

pub struct Directory(File);

fn cstr(value: &[u8]) -> io::Result<CString> {
    CString::new(value).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid path"))
}
fn open_at(parent: i32, name: &CString, flags: i32, mode: libc::mode_t) -> io::Result<File> {
    // The CString and directory descriptor remain alive during openat. A
    // successful descriptor is immediately owned by File.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

impl Directory {
    pub fn root(root: &Path) -> io::Result<Self> {
        Ok(Self(open_at(
            libc::AT_FDCWD,
            &cstr(root.as_os_str().as_bytes())?,
            libc::O_RDONLY | libc::O_DIRECTORY,
            0,
        )?))
    }

    pub fn identity(&self) -> io::Result<String> {
        let meta = self.0.metadata()?;
        Ok(format!("{}:{}", meta.dev(), meta.ino()))
    }

    pub fn descend(&self, path: &str, create: bool) -> io::Result<Self> {
        let mut current = Self(self.0.try_clone()?);
        if path.is_empty() {
            return Ok(current);
        }
        for part in path.split('/') {
            if !super::model::valid_component(part) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid directory",
                ));
            }
            let name = cstr(part.as_bytes())?;
            if create {
                let result = unsafe { libc::mkdirat(current.0.as_raw_fd(), name.as_ptr(), 0o755) };
                if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                    return Err(io::Error::last_os_error());
                }
                if result == 0 {
                    current.0.sync_all()?;
                }
            }
            current = Self(open_at(
                current.0.as_raw_fd(),
                &name,
                libc::O_RDONLY | libc::O_DIRECTORY,
                0,
            )?);
        }
        Ok(current)
    }

    pub fn temporary(self) -> io::Result<Temporary> {
        let name = cstr(format!(".sulion-upload-{}", Uuid::new_v4()).as_bytes())?;
        let file = open_at(
            self.0.as_raw_fd(),
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )?;
        Ok(Temporary {
            directory: self,
            name,
            file: Some(file),
        })
    }
}

pub struct Temporary {
    pub directory: Directory,
    name: CString,
    file: Option<File>,
}
impl Temporary {
    pub fn take_file(&mut self) -> File {
        self.file.take().expect("temporary file taken once")
    }
    pub fn install(&self, filename: &str) -> io::Result<()> {
        if !super::model::valid_component(filename) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid filename",
            ));
        }
        let target = cstr(filename.as_bytes())?;
        // renameat replaces a symlink itself, never its target. Both names are
        // relative to the already opened destination directory.
        let result = unsafe {
            libc::renameat(
                self.directory.0.as_raw_fd(),
                self.name.as_ptr(),
                self.directory.0.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        self.directory.0.sync_all()
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        unsafe {
            libc::unlinkat(self.directory.0.as_raw_fd(), self.name.as_ptr(), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn symlinks_cannot_redirect_installation_and_bytes_become_visible_atomically() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        let dir = Directory::root(root.path()).unwrap();
        assert!(dir.descend("escape", true).is_err());
        assert!(dir.descend("../escape", true).is_err());
        std::fs::write(outside.path().join("target"), b"outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("target"), root.path().join("target"))
            .unwrap();
        let mut temp = dir.temporary().unwrap();
        let mut file = temp.take_file();
        file.write_all(b"new bytes").unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            std::fs::read(root.path().join("target")).unwrap(),
            b"outside"
        );
        temp.install("target").unwrap();
        assert_eq!(
            std::fs::read(root.path().join("target")).unwrap(),
            b"new bytes"
        );
        assert_eq!(
            std::fs::read(outside.path().join("target")).unwrap(),
            b"outside"
        );
    }
}
