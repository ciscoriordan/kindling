//! Write a book's cover thumbnail onto a mounted Kindle.
//!
//! ## Why this exists
//!
//! A sideloaded book gets a lock screen cover only if it carries EXTH 113 and
//! EXTH 501 (see `--doc-type ebok` and issue #26). But those same records make
//! the firmware treat 113 as a real ASIN: it asks the Amazon store, finds
//! nothing for a sideloaded UUID, and caches an Amazon "No image available"
//! image at `system/thumbnails/thumbnail_<113>_<501>_portrait.jpg`. That
//! cached file becomes the library tile, so opting into a lock screen cover
//! costs you the shelf art.
//!
//! Overwriting that file with the book's own thumbnail fixes the tile and
//! leaves the lock screen cover working, both verified on a Paperwhite 5
//! running 5.19.2. This is the same move calibre makes, and its
//! `amazon-cover-bug/` directory is a backup copy for restoring the file after
//! the firmware clobbers it again on a later sync, which is why this writes
//! one too.
//!
//! Nothing here can be done from inside the book: the filename lives on the
//! device, so it takes a mounted volume and an explicit command.

use crate::mobi_rewrite::{self, RewriteError};
use std::path::{Path, PathBuf};

/// Where the thumbnail landed, for reporting back to the user.
pub struct Installed {
    pub thumbnail: PathBuf,
    pub backup: PathBuf,
    /// True when a file was already there and got replaced. Almost always the
    /// Amazon placeholder, which is the entire point of the command.
    pub replaced: bool,
    /// Byte size of the image written.
    pub bytes: usize,
}

/// Errors specific to installing onto a device, as opposed to parsing a book.
pub enum InstallError {
    Book(RewriteError),
    Io(std::io::Error),
    /// `--kindle` did not point at something that looks like a Kindle volume.
    NotAKindle(PathBuf),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Book(e) => write!(f, "{e}"),
            InstallError::Io(e) => write!(f, "I/O error: {e}"),
            InstallError::NotAKindle(p) => write!(
                f,
                "{} does not look like a mounted Kindle (no system/ or documents/ directory). \
                 Pass the volume root, e.g. --kindle /Volumes/Kindle",
                p.display()
            ),
        }
    }
}

impl From<RewriteError> for InstallError {
    fn from(e: RewriteError) -> Self {
        InstallError::Book(e)
    }
}

impl From<std::io::Error> for InstallError {
    fn from(e: std::io::Error) -> Self {
        InstallError::Io(e)
    }
}

/// Directory the firmware reads device-side cover thumbnails from.
const THUMBNAIL_DIR: &str = "system/thumbnails";

/// Calibre's restore directory. Keeping a copy here matches what calibre does
/// and gives the user something to put back after a sync overwrites the real
/// file with the store placeholder again.
const BACKUP_DIR: &str = "amazon-cover-bug";

/// Extract `input`'s thumbnail and install it on the Kindle mounted at
/// `kindle_root`.
pub fn install(input: &Path, kindle_root: &Path) -> Result<Installed, InstallError> {
    // Refuse a path that is not a Kindle volume root rather than scattering
    // directories somewhere the user did not mean. Both markers exist on every
    // Kindle that presents USB mass storage.
    if !kindle_root.join("system").is_dir() && !kindle_root.join("documents").is_dir() {
        return Err(InstallError::NotAKindle(kindle_root.to_path_buf()));
    }

    let thumb = mobi_rewrite::extract_device_thumbnail(input)?;
    let name = format!("thumbnail_{}_{}_portrait.jpg", thumb.asin, thumb.cde_type);

    let thumb_dir = kindle_root.join(THUMBNAIL_DIR);
    std::fs::create_dir_all(&thumb_dir)?;
    let target = thumb_dir.join(&name);
    let replaced = target.exists();
    std::fs::write(&target, &thumb.image)?;

    let backup_dir = kindle_root.join(BACKUP_DIR);
    std::fs::create_dir_all(&backup_dir)?;
    let backup = backup_dir.join(&name);
    std::fs::write(&backup, &thumb.image)?;

    Ok(Installed {
        thumbnail: target,
        backup,
        replaced,
        bytes: thumb.image.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kindling_thumbnail_test_{}_{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn rejects_a_path_that_is_not_a_kindle() {
        // Writing system/thumbnails/ into an arbitrary directory because the
        // user fat-fingered --kindle is worse than refusing.
        let dir = tmp_dir("not_a_kindle");
        let book = dir.join("book.azw3");
        std::fs::write(&book, b"irrelevant").unwrap();
        match install(&book, &dir) {
            Err(InstallError::NotAKindle(p)) => assert_eq!(p, dir),
            _ => panic!("a directory with no system/ or documents/ must be refused"),
        }
        assert!(
            !dir.join("system").exists(),
            "must not create directories on a path it rejected"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_a_book_with_no_device_cover_key() {
        // Without EXTH 113/501 the firmware has no filename to look under, so
        // there is nothing to install and saying so beats writing a file the
        // device will never read.
        let dir = tmp_dir("no_key");
        std::fs::create_dir_all(dir.join("documents")).unwrap();
        let book = dir.join("book.azw3");
        std::fs::write(&book, b"not a mobi at all").unwrap();
        assert!(
            install(&book, &dir).is_err(),
            "a file with no 113/501 pair must not install"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
