use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub loader: crate::loaders::LoaderKind,
    pub loader_version: String,
    pub memory_mb: u32,
    pub java_path: String,
    pub microsoft_client_id: String,
    pub selected_version: String,
    pub show_snapshots: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            loader: crate::loaders::LoaderKind::Vanilla,
            loader_version: String::new(),
            memory_mb: 4096,
            java_path: String::new(),
            microsoft_client_id: String::new(),
            selected_version: String::new(),
            show_snapshots: false,
        }
    }
}

pub fn data_dir() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("RustyLauncher")
}

pub fn game_dir() -> PathBuf {
    data_dir().join("game")
}

impl Settings {
    pub fn load() -> Result<Self> {
        Self::load_at(&data_dir().join("settings.json"))
    }

    fn load_at(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::from_value(read_document(path)?)
    }

    fn from_value(value: serde_json::Value) -> Result<Self> {
        let mut settings: Self = serde_json::from_value(value)
            .context("설정 형식을 확인하세요. 기존 파일은 보존했습니다")?;
        settings.memory_mb = settings.memory_mb.clamp(1024, 32768);
        Ok(settings)
    }

    /// Merge only fields edited since the last snapshot into the latest disk document.
    /// Other launcher windows and external editors may have changed unrelated fields.
    pub fn save_changes(&self, baseline: &Self) -> Result<Self> {
        self.save_changes_at(&data_dir().join("settings.json"), baseline)
    }

    fn save_changes_at(&self, path: &Path, baseline: &Self) -> Result<Self> {
        let parent = path.parent().context("설정 저장 경로 오류")?;
        fs::create_dir_all(parent)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock().context("설정 파일 잠금 실패")?;
        let existed = path.exists();
        let mut document = if existed {
            read_document(path)?
        } else {
            serde_json::to_value(Self::default())?
        };
        // Refuse to replace malformed or incompatible settings with defaults.
        Self::from_value(document.clone())?;
        let edited = serde_json::to_value(self)?;
        let before = serde_json::to_value(baseline)?;
        let object = document
            .as_object_mut()
            .context("설정은 JSON 객체여야 합니다")?;
        let mut changed = false;
        for (key, value) in edited.as_object().context("설정 변환 실패")? {
            if before.get(key) != Some(value) {
                object.insert(key.clone(), value.clone());
                changed = true;
            }
        }
        let merged = Self::from_value(document.clone())?;
        if changed || !existed {
            atomic_write(path, &serde_json::to_vec_pretty(&document)?)?;
        }
        Ok(merged)
    }
}

fn read_document(path: &Path) -> Result<serde_json::Value> {
    let bytes = fs::read(path).context("설정 파일을 읽을 수 없습니다")?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).context("설정 JSON을 읽을 수 없습니다. 기존 파일은 보존했습니다")
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path).context("설정 저장 실패")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub fn open_path(path: impl AsRef<std::ffi::OsStr>) -> Result<()> {
    Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .context("탐색기를 열 수 없습니다")?;
    Ok(())
}

pub fn edit_settings() -> Result<()> {
    let path = data_dir().join("settings.json");
    if !path.exists() {
        Settings::default().save_changes(&Settings::default())?;
    }
    Command::new("notepad.exe").arg(path).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaders::LoaderKind;
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "rusty-config-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> PathBuf {
            self.0.join("settings.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for entry in fs::read_dir(&self.0).unwrap().flatten() {
                fs::remove_file(entry.path()).unwrap();
            }
            fs::remove_dir(&self.0).unwrap();
        }
    }

    #[test]
    fn preserves_all_preferences_after_reopening() {
        let fixture = Fixture::new();
        let before = Settings::default();
        let edited = Settings {
            memory_mb: 8192,
            java_path: "C:/Program Files/Java/bin/java.exe".to_owned(),
            microsoft_client_id: "00001111-aaaa-2222-bbbb-3333cccc4444".to_owned(),
            loader: LoaderKind::NeoForge,
            loader_version: "21.1.265".to_owned(),
            selected_version: "1.21.1".to_owned(),
            show_snapshots: true,
        };
        edited.save_changes_at(&fixture.path(), &before).unwrap();
        let reopened = Settings::load_at(&fixture.path()).unwrap();
        assert_eq!(
            serde_json::to_value(reopened).unwrap(),
            serde_json::to_value(edited).unwrap()
        );
    }

    #[test]
    fn stale_window_does_not_erase_external_app_id_or_unknown_fields() {
        let fixture = Fixture::new();
        let before = Settings::default();
        before.save_changes_at(&fixture.path(), &before).unwrap();
        let mut document = read_document(&fixture.path()).unwrap();
        document["microsoft_client_id"] = json!("00001111-aaaa-2222-bbbb-3333cccc4444");
        document["future_option"] = json!({"enabled":true});
        atomic_write(&fixture.path(), &serde_json::to_vec(&document).unwrap()).unwrap();
        let stale = Settings {
            memory_mb: 6144,
            ..before.clone()
        };
        let merged = stale.save_changes_at(&fixture.path(), &before).unwrap();
        assert_eq!(
            merged.microsoft_client_id,
            "00001111-aaaa-2222-bbbb-3333cccc4444"
        );
        assert_eq!(merged.memory_mb, 6144);
        let reopened = Settings::load_at(&fixture.path()).unwrap();
        assert_eq!(reopened.microsoft_client_id, merged.microsoft_client_id);
        assert_eq!(
            read_document(&fixture.path()).unwrap()["future_option"],
            json!({"enabled":true})
        );
        let cleared = Settings {
            microsoft_client_id: String::new(),
            ..merged.clone()
        };
        cleared.save_changes_at(&fixture.path(), &merged).unwrap();
        assert!(
            Settings::load_at(&fixture.path())
                .unwrap()
                .microsoft_client_id
                .is_empty()
        );
    }

    #[test]
    fn concurrent_windows_merge_separate_fields() {
        let fixture = Fixture::new();
        let before = Settings::default();
        before.save_changes_at(&fixture.path(), &before).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for i in 0..2 {
            let baseline = before.clone();
            let path = fixture.path();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let mut edited = baseline.clone();
                if i == 0 {
                    edited.memory_mb = 8192;
                } else {
                    edited.java_path = "D:/Java/bin/java.exe".to_owned();
                }
                barrier.wait();
                edited.save_changes_at(&path, &baseline).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let reopened = Settings::load_at(&fixture.path()).unwrap();
        assert_eq!(reopened.memory_mb, 8192);
        assert_eq!(reopened.java_path, "D:/Java/bin/java.exe");
    }

    #[test]
    fn reads_notepad_utf8_bom_and_refuses_to_overwrite_invalid_json() {
        let fixture = Fixture::new();
        let path = fixture.path();
        fs::write(&path, b"\xef\xbb\xbf{\"memory_mb\":8192}").unwrap();
        assert_eq!(Settings::load_at(&path).unwrap().memory_mb, 8192);
        for invalid in [b"{broken".as_slice(), b"{\"memory_mb\":\"bad\"}".as_slice()] {
            fs::write(&path, invalid).unwrap();
            let edited = Settings {
                memory_mb: 6144,
                ..Default::default()
            };
            assert!(Settings::load_at(&path).is_err());
            assert!(edited.save_changes_at(&path, &Settings::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), invalid);
        }
    }
}
