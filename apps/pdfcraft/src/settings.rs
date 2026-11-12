//! Platform preference location, separate from logs and crash recovery.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static STAGING_ID: AtomicU64 = AtomicU64::new(0);

pub fn directory(
    xdg: bool,
    portable: Option<&Path>,
    legacy: Option<PathBuf>,
    config_home: Option<PathBuf>,
    home: Option<PathBuf>,
) -> std::io::Result<Option<PathBuf>> {
    if let Some(portable) = portable {
        return Ok(Some(portable.to_path_buf()));
    }
    if !xdg {
        return Ok(legacy);
    }
    // XDG requires absolute environment paths; empty and relative overrides are ignored.
    let preferred = config_home.filter(|p| p.is_absolute()).or_else(|| home.filter(|p| p.is_absolute()).map(|p| p.join(".config")));
    let Some(preferred) = preferred.map(|p| p.join(pdfcraft_ui_egui::portable::APP_DIR_LOWER)) else { return Ok(legacy) };
    let target = preferred.join("app.ron");
    if std::fs::symlink_metadata(&target).is_ok() {
        return Ok(Some(preferred));
    }
    create_private_directory(&preferred)?;
    if let Some(source) = legacy.map(|p| p.join("app.ron")) {
        match std::fs::File::open(&source) {
            Ok(mut source) => migrate(&mut source, &preferred, &target)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(preferred))
}

/// Create private configuration directories without changing existing directory permissions.
fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)
}

/// Publish a complete copy without replacing a concurrent launch's preferences. Keep the
/// original file and all other data (especially recovery and digital IDs) where they were.
fn migrate(source: &mut std::fs::File, directory: &Path, target: &Path) -> std::io::Result<()> {
    create_private_directory(directory)?;
    for _ in 0..32 {
        let id = STAGING_ID.fetch_add(1, Ordering::Relaxed);
        let staging = directory.join(format!(".app.ron-migrate-{}-{id}", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = match options.open(&staging) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let result = (|| {
            std::io::copy(source, &mut file)?;
            file.sync_all()?;
            // Both files are in the same directory/filesystem. Unlike rename, hard_link
            // atomically refuses an existing destination on Unix as well as Windows.
            match std::fs::hard_link(&staging, target) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                result => result,
            }
        })();
        drop(file);
        let _ = std::fs::remove_file(&staging);
        return result;
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free settings migration staging name"))
}

/// Copies the folder `from` to `to` once, for MaewPDF's first start after PdfCraft Thai Edition:
/// both kept their settings in official PdfCraft's folders, which official PdfCraft may still use,
/// so they are copied, never moved. Nothing happens when `to` already exists (MaewPDF has its own
/// by then) or `from` doesn't. The copy is made beside `to` and renamed into place, so an
/// interrupted copy is never mistaken for a finished one. Links are skipped (they could point
/// anywhere), top-level names in `skip` too (`logs`), and the walk is bounded in depth and in
/// file count so a strange folder can't keep the start-up busy. Returns whether it copied.
pub fn copy_once(from: &Path, to: &Path, skip: &[&str]) -> std::io::Result<bool> {
    if std::fs::symlink_metadata(to).is_ok() || !std::fs::symlink_metadata(from).is_ok_and(|m| m.is_dir()) {
        return Ok(false);
    }
    let id = STAGING_ID.fetch_add(1, Ordering::Relaxed);
    let staging =
        to.with_file_name(format!(".{}-copying-{}-{id}", to.file_name().and_then(|n| n.to_str()).unwrap_or("settings"), std::process::id()));
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut budget = 10_000u32;
    let result = copy_tree(from, &staging, skip, 0, &mut budget).and_then(|()| std::fs::rename(&staging, to));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map(|()| true)
}

fn copy_tree(from: &Path, to: &Path, skip: &[&str], depth: u32, budget: &mut u32) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    if depth >= 8 {
        return Ok(());
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if depth == 0 && name.to_str().is_some_and(|n| skip.iter().any(|s| s.eq_ignore_ascii_case(n))) {
            continue;
        }
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        // `file_type` doesn't follow links, so a link is neither a file nor a folder here.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &to.join(&name), skip, depth + 1, budget)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(&name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_preferences_use_config_instead_of_data() {
        let fixture = Fixture::new();
        let home = fixture.0.join("home");
        let data = home.join(".local/share/pdfcraft");
        assert_eq!(directory(true, None, Some(data), None, Some(home.clone())).unwrap(), Some(home.join(".config/maewpdf")));
    }

    #[test]
    fn official_settings_are_copied_once_and_left_in_place() {
        let f = Fixture::new();
        let (old, new) = (f.0.join("PdfCraft"), f.0.join("MaewPDF"));
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::write(old.join("app.ron"), b"PREFS").unwrap();
        std::fs::write(old.join("sub").join("words.txt"), b"W").unwrap();
        std::fs::write(old.join("logs").join("x.log"), b"L").unwrap();
        assert!(copy_once(&old, &new, &["logs"]).unwrap());
        assert_eq!(std::fs::read(new.join("app.ron")).unwrap(), b"PREFS");
        assert_eq!(std::fs::read(new.join("sub").join("words.txt")).unwrap(), b"W");
        assert!(!new.join("logs").exists(), "logs stay behind");
        assert_eq!(std::fs::read(old.join("app.ron")).unwrap(), b"PREFS", "official PdfCraft keeps its own");
        std::fs::write(new.join("app.ron"), b"MINE").unwrap();
        std::fs::write(old.join("app.ron"), b"LATER").unwrap();
        assert!(!copy_once(&old, &new, &[]).unwrap(), "MaewPDF's own folder is never overwritten");
        assert_eq!(std::fs::read(new.join("app.ron")).unwrap(), b"MINE");
        assert!(!copy_once(&f.0.join("absent"), &f.0.join("other"), &[]).unwrap());
        assert!(!f.0.join("other").exists());
        let leftovers = std::fs::read_dir(&f.0).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("copying")).count();
        assert_eq!(leftovers, 0);
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let id = STAGING_ID.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("pdfcraft-settings-{}-{id}", std::process::id()));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn xdg_override_and_invalid_overrides() {
        let f = Fixture::new();
        let home = f.0.join("home");
        let config = f.0.join("custom-config");
        assert_eq!(directory(true, None, None, Some(config.clone()), Some(home.clone())).unwrap(), Some(config.join("maewpdf")));
        for invalid in [PathBuf::new(), PathBuf::from("relative")] {
            assert_eq!(directory(true, None, None, Some(invalid), Some(home.clone())).unwrap(), Some(home.join(".config/maewpdf")));
        }
        assert_eq!(directory(true, None, Some(f.0.clone()), None, None).unwrap(), Some(f.0.clone()));
    }

    #[test]
    fn migration_preserves_preferences_and_other_data() {
        let f = Fixture::new();
        let data = f.0.join("data/pdfcraft");
        let config = f.0.join("config");
        std::fs::create_dir_all(data.join("recovery")).unwrap();
        std::fs::write(data.join("app.ron"), b"(theme:Dark,language:German)").unwrap();
        std::fs::write(data.join("recovery/unsaved.pdf"), b"UNSAVED").unwrap();
        std::fs::write(data.join("digital-id.p12"), b"KEY").unwrap();
        let selected = directory(true, None, Some(data.clone()), Some(config.clone()), None).unwrap().unwrap();
        assert_eq!(selected, config.join("maewpdf"));
        assert_eq!(std::fs::read(selected.join("app.ron")).unwrap(), std::fs::read(data.join("app.ron")).unwrap());
        assert_eq!(std::fs::read(data.join("recovery/unsaved.pdf")).unwrap(), b"UNSAVED");
        assert_eq!(std::fs::read(data.join("digital-id.p12")).unwrap(), b"KEY");
        // The migrated file is a copy, not a link to the original settings.
        std::fs::write(selected.join("app.ron"), b"NEW").unwrap();
        assert_eq!(std::fs::read(data.join("app.ron")).unwrap(), b"(theme:Dark,language:German)");
        assert_eq!(directory(true, None, Some(data), Some(config), None).unwrap(), Some(selected.clone()));
        assert_eq!(std::fs::read(selected.join("app.ron")).unwrap(), b"NEW");
        assert_eq!(std::fs::read_dir(selected).unwrap().count(), 1);
    }

    #[test]
    fn existing_or_concurrently_created_preferences_are_never_replaced() {
        let f = Fixture::new();
        let source = f.0.join("old.ron");
        let config = f.0.join("config/pdfcraft");
        std::fs::write(&source, b"OLD").unwrap();
        std::fs::create_dir_all(&config).unwrap();
        let target = config.join("app.ron");
        std::fs::write(&target, b"CURRENT").unwrap();
        migrate(&mut std::fs::File::open(source).unwrap(), &config, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"CURRENT");
        assert_eq!(std::fs::read_dir(config).unwrap().count(), 1);
    }

    #[test]
    fn failed_migration_keeps_original_available() {
        let f = Fixture::new();
        std::fs::write(f.0.join("app.ron"), b"OLD").unwrap();
        let blocked = f.0.join("blocked");
        std::fs::write(&blocked, b"NOT A DIRECTORY").unwrap();
        assert!(directory(true, None, Some(f.0.clone()), Some(blocked), None).is_err());
        assert_eq!(std::fs::read(f.0.join("app.ron")).unwrap(), b"OLD");
    }

    #[test]
    fn portable_and_non_xdg_platforms_keep_their_locations() {
        let f = Fixture::new();
        let portable = f.0.join("MaewPDFData");
        for xdg in [true, false] {
            assert_eq!(directory(xdg, Some(&portable), Some(f.0.clone()), Some(f.0.clone()), None).unwrap(), Some(portable.clone()));
        }
        assert_eq!(directory(false, None, Some(f.0.clone()), Some(portable), None).unwrap(), Some(f.0.clone()));
        assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0, "no migration outside XDG mode");
    }

    #[cfg(unix)]
    #[test]
    fn new_config_directory_is_private_and_existing_permissions_are_preserved() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let selected = directory(true, None, None, Some(f.0.join("config")), None).unwrap().unwrap();
        assert_eq!(std::fs::metadata(&selected).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::set_permissions(&selected, std::fs::Permissions::from_mode(0o750)).unwrap();
        create_private_directory(&selected).unwrap();
        assert_eq!(std::fs::metadata(selected).unwrap().permissions().mode() & 0o777, 0o750);
    }
}
