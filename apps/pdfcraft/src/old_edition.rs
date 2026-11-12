//! MaewPDF was PdfCraft Thai Edition until 0.20.0-th.x. That edition installed with official
//! PdfCraft's UpgradeCode, so MaewPDF's installer can't remove it: its Upgrade table would remove
//! official PdfCraft too, and an MSI can't start another uninstall while it runs. So MaewPDF's
//! start removes it instead, without asking (it was replaced, and a question would only confuse):
//! only entries that are certainly the Thai edition's, by product code, publisher and name.
//! Official PdfCraft (another publisher and name) is never touched, nor are the old edition's
//! settings, which MaewPDF has copied.

/// The publisher and product name PdfCraft Thai Edition's MSI wrote under `Uninstall`.
pub const OLD_PUBLISHER: &str = "PdfCraft Thai Edition (unofficial)";
pub const OLD_NAME: &str = "PdfCraft Thai Edition";

/// How many starts try to remove it: a cancelled administrator prompt (an install for everyone)
/// is asked again, but not forever.
const ATTEMPTS: u8 = 3;

/// The MSI product code of an `Uninstall` entry named `key`, when the entry is certainly
/// PdfCraft Thai Edition's: the key is a braced GUID (it goes on msiexec's command line, so
/// nothing else may pass), and the publisher and the name are exactly the edition's.
pub fn product_code<'a>(key: &'a str, publisher: &str, name: &str) -> Option<&'a str> {
    let guid = key.strip_prefix('{')?.strip_suffix('}')?;
    let shape = guid.len() == 36 && guid.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() });
    (shape && publisher == OLD_PUBLISHER && name == OLD_NAME).then_some(key)
}

/// How many removals `marker` records (none when it is missing or unreadable).
fn attempts(marker: &[u8]) -> u8 {
    marker.iter().filter(|b| **b == b'\n').count().min(usize::from(u8::MAX)) as u8
}

/// Starts uninstalling PdfCraft Thai Edition when it is still installed. `marker` counts the
/// starts that tried, up to [`ATTEMPTS`]. Best effort: any failure just leaves it installed.
#[cfg(windows)]
pub fn remove(marker: &std::path::Path) {
    let tried = std::fs::read(marker).map(|b| attempts(&b)).unwrap_or(0);
    if tried >= ATTEMPTS {
        return;
    }
    let codes = installed();
    if codes.is_empty() {
        return;
    }
    if let Some(dir) = marker.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let record = std::fs::OpenOptions::new().create(true).append(true).open(marker).and_then(|mut f| std::io::Write::write_all(&mut f, b"tried\n"));
    if let Err(e) = record {
        // Unrecorded, it would be tried on every start: don't try at all.
        log::warn!("not removing PdfCraft Thai Edition: can't record the attempt ({e})");
        return;
    }
    for code in codes {
        // msiexec asks for administrator rights itself when the old edition was installed for
        // everyone. Not waited for: MaewPDF opens meanwhile.
        match std::process::Command::new("msiexec.exe").args(["/x", &code, "/passive", "/norestart"]).spawn() {
            Ok(_) => log::info!("uninstalling PdfCraft Thai Edition {code}, which MaewPDF replaces"),
            Err(e) => log::warn!("couldn't start uninstalling PdfCraft Thai Edition {code}: {e}"),
        }
    }
}

/// Product codes of the PdfCraft Thai Edition installations Windows lists, for everyone and for
/// this user.
#[cfg(windows)]
fn installed() -> Vec<String> {
    use winsafe::{HKEY, RegistryValue, co};
    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL_32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    let mut codes = Vec::new();
    for (root, path) in [(&HKEY::LOCAL_MACHINE, UNINSTALL), (&HKEY::LOCAL_MACHINE, UNINSTALL_32), (&HKEY::CURRENT_USER, UNINSTALL)] {
        let Ok(list) = root.RegOpenKeyEx(Some(path), co::REG_OPTION::default(), co::KEY::READ) else { continue };
        let Ok(names) = list.RegEnumKeyEx() else { continue };
        // Bounded: a damaged registry can't keep the start-up busy.
        for key in names.take(10_000).flatten() {
            if !key.starts_with('{') {
                continue;
            }
            let Ok(entry) = list.RegOpenKeyEx(Some(&key), co::REG_OPTION::default(), co::KEY::READ) else { continue };
            let text = |value: &str| match entry.RegQueryValueEx(Some(value)) {
                Ok(RegistryValue::Sz(s)) => s,
                _ => String::new(),
            };
            if let Some(code) = product_code(&key, &text("Publisher"), &text("DisplayName"))
                && !codes.iter().any(|c: &String| c.eq_ignore_ascii_case(code))
            {
                codes.push(code.to_owned());
            }
        }
    }
    codes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_thai_editions_msi_entries_are_found() {
        let code = "{D409508B-861C-4855-9EAC-2111608A669E}";
        assert_eq!(product_code(code, OLD_PUBLISHER, OLD_NAME), Some(code));
        assert_eq!(product_code(code, "ArtCraft", "PdfCraft"), None, "official PdfCraft is never touched");
        assert_eq!(product_code(code, "MaewPDF", "MaewPDF"), None, "nor MaewPDF itself");
        assert_eq!(product_code(code, "PdfCraft Thai Edition", OLD_NAME), None, "the publisher must match exactly");
        assert_eq!(product_code(code, OLD_PUBLISHER, "PdfCraft"), None, "and the name too");
        for key in [
            "PdfCraft",
            "D409508B-861C-4855-9EAC-2111608A669E",
            "{D409508B-861C-4855-9EAC-2111608A669}",
            "{D409508B-861C-4855-9EAC-2111608A669E} /qn",
            "{D409508B-861C-4855-9EAC-2111608A669G}",
            "{D409508B 861C-4855-9EAC-2111608A669E}",
            "{ไทย}",
        ] {
            assert_eq!(product_code(key, OLD_PUBLISHER, OLD_NAME), None, "{key}");
        }
    }

    #[test]
    fn removal_is_tried_a_few_starts_at_most() {
        assert_eq!(attempts(b""), 0);
        assert_eq!(attempts(b"tried\n"), 1);
        assert!(attempts(b"tried\ntried\ntried\n") >= ATTEMPTS);
        assert_eq!(attempts(&[b'\n'; 1000]), u8::MAX, "a huge marker can't overflow the count");
    }
}
