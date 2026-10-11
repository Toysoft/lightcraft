//! The system Trash: where `photo.trashFiles` moves a photo's file, so that taking a photo out
//! of the library for good can take its file along without destroying it. Nothing else in
//! LightCraft removes a photo's file.
//!
//! [`Bin`] is the Trash as the engine uses it; [`SystemBin`] is the real one:
//! - macOS: the Finder's Trash (`NSFileManager`), which says where the file went, so undo can
//!   put it back.
//! - Linux and the other Unixes: the freedesktop.org Trash ([`xdg`]), likewise.
//! - Windows and the web: not yet. Asking is refused, and no file is touched.
//!
//! A disk without a Trash (many network shares) answers [`BinError::NoTrash`]: the file stays,
//! and only an explicit second question may erase it ([`Bin::erase`]).

use std::path::{Path, PathBuf};

/// Why a file did not go to the Trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinError {
    /// The disk the file is on has no Trash: it can only be erased, which is asked separately.
    NoTrash,
    /// Anything else, as a sentence for the person (the file is where it was).
    Failed(String),
}

/// A Trash. Implementations move, never copy-and-forget: after `trash` returned `Ok` the file is
/// in the Trash and not at `path`; after an error it is at `path`.
pub trait Bin: Send + Sync {
    /// Move the file at `path` to the Trash. `Ok(Some(at))`: where it is now, when the Trash
    /// tells (then [`Bin::put_back`] can undo it); `Ok(None)`: in the Trash, somewhere.
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, BinError>;

    /// Move a trashed file from `at` back to `to`, never replacing what is at `to` now.
    fn put_back(&self, at: &Path, to: &Path) -> std::io::Result<()> {
        use crate::rename::MoveFs;
        if let Some(dir) = to.parent() {
            // the folder may have been removed since, when it was left empty
            std::fs::create_dir_all(dir)?;
        }
        crate::rename::RealFs.rename_no_replace(at, to)
    }

    /// Erase the file at `path` for good: only for a file on a disk without a Trash, after the
    /// person was asked about exactly that.
    fn erase(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }
}

/// The system's Trash.
pub struct SystemBin;

impl Bin for SystemBin {
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, BinError> {
        system_trash(path)
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn put_back(&self, at: &Path, to: &Path) -> std::io::Result<()> {
        use crate::rename::MoveFs;
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::rename::RealFs.rename_no_replace(at, to)?;
        // the Trash's note about it goes with it
        if let Some(info) = xdg::info_of(at) {
            let _ = std::fs::remove_file(info);
        }
        Ok(())
    }
}

/// A file a step moved out of its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// Where it was.
    pub from: String,
    pub now: Whereabouts,
}

/// What became of a file `photo.trashFiles` took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Whereabouts {
    /// In the Trash, at this path: undo moves it back.
    At(String),
    /// In the Trash, the system did not say where: the person puts it back from there.
    InTrash,
    /// Erased (a disk without a Trash): it is not coming back.
    Erased,
}

/// The file work of an undo or redo step of `photo.trashFiles`, done before its catalog op.
#[derive(Clone, Debug)]
pub(crate) enum TrashStep {
    /// Undo: put these files back where they were.
    PutBack(Vec<Trashed>),
    /// Redo: move these files (the ones undo put back) to the Trash again.
    Again(Vec<String>),
}

/// "1 file" / "3 files".
pub(crate) fn files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

impl TrashStep {
    /// Do the step's file work with `bin`. Returns the step that reverses it and, when not
    /// everything could be done, a sentence saying what was not. It never fails as a whole: the
    /// catalog step goes ahead, and a photo whose file did not come back shows as missing.
    pub(crate) fn run(&self, bin: &dyn Bin) -> (TrashStep, Option<String>) {
        match self {
            TrashStep::PutBack(list) => {
                let (mut back, mut failed, mut elsewhere, mut erased) = (Vec::new(), 0usize, 0usize, 0usize);
                for t in list {
                    match &t.now {
                        Whereabouts::At(at) => match bin.put_back(Path::new(at), Path::new(&t.from)) {
                            Ok(()) => back.push(t.from.clone()),
                            Err(_) => failed += 1,
                        },
                        Whereabouts::InTrash => elsewhere += 1,
                        Whereabouts::Erased => erased += 1,
                    }
                }
                let mut notes = Vec::new();
                if failed > 0 {
                    let them = if failed == 1 {
                        "it is no longer in the Trash, or something else is in its place"
                    } else {
                        "they are no longer in the Trash, or something else is in their place"
                    };
                    notes.push(format!("{} could not be put back: {them}", files(failed)));
                }
                if elsewhere > 0 {
                    notes.push(format!(
                        "{} still in the Trash: put {} back from there",
                        if elsewhere == 1 { "1 file is".to_string() } else { format!("{elsewhere} files are") },
                        if elsewhere == 1 { "it" } else { "them" }
                    ));
                }
                if erased > 0 {
                    notes.push(format!(
                        "{} erased and {} not coming back",
                        if erased == 1 { "1 file was".to_string() } else { format!("{erased} files were") },
                        if erased == 1 { "is" } else { "are" }
                    ));
                }
                (TrashStep::Again(back), (!notes.is_empty()).then(|| notes.join("; ")))
            }
            TrashStep::Again(paths) => {
                let (mut list, mut failed) = (Vec::new(), 0usize);
                for from in paths {
                    match bin.trash(Path::new(from)) {
                        Ok(at) => list.push(Trashed {
                            from: from.clone(),
                            now: at.map_or(Whereabouts::InTrash, |p| Whereabouts::At(p.to_string_lossy().to_string())),
                        }),
                        // (never erased again without being asked: the file stays)
                        Err(_) => failed += 1,
                    }
                }
                let note = (failed > 0).then(|| {
                    format!(
                        "{} could not be moved to the Trash and {} where {} was",
                        files(failed),
                        if failed == 1 { "stays" } else { "stay" },
                        if failed == 1 { "it" } else { "they" }
                    )
                });
                (TrashStep::PutBack(list), note)
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn system_trash(path: &Path) -> Result<Option<PathBuf>, BinError> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    /// `NSFeatureUnsupportedError`: what a volume without a Trash answers.
    const FEATURE_UNSUPPORTED: isize = 3328;
    let Some(text) = path.to_str() else { return Err(BinError::Failed(format!("{}: not a name the Trash can take", path.display()))) };
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    let mut at = None;
    match NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, Some(&mut at)) {
        Ok(()) => Ok(at.and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))),
        Err(e) if e.code() == FEATURE_UNSUPPORTED => Err(BinError::NoTrash),
        Err(e) => Err(BinError::Failed(e.localizedDescription().to_string())),
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
fn system_trash(path: &Path) -> Result<Option<PathBuf>, BinError> {
    xdg::trash(path, &xdg::Places::of_this_user()).map(Some)
}

#[cfg(not(any(unix, target_arch = "wasm32")))]
fn system_trash(_path: &Path) -> Result<Option<PathBuf>, BinError> {
    Err(BinError::Failed("moving files to the Recycle Bin is not available on Windows yet: nothing was changed".into()))
}

#[cfg(target_arch = "wasm32")]
fn system_trash(_path: &Path) -> Result<Option<PathBuf>, BinError> {
    Err(BinError::Failed("there is no Trash in the browser: nothing was changed".into()))
}

/// The freedesktop.org Trash (specification 1.0), in plain file operations: a file goes to
/// `<trash>/files/<name>` beside a `<trash>/info/<name>.trashinfo` that records where it was and
/// when, which is what file managers read to list it and put it back.
///
/// The home Trash (`$XDG_DATA_HOME/Trash`) takes files of the disk it is on. A file on another
/// disk goes to that disk's own `.Trash-<uid>` at its top, made when missing; where that can't
/// be made (a read-only or foreign disk), the disk has no Trash.
#[cfg(unix)]
pub mod xdg {
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use super::BinError;

    /// Where this user's Trash is, and who they are.
    #[derive(Clone, Debug)]
    pub struct Places {
        /// `$XDG_DATA_HOME/Trash` (`~/.local/share/Trash`).
        pub home_trash: PathBuf,
        pub uid: u32,
        /// The time written to `DeletionDate` (`YYYY-MM-DDThh:mm:ss`, local time by the
        /// specification; UTC here, as there is no time zone to ask).
        pub now: String,
    }

    impl Places {
        pub fn of_this_user() -> Places {
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share"));
            // (the owner of the home folder: std has no getuid)
            let uid = std::fs::metadata(&home).map(|m| m.uid()).unwrap_or(0);
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let day = secs.rem_euclid(86_400);
            let now = format!("{}T{:02}:{:02}:{:02}", lightcraft_catalog::dates::civil(secs), day / 3600, day % 3600 / 60, day % 60);
            Places { home_trash: data.join("Trash"), uid, now }
        }
    }

    /// The device a path is on (`None`: it can't be asked).
    fn device(p: &Path) -> Option<u64> {
        std::fs::symlink_metadata(p).ok().map(|m| m.dev())
    }

    /// The top folder of the disk `dir` is on: the highest folder above it still on that device.
    fn top_of_disk(dir: &Path) -> Option<PathBuf> {
        let dev = device(dir)?;
        let mut top = dir.to_path_buf();
        // (bounded: a path has no more levels than this)
        for _ in 0..4096 {
            match top.parent() {
                Some(up) if device(up) == Some(dev) => top = up.to_path_buf(),
                _ => break,
            }
        }
        Some(top)
    }

    /// A path as `Path=` wants it: percent-encoded, `/` kept.
    fn encoded(p: &Path) -> String {
        use std::os::unix::ffi::OsStrExt;
        let mut out = String::new();
        for b in p.as_os_str().as_bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(*b as char),
                b => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    /// Move `path` to the Trash; returns where it is now.
    pub fn trash(path: &Path, places: &Places) -> Result<PathBuf, BinError> {
        let failed = |what: &str, e: std::io::Error| BinError::Failed(format!("{}: {what}: {e}", path.display()));
        let path = std::path::absolute(path).map_err(|e| failed("not a path", e))?;
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(BinError::Failed(format!("{}: not a file", path.display())));
        };
        let Some(dev) = device(dir) else { return Err(BinError::Failed(format!("{}: its folder can't be read", path.display()))) };
        // the home Trash for files of its disk, else the disk's own
        let home_dev = std::fs::create_dir_all(&places.home_trash).ok().and_then(|()| device(&places.home_trash));
        let (bin, relative_to) = if home_dev == Some(dev) {
            (places.home_trash.clone(), None)
        } else {
            let Some(top) = top_of_disk(dir) else { return Err(BinError::NoTrash) };
            (top.join(format!(".Trash-{}", places.uid)), Some(top))
        };
        let (files, info) = (bin.join("files"), bin.join("info"));
        if std::fs::create_dir_all(&files).is_err() || std::fs::create_dir_all(&info).is_err() {
            return Err(BinError::NoTrash);
        }
        // in a disk's own Trash the path is written from the disk's top
        let original = relative_to.as_deref().and_then(|top| path.strip_prefix(top).ok()).unwrap_or(&path);
        let record = format!("[Trash Info]\nPath={}\nDeletionDate={}\n", encoded(original), places.now);
        // a name of its own: the info file is made first and never replaces one (the
        // specification's way of reserving the name)
        let base = name.to_string_lossy().to_string();
        for n in 0..10_000u32 {
            let unique = if n == 0 { base.clone() } else { format!("{base}.{n}") };
            let (info_file, to) = (info.join(format!("{unique}.trashinfo")), files.join(&unique));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&info_file) {
                Ok(mut f) => {
                    use std::io::Write;
                    let moved = f.write_all(record.as_bytes()).and_then(|()| f.sync_all()).and_then(|()| {
                        if std::fs::symlink_metadata(&to).is_ok() {
                            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "the Trash has a file of that name"));
                        }
                        std::fs::rename(&path, &to)
                    });
                    return match moved {
                        Ok(()) => Ok(to),
                        Err(e) => {
                            // nothing went: the reservation goes too
                            let _ = std::fs::remove_file(&info_file);
                            Err(failed("can't be moved to the Trash", e))
                        }
                    };
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(BinError::NoTrash),
            }
        }
        Err(BinError::Failed(format!("{}: the Trash holds too many files of that name", path.display())))
    }

    /// The `.trashinfo` of a file in a Trash (`<trash>/files/<name>`), to remove once the file
    /// was put back.
    pub fn info_of(at: &Path) -> Option<PathBuf> {
        let (files, name) = (at.parent()?, at.file_name()?);
        (files.file_name()? == "files").then(|| files.parent().map(|bin| bin.join("info").join(format!("{}.trashinfo", name.to_string_lossy()))))?
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::xdg::{Places, info_of, trash};
    use super::*;

    /// A scratch folder that goes away with the test.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(tag: &str) -> (Scratch, Places) {
        let dir = std::env::temp_dir().join(format!("lc-xdg-trash-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        let places = Places { home_trash: dir.join("data/Trash"), uid: 1000, now: "2026-10-11T12:00:00".into() };
        (Scratch(dir), places)
    }

    /// A file goes to `files/`, with an info file that says where it was and when; a second file
    /// of the same name gets a name of its own, and nothing in the Trash is replaced.
    #[test]
    fn a_file_goes_to_the_freedesktop_trash_with_its_info() {
        let (dir, places) = scratch("basic");
        let photo = dir.0.join("photos/my photo é.jpg");
        std::fs::write(&photo, "one").unwrap();
        let at = trash(&photo, &places).unwrap();
        assert_eq!(at, places.home_trash.join("files/my photo é.jpg"));
        assert!(!photo.exists());
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "one");
        let info = std::fs::read_to_string(info_of(&at).unwrap()).unwrap();
        let mut lines = info.lines();
        assert_eq!(lines.next(), Some("[Trash Info]"));
        let path = lines.next().unwrap();
        assert!(path.starts_with("Path=/") && path.ends_with("/photos/my%20photo%20%C3%A9.jpg"), "{path}");
        assert_eq!(lines.next(), Some("DeletionDate=2026-10-11T12:00:00"));
        // the same name again
        std::fs::write(&photo, "two").unwrap();
        let second = trash(&photo, &places).unwrap();
        assert_eq!(second, places.home_trash.join("files/my photo é.jpg.1"));
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "one", "the first is untouched");
        assert!(info_of(&second).unwrap().exists());
    }

    /// A file that is not there is not trashed, and leaves nothing behind in the Trash.
    #[test]
    fn a_file_that_can_t_be_moved_leaves_no_info_behind() {
        let (dir, places) = scratch("gone");
        let e = trash(&dir.0.join("photos/none.jpg"), &places).unwrap_err();
        assert!(matches!(e, BinError::Failed(_)), "{e:?}");
        let left: Vec<_> = std::fs::read_dir(places.home_trash.join("info")).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
    }

    /// Putting a file back never replaces what is at its place now.
    #[test]
    fn putting_back_never_replaces() {
        let (dir, places) = scratch("back");
        let photo = dir.0.join("photos/a.jpg");
        std::fs::write(&photo, "old").unwrap();
        let at = trash(&photo, &places).unwrap();
        std::fs::write(&photo, "new").unwrap();
        assert!(SystemBin.put_back(&at, &photo).is_err());
        assert_eq!(std::fs::read_to_string(&photo).unwrap(), "new");
        std::fs::remove_file(&photo).unwrap();
        SystemBin.put_back(&at, &photo).unwrap();
        assert_eq!(std::fs::read_to_string(&photo).unwrap(), "old");
    }

    /// The real Trash of this Mac (run by hand: `cargo test -p lightcraft-engine -- --ignored
    /// the_system_trash`): a file goes there and comes back, so nothing is left in it.
    #[test]
    #[ignore]
    #[cfg(target_os = "macos")]
    fn the_system_trash_takes_a_file_and_says_where_it_went() {
        let dir = std::env::temp_dir().join(format!("lc-system-trash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("lightcraft-trash-test.jpg");
        std::fs::write(&photo, "x").unwrap();
        let at = SystemBin.trash(&photo).unwrap().expect("the Finder's Trash says where");
        assert!(!photo.exists() && at.exists(), "{at:?}");
        SystemBin.put_back(&at, &photo).unwrap();
        assert!(photo.exists() && !at.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
