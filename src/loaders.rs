use crate::{
    launcher::{self, Download, Event, Version},
    settings::{self, Settings},
};
use anyhow::{Context, Result, bail, ensure};
use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub const ALL: [Self; 5] = [
        Self::Vanilla,
        Self::Fabric,
        Self::Forge,
        Self::Quilt,
        Self::NeoForge,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }
    fn meta(self) -> &'static str {
        if self == Self::Quilt {
            "https://meta.quiltmc.org/v3"
        } else {
            "https://meta.fabricmc.net/v2"
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoaderVersion {
    pub id: String,
    pub stable: bool,
}

fn identifier(s: &str) -> Result<()> {
    ensure!(
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c)),
        "잘못된 버전 식별자"
    );
    Ok(())
}

pub fn profile_id(game: &str, kind: LoaderKind, loader: &str) -> String {
    if kind == LoaderKind::Vanilla {
        game.to_owned()
    } else {
        format!("rusty-{}-{game}-{loader}", kind.key())
    }
}

pub fn is_installed(game: &str, kind: LoaderKind, loader: &str) -> bool {
    launcher::installed(game)
        && (kind == LoaderKind::Vanilla
            || !loader.is_empty() && launcher::installed(&profile_id(game, kind, loader)))
}

pub fn instance_dir(settings: &Settings) -> Result<PathBuf> {
    if settings.loader == LoaderKind::Vanilla {
        return Ok(settings::game_dir());
    }
    identifier(&settings.selected_version)?;
    launcher::safe_join(
        &settings::data_dir().join("instances"),
        &format!("{}-{}", settings.loader.key(), settings.selected_version),
    )
}

fn numeric_key(s: &str) -> Vec<u32> {
    s.split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
}

fn neo_prefix(game: &str) -> Option<String> {
    let parts: Vec<_> = game.split('.').collect();
    if parts.iter().any(|p| p.parse::<u32>().is_err()) {
        return None;
    }
    if parts.first() == Some(&"1") {
        Some(format!(
            "{}.{}.",
            parts.get(1)?,
            parts.get(2).unwrap_or(&"0")
        ))
    } else if parts.first()?.parse::<u32>().ok()? >= 26 {
        Some(format!(
            "{}.{}.{}.",
            parts.first()?,
            parts.get(1)?,
            parts.get(2).unwrap_or(&"0")
        ))
    } else {
        None
    }
}

pub fn versions(game: &str, kind: LoaderKind) -> Result<Vec<LoaderVersion>> {
    identifier(game)?;
    if kind == LoaderKind::Vanilla {
        return Ok(vec![]);
    }
    let client = launcher::client()?;
    let mut list = match kind {
        LoaderKind::Fabric | LoaderKind::Quilt => {
            let response = client
                .get(format!("{}/versions/loader/{game}", kind.meta()))
                .send()?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok(vec![]);
            }
            let value: Value = response.error_for_status()?.json()?;
            value
                .as_array()
                .context("로더 버전 목록 오류")?
                .iter()
                .filter_map(|v| {
                    let id = v["loader"]["version"].as_str()?.to_owned();
                    let stable = v["loader"]["stable"].as_bool().unwrap_or(
                        !id.contains("beta") && !id.contains("alpha") && !id.contains("rc"),
                    );
                    Some(LoaderVersion { id, stable })
                })
                .collect::<Vec<_>>()
        }
        LoaderKind::Forge => {
            let value: Value = client
                .get(
                    "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json",
                )
                .send()?
                .error_for_status()?
                .json()?;
            value[game]
                .as_array()
                .map(|v| {
                    v.iter()
                        .filter_map(|v| v.as_str())
                        .map(|v| LoaderVersion {
                            id: v.to_owned(),
                            stable: !v.contains("beta"),
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
        LoaderKind::NeoForge => {
            let Some(prefix) = neo_prefix(game) else {
                return Ok(vec![]);
            };
            let xml=client.get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml").send()?.error_for_status()?.text()?;
            let doc = roxmltree::Document::parse(&xml)?;
            doc.descendants()
                .filter(|n| n.has_tag_name("version"))
                .filter_map(|n| n.text())
                .filter(|v| v.starts_with(&prefix))
                .map(|v| LoaderVersion {
                    id: v.to_owned(),
                    stable: !v.contains("beta") && !v.contains("alpha"),
                })
                .collect()
        }
        LoaderKind::Vanilla => vec![],
    };
    list.retain(|v| identifier(&v.id).is_ok());
    list.sort_by(|a, b| {
        b.stable
            .cmp(&a.stable)
            .then_with(|| numeric_key(&b.id).cmp(&numeric_key(&a.id)))
    });
    list.dedup_by(|a, b| a.id == b.id);
    Ok(list)
}

fn maven_path(coordinate: &str) -> Result<String> {
    let (name, extension) = coordinate.split_once('@').unwrap_or((coordinate, "jar"));
    let parts: Vec<_> = name.split(':').collect();
    ensure!((3..=4).contains(&parts.len()), "잘못된 Maven 좌표");
    for part in parts.iter().copied().chain([extension]) {
        identifier(part)?;
    }
    ensure!(
        parts[0].split('.').all(|p| !p.is_empty()),
        "잘못된 Maven 그룹"
    );
    let classifier = parts.get(3).map(|s| format!("-{s}")).unwrap_or_default();
    Ok(format!(
        "{}/{}/{}/{}-{}{}.{}",
        parts[0].replace('.', "/"),
        parts[1],
        parts[2],
        parts[1],
        parts[2],
        classifier,
        extension
    ))
}

fn library_key(lib: &Value) -> String {
    let name = lib["name"].as_str().unwrap_or("");
    let parts: Vec<_> = name.split(':').collect();
    if parts.len() >= 3 {
        format!("{}:{}:{}", parts[0], parts[1], parts.get(3).unwrap_or(&""))
    } else {
        name.to_owned()
    }
}

pub(crate) fn merge_profile(base: &Value, child: &Value, id: &str, game: &str) -> Result<Value> {
    ensure!(
        child["inheritsFrom"].as_str() == Some(game),
        "로더와 마인크래프트 버전이 일치하지 않습니다"
    );
    let mut merged = base.clone();
    for key in ["mainClass", "type", "javaVersion", "logging"] {
        if !child[key].is_null() {
            merged[key] = child[key].clone();
        }
    }
    ensure!(
        merged["mainClass"].as_str().is_some(),
        "로더 메인 클래스 누락"
    );
    for key in ["jvm", "game"] {
        let mut args = base["arguments"][key]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(extra) = child["arguments"][key].as_array() {
            args.extend(extra.iter().cloned());
        }
        merged["arguments"][key] = json!(args);
    }
    let mut seen = HashSet::new();
    let mut libraries = Vec::new();
    // Child libraries override older parent versions; native classifiers remain distinct.
    for lib in child["libraries"]
        .as_array()
        .context("로더 라이브러리 누락")?
        .iter()
        .chain(
            base["libraries"]
                .as_array()
                .context("바닐라 라이브러리 누락")?,
        )
    {
        if seen.insert(library_key(lib)) {
            libraries.push(lib.clone());
        }
    }
    merged["libraries"] = json!(libraries);
    merged["id"] = json!(id);
    merged["jar"] = json!(game);
    merged
        .as_object_mut()
        .context("잘못된 프로필")?
        .remove("inheritsFrom");
    Ok(merged)
}

fn checksum(client: &Client, url: &str) -> Result<String> {
    let text = client
        .get(format!("{url}.sha1"))
        .send()?
        .error_for_status()?
        .text()?;
    let hash = text
        .split_whitespace()
        .next()
        .context("체크섬 누락")?
        .to_ascii_lowercase();
    ensure!(
        hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
        "잘못된 SHA-1 체크섬"
    );
    Ok(hash)
}

fn normalize_libraries(
    client: &Client,
    profile: &mut Value,
    root: &Path,
    installed_by_processor: bool,
    cancel: &AtomicBool,
) -> Result<Vec<Download>> {
    let mut jobs = Vec::new();
    for lib in profile["libraries"]
        .as_array_mut()
        .context("라이브러리 누락")?
    {
        ensure!(!cancel.load(Ordering::Relaxed), "설치를 취소했습니다");
        let artifact = &lib["downloads"]["artifact"];
        let relative = artifact["path"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(maven_path(
                lib["name"].as_str().context("Maven 좌표 누락")?,
            )?);
        let path = launcher::safe_join(&root.join("libraries"), &relative)?;
        let url = artifact["url"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}/{}",
                    lib["url"]
                        .as_str()
                        .unwrap_or("https://libraries.minecraft.net/")
                        .trim_end_matches('/'),
                    relative
                )
            });
        let mut hash = artifact["sha1"]
            .as_str()
            .or_else(|| lib["sha1"].as_str())
            .map(str::to_owned)
            .unwrap_or_default();
        let mut size = artifact["size"]
            .as_u64()
            .or_else(|| lib["size"].as_u64())
            .unwrap_or(0);
        if hash.is_empty() {
            if installed_by_processor && path.is_file() {
                let mut hasher = Sha1::new();
                std::io::copy(&mut File::open(&path)?, &mut hasher)?;
                hash = format!("{:x}", hasher.finalize());
                size = fs::metadata(&path)?.len();
            } else {
                hash = checksum(client, &url)
                    .with_context(|| format!("라이브러리 체크섬 확인 실패: {relative}"))?;
            }
        }
        lib["downloads"]["artifact"] = json!({"path":relative,"url":url,"sha1":hash,"size":size});
        jobs.push(Download {
            url,
            path,
            hash,
            size,
        });
    }
    Ok(jobs)
}

fn installer_url(kind: LoaderKind, version: &str) -> String {
    if kind == LoaderKind::Forge {
        format!(
            "https://maven.minecraftforge.net/net/minecraftforge/forge/{version}/forge-{version}-installer.jar"
        )
    } else {
        format!(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar"
        )
    }
}

fn installer_profile(path: &Path) -> Result<Value> {
    let mut zip = zip::ZipArchive::new(File::open(path)?)?;
    Ok(serde_json::from_reader(
        zip.by_name("version.json")
            .context("설치기에 version.json이 없습니다")?,
    )?)
}

fn run_installer(
    path: &Path,
    root: &Path,
    settings: &Settings,
    required: u32,
    cancel: &AtomicBool,
    tx: &async_channel::Sender<Event>,
) -> Result<()> {
    let profiles = root.join("launcher_profiles.json");
    if !profiles.exists() {
        fs::write(&profiles, b"{\"profiles\":{}}")?;
    }
    let java = launcher::find_java(settings, required)?;
    let log = File::create(settings::data_dir().join("installer.log"))?;
    let mut command = Command::new(java);
    command
        .current_dir(root)
        .arg("-Djava.awt.headless=true")
        .arg("-jar")
        .arg(path)
        .arg("--installClient")
        .arg(root)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    launcher::hide_console(&mut command);
    let mut child = command.spawn().context("공식 로더 설치기 실행 실패")?;
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) || start.elapsed() > Duration::from_secs(1200) {
            #[cfg(windows)]
            {
                // Official installers may spawn Java processors; stop only our owned tree.
                let mut stop = Command::new("taskkill.exe");
                stop.args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                launcher::hide_console(&mut stop);
                let _ = stop.status();
            }
            let _ = child.kill();
            let _ = child.wait();
            bail!(if cancel.load(Ordering::Relaxed) {
                "설치를 취소했습니다"
            } else {
                "설치 제한 시간 초과 · 설치 로그를 확인하세요"
            });
        }
        if let Some(status) = child.try_wait()? {
            ensure!(
                status.success(),
                "공식 로더 설치기 실패 ({status}) · 설치 로그를 확인하세요"
            );
            return Ok(());
        }
        if start.elapsed().as_secs().is_multiple_of(5) {
            tx.send_blocking(Event::Progress(
                format!(
                    "공식 설치기 실행 중 · {}초 · 자세한 내용은 설치 로그",
                    start.elapsed().as_secs()
                ),
                0.7,
            ))
            .ok();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn install(
    version: &Version,
    kind: LoaderKind,
    loader: &str,
    settings: &Settings,
    tx: &async_channel::Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    if kind == LoaderKind::Vanilla {
        return launcher::install(version, tx, cancel);
    }
    identifier(&version.id)?;
    identifier(loader)?;
    let client = launcher::client()?;
    let root = settings::game_dir();
    let id = profile_id(&version.id, kind, loader);
    let dir = launcher::safe_join(&root.join("versions"), &id)?;
    fs::create_dir_all(&dir)?;
    if dir.join("ready").exists() {
        fs::remove_file(dir.join("ready"))?;
    }
    tx.send_blocking(Event::Progress(
        format!("{} {} 설치 정보를 확인하는 중", kind.name(), loader),
        0.01,
    ))
    .ok();
    let processor = matches!(kind, LoaderKind::Forge | LoaderKind::NeoForge);
    let installer = if processor {
        let url = installer_url(kind, loader);
        let path = settings::data_dir()
            .join("installers")
            .join(format!("{}-{loader}.jar", kind.key()));
        launcher::download(
            &client,
            &Download {
                hash: checksum(&client, &url)?,
                url,
                path: path.clone(),
                size: 0,
            },
            &cancel,
        )?;
        Some(path)
    } else {
        None
    };
    let child = if let Some(path) = &installer {
        installer_profile(path)?
    } else {
        client
            .get(format!(
                "{}/versions/loader/{}/{loader}/profile/json",
                kind.meta(),
                version.id
            ))
            .send()?
            .error_for_status()?
            .json::<Value>()?
    };
    ensure!(
        child["inheritsFrom"].as_str() == Some(&version.id),
        "선택한 게임 버전과 로더가 호환되지 않습니다"
    );
    // Vanilla's verified libraries/assets are shared; mods/worlds live in isolated instances.
    launcher::install(version, tx, cancel.clone())?;
    let base_path = launcher::safe_join(&root.join("versions"), &version.id)?
        .join(format!("{}.json", version.id));
    let base: Value = serde_json::from_slice(&fs::read(base_path)?)?;
    if let Some(path) = &installer {
        let required = child["javaVersion"]["majorVersion"]
            .as_u64()
            .or_else(|| base["javaVersion"]["majorVersion"].as_u64())
            .unwrap_or(17) as u32;
        run_installer(path, &root, settings, required, &cancel, tx)?;
    }
    let mut merged = merge_profile(&base, &child, &id, &version.id)?;
    tx.send_blocking(Event::Progress(
        "로더 라이브러리를 검사하는 중".to_owned(),
        0.92,
    ))
    .ok();
    let jobs = normalize_libraries(&client, &mut merged, &root, processor, &cancel)?;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(12).build()?;
    pool.install(|| {
        jobs.par_iter()
            .try_for_each(|j| launcher::download(&client, j, &cancel))
    })?;
    ensure!(!cancel.load(Ordering::Relaxed), "설치를 취소했습니다");
    let temp = dir.join("profile.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(&merged)?)?;
    fs::rename(temp, dir.join(format!("{id}.json")))?;
    let mut selected = settings.clone();
    selected.loader = kind;
    selected.selected_version = version.id.clone();
    fs::create_dir_all(instance_dir(&selected)?.join("mods"))?;
    fs::write(dir.join("ready"), b"1")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_coordinates_preserve_classifiers_and_reject_escape() {
        assert_eq!(
            maven_path("net.fabricmc:sponge-mixin:0.15.0+mixin.0.8.7").unwrap(),
            "net/fabricmc/sponge-mixin/0.15.0+mixin.0.8.7/sponge-mixin-0.15.0+mixin.0.8.7.jar"
        );
        assert_eq!(
            maven_path("org.test:lib:1.0:natives-windows").unwrap(),
            "org/test/lib/1.0/lib-1.0-natives-windows.jar"
        );
        for c in ["../evil:lib:1", "a:b:../x", "a:b:1:../../x", "a:b:1@../zip"] {
            assert!(maven_path(c).is_err());
        }
    }

    #[test]
    fn merges_loader_main_class_arguments_and_overrides_libraries() {
        let base = json!({"mainClass":"Vanilla","javaVersion":{"majorVersion":21},"arguments":{"jvm":["-cp","${classpath}"],"game":["--username","${auth_player_name}"]},"libraries":[{"name":"org.asm:asm:9.0"},{"name":"org.lwjgl:lwjgl:3.0:natives-windows"},{"name":"org.lwjgl:lwjgl:3.0"}],"assetIndex":{"id":"17"}});
        let child = json!({"inheritsFrom":"1.21.1","mainClass":"KnotClient","arguments":{"jvm":["-Dtest=true"],"game":["--fabric"]},"libraries":[{"name":"org.asm:asm:9.7"},{"name":"net.fabricmc:fabric-loader:0.16"}]});
        let merged = merge_profile(&base, &child, "profile", "1.21.1").unwrap();
        assert_eq!(merged["mainClass"], "KnotClient");
        assert_eq!(merged["jar"], "1.21.1");
        assert_eq!(merged["arguments"]["jvm"].as_array().unwrap().len(), 3);
        assert_eq!(merged["arguments"]["game"].as_array().unwrap().len(), 3);
        assert_eq!(merged["libraries"].as_array().unwrap().len(), 4);
        assert_eq!(merged["libraries"][0]["name"], "org.asm:asm:9.7");
        assert_eq!(merged["assetIndex"], base["assetIndex"]);
        assert!(merge_profile(&base, &child, "wrong", "1.20.1").is_err());
    }

    #[test]
    fn neoforge_matches_exact_game_patch() {
        assert_eq!(neo_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neo_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neo_prefix("26.3").as_deref(), Some("26.3.0."));
        assert_eq!(neo_prefix("26.1.2").as_deref(), Some("26.1.2."));
        assert!(neo_prefix("26w01a").is_none());
    }

    #[test]
    fn isolates_mods_by_game_and_loader() {
        let mut settings = Settings {
            loader: LoaderKind::Fabric,
            selected_version: "1.21.1".to_owned(),
            ..Default::default()
        };
        let fabric = instance_dir(&settings).unwrap();
        settings.loader = LoaderKind::Forge;
        assert_ne!(fabric, instance_dir(&settings).unwrap());
        settings.loader = LoaderKind::Fabric;
        settings.selected_version = "1.20.1".to_owned();
        assert_ne!(fabric, instance_dir(&settings).unwrap());
    }

    #[test]
    #[ignore = "downloads and executes official mod loader installers"]
    fn install_official_loaders() -> Result<()> {
        let (vanilla, _) = launcher::versions()?;
        let version = vanilla
            .iter()
            .find(|v| v.id == "1.21.1")
            .context("test game version missing")?;
        let (tx, _rx) = async_channel::unbounded();
        for kind in [
            LoaderKind::Fabric,
            LoaderKind::Quilt,
            LoaderKind::Forge,
            LoaderKind::NeoForge,
        ] {
            let choices = versions(&version.id, kind)?;
            let loader = &choices.first().context("no compatible loader")?.id;
            let settings = Settings {
                loader: kind,
                loader_version: loader.clone(),
                selected_version: version.id.clone(),
                ..Default::default()
            };
            install(
                version,
                kind,
                loader,
                &settings,
                &tx,
                Arc::new(AtomicBool::new(false)),
            )?;
            ensure!(is_installed(&version.id, kind, loader), "profile not ready");
            let id = profile_id(&version.id, kind, loader);
            let meta: Value = serde_json::from_slice(&fs::read(
                settings::game_dir()
                    .join("versions")
                    .join(&id)
                    .join(format!("{id}.json")),
            )?)?;
            ensure!(meta["jar"] == version.id, "base JAR missing");
            ensure!(
                instance_dir(&settings)?.join("mods").is_dir(),
                "mods directory missing"
            );
            // Assemble the real Java command without starting an unauthenticated game.
            let session = crate::auth::Session {
                name: "Validation".to_owned(),
                uuid: "00000000000000000000000000000000".to_owned(),
                token: "validation-only-never-sent".to_owned(),
                expires: Instant::now() + Duration::from_secs(60),
            };
            let command = launcher::launch_command(&id, &settings, &session)?;
            ensure!(command.get_args().count() > 10, "incomplete launch command");
            ensure!(
                command.get_current_dir() == Some(instance_dir(&settings)?.as_path()),
                "wrong instance working directory"
            );
            println!("Verified {} {} on {}", kind.name(), loader, version.id);
        }
        Ok(())
    }
}
