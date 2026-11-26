//! System keychain entries under `aquanter-surface.<service>`.
//!
//! Releases before 0.9.4 (and upstream Flowsurface) used `flowsurface.<service>`.
//! The first time an entry is opened its old value is copied over, once, so saved
//! keys survive the rename; the old entry is left for upstream installs.

const SERVICE_PREFIX: &str = "aquanter-surface";
const LEGACY_SERVICE_PREFIX: &str = "flowsurface";
/// Lines of `service<TAB>account` already migrated, so a key the user deletes is
/// never copied back from the old entry.
const MIGRATED_FILE: &str = "keychain-migrated.txt";

pub fn service_name(service: &str) -> String {
    format!("{SERVICE_PREFIX}.{service}")
}

pub fn entry(service: &str, account: &str) -> keyring::Result<keyring::Entry> {
    let entry = keyring::Entry::new(&service_name(service), account)?;
    if !cfg!(test) {
        let path = crate::data_path(Some(MIGRATED_FILE));
        migrate(&entry, LEGACY_SERVICE_PREFIX, service, account, &path);
    }
    Ok(entry)
}

fn migrate(
    entry: &keyring::Entry,
    legacy_prefix: &str,
    service: &str,
    account: &str,
    path: &std::path::Path,
) {
    let marker = format!("{service}\t{account}");
    let done = std::fs::read_to_string(path).unwrap_or_default();
    if done.lines().any(|line| line == marker) {
        return;
    }
    match entry.get_password() {
        Ok(_) => {}
        Err(keyring::Error::NoEntry) => {
            let legacy = keyring::Entry::new(&format!("{legacy_prefix}.{service}"), account)
                .and_then(|legacy| legacy.get_password());
            match legacy {
                Ok(value) => {
                    if let Err(error) = entry.set_password(&value) {
                        log::warn!("Keychain migration failed for {service}/{account}: {error}");
                        return;
                    }
                    log::info!("Migrated keychain entry {service}/{account}");
                }
                Err(keyring::Error::NoEntry) => {}
                // Keychain locked or unavailable: try again next time.
                Err(_) => return,
            }
        }
        Err(_) => return,
    }
    let mut done = done;
    if !done.is_empty() && !done.ends_with('\n') {
        done.push('\n');
    }
    done.push_str(&marker);
    done.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(error) = std::fs::write(path, done) {
        log::warn!("Failed to record keychain migration: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real OS keychain: `cargo test -p aquanter-surface-data keychain -- --ignored`.
    #[test]
    #[ignore]
    fn copies_legacy_entry_once() {
        let account = format!("migration-test-{}", std::process::id());
        let path = std::env::temp_dir().join(format!("aquanter-surface-keychain-{account}.txt"));
        let _ = std::fs::remove_file(&path);
        let legacy = keyring::Entry::new("aquanter-test-legacy.svc", &account).unwrap();
        let new = keyring::Entry::new(&service_name("svc"), &account).unwrap();
        legacy.set_password("secret").unwrap();

        migrate(&new, "aquanter-test-legacy", "svc", &account, &path);
        assert_eq!(new.get_password().unwrap(), "secret");
        // Deleted by the user: not copied back.
        new.delete_credential().unwrap();
        migrate(&new, "aquanter-test-legacy", "svc", &account, &path);
        assert!(matches!(new.get_password(), Err(keyring::Error::NoEntry)));

        legacy.delete_credential().unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
