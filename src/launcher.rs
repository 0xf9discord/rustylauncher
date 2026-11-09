use crate::{
    auth::Session,
    settings::{self, Settings},
};
use anyhow::{Context, Result, bail, ensure};
use rayon::prelude::*;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

const MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub sha1: String,
}

pub enum Event {
    Loaders(
        String,
        crate::loaders::LoaderKind,
        Vec<crate::loaders::LoaderVersion>,
    ),
    Versions(Vec<Version>, String),
    Progress(String, f32),
    Installed(String),
    SignedIn(Session),
    DeviceCode(String, String),
    Started,
    Exited(String),
    Error(String),
}

pub(crate) fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("RustyLauncher/0.1.0")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()?)
}

pub fn versions() -> Result<(Vec<Version>, String)> {
    fs::create_dir_all(settings::data_dir())?;
    let path = settings::data_dir().join("manifest.json");
    let (bytes, status) = match client()?
        .get(MANIFEST)
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
    {
        Ok(bytes) => (bytes.to_vec(), "공식 버전 목록을 불러왔습니다".to_string()),
        Err(error) => (
            fs::read(&path).with_context(|| format!("버전 목록 연결 실패: {error}"))?,
            "오프라인 · 저장된 버전 목록".to_string(),
        ),
    };
    let value: Value = serde_json::from_slice(&bytes)?;
    let versions: Vec<Version> = serde_json::from_value(value["versions"].clone())?;
    fs::write(path, bytes)?;
    Ok((
        versions
            .into_iter()
            .filter(|v| supported_id(&v.id, &v.kind))
            .collect(),
        status,
    ))
}

fn supported_id(id: &str, kind: &str) -> bool {
    if kind == "release" {
        let pieces: Vec<_> = id.split('.').collect();
        return pieces.first() == Some(&"1")
            && pieces
                .get(1)
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|v| v >= 18)
            || pieces
                .first()
                .and_then(|v| v.parse::<u32>().ok())
                .is_some_and(|v| v >= 26);
    }
    // Only modern manifests are supported (Java 17+, structured JVM/game arguments).
    kind == "snapshot"
        && (id
            .get(..2)
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|year| year >= 22)
            || id.starts_with("1.2"))
}

pub fn installed(id: &str) -> bool {
    safe_join(&settings::game_dir().join("versions"), id).is_ok_and(|p| p.join("ready").exists())
}

pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    ensure!(
        !relative.is_empty()
            && Path::new(relative)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "잘못된 상대 경로: {relative}"
    );
    Ok(base.join(relative))
}

#[derive(Clone)]
pub(crate) struct Download {
    pub url: String,
    pub path: PathBuf,
    pub hash: String,
    pub size: u64,
}

fn artifact(value: &Value, path: PathBuf) -> Result<Download> {
    Ok(Download {
        url: value["url"]
            .as_str()
            .context("다운로드 URL 누락")?
            .to_owned(),
        path,
        hash: value["sha1"]
            .as_str()
            .context("다운로드 체크섬 누락")?
            .to_owned(),
        size: value["size"].as_u64().unwrap_or(0),
    })
}

pub(crate) fn download(client: &Client, job: &Download, cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("설치를 취소했습니다");
    }
    if job.path.is_file() && (job.size == 0 || fs::metadata(&job.path)?.len() == job.size) {
        let mut file = File::open(&job.path)?;
        let mut hash = Sha1::new();
        std::io::copy(&mut file, &mut hash)?;
        if format!("{:x}", hash.finalize()) == job.hash {
            return Ok(());
        }
    }
    ensure!(
        job.url.starts_with("https://"),
        "HTTPS 다운로드만 지원합니다"
    );
    fs::create_dir_all(job.path.parent().context("다운로드 경로 오류")?)?;
    let temp = job.path.with_extension("download");
    let result = (|| -> Result<()> {
        let mut response = client.get(&job.url).send()?.error_for_status()?;
        let mut file = File::create(&temp)?;
        let mut hasher = Sha1::new();
        let mut count = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("설치를 취소했습니다");
            }
            let size = response.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            file.write_all(&buffer[..size])?;
            hasher.update(&buffer[..size]);
            count += size as u64;
        }
        ensure!(job.size == 0 || job.size == count, "다운로드 크기 불일치");
        ensure!(
            format!("{:x}", hasher.finalize()) == job.hash,
            "SHA-1 검증 실패: {}",
            job.path.display()
        );
        drop(file);
        fs::rename(&temp, &job.path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn rules_allowed(value: &Value) -> bool {
    let Some(rules) = value["rules"].as_array() else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        // This binary targets Windows x86_64; optional features default to false.
        let os = &rule["os"];
        let os_matches = os["name"].as_str().is_none_or(|s| s == "windows")
            && os["arch"]
                .as_str()
                .is_none_or(|s| s == "x86_64" || s == "amd64")
            && os["version"]
                .as_str()
                .is_none_or(|pattern| regex::Regex::new(pattern).is_ok_and(|r| r.is_match("10.0")));
        let features_match = rule["features"]
            .as_object()
            .is_none_or(|f| f.values().all(|v| v == false));
        if os_matches && features_match {
            allowed = rule["action"] == "allow";
        }
    }
    allowed
}

fn library_jobs(meta: &Value, root: &Path) -> Result<(Vec<Download>, Vec<PathBuf>, Vec<PathBuf>)> {
    let mut jobs = Vec::new();
    let mut classpath = Vec::new();
    let mut natives = Vec::new();
    for lib in meta["libraries"]
        .as_array()
        .context("라이브러리 목록 누락")?
    {
        if !rules_allowed(lib) {
            continue;
        }
        let a = &lib["downloads"]["artifact"];
        if let Some(path) = a["path"].as_str() {
            let path = safe_join(&root.join("libraries"), path)?;
            classpath.push(path.clone());
            jobs.push(artifact(a, path)?);
        }
        if let Some(classifier) = lib["natives"]["windows"].as_str() {
            let classifier = classifier.replace("${arch}", "64");
            let a = &lib["downloads"]["classifiers"][&classifier];
            let path = safe_join(
                &root.join("libraries"),
                a["path"].as_str().context("네이티브 경로 누락")?,
            )?;
            natives.push(path.clone());
            jobs.push(artifact(a, path)?);
        }
    }
    Ok((jobs, classpath, natives))
}

pub fn install(
    version: &Version,
    tx: &async_channel::Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let client = client()?;
    let root = settings::game_dir();
    let dir = safe_join(&root.join("versions"), &version.id)?;
    fs::create_dir_all(&dir)?;
    // Remove readiness before repair; an interrupted repair cannot be launched.
    if dir.join("ready").exists() {
        fs::remove_file(dir.join("ready"))?;
    }
    let report = |text: &str, progress| {
        tx.send_blocking(Event::Progress(text.to_owned(), progress))
            .ok();
    };
    report("버전 정보를 확인하는 중", 0.01);
    let meta_path = dir.join(format!("{}.json", version.id));
    download(
        &client,
        &Download {
            url: version.url.clone(),
            path: meta_path.clone(),
            hash: version.sha1.clone(),
            size: 0,
        },
        &cancel,
    )?;
    let meta: Value = serde_json::from_slice(&fs::read(meta_path)?)?;
    ensure!(
        meta["arguments"].is_object(),
        "구형 버전은 지원하지 않습니다. 1.18 이상을 선택하세요."
    );
    let (mut jobs, _, natives) = library_jobs(&meta, &root)?;
    jobs.push(artifact(
        &meta["downloads"]["client"],
        dir.join(format!("{}.jar", version.id)),
    )?);
    if let Some(id) = meta["logging"]["client"]["file"]["id"].as_str() {
        jobs.push(artifact(
            &meta["logging"]["client"]["file"],
            safe_join(&root.join("assets/log_configs"), id)?,
        )?);
    }
    let index = &meta["assetIndex"];
    let index_id = index["id"].as_str().context("에셋 인덱스 ID 누락")?;
    let index_path = safe_join(&root.join("assets/indexes"), &format!("{index_id}.json"))?;
    download(&client, &artifact(index, index_path.clone())?, &cancel)?;
    let assets: Value = serde_json::from_slice(&fs::read(index_path)?)?;
    for a in assets["objects"]
        .as_object()
        .context("에셋 목록 누락")?
        .values()
    {
        let hash = a["hash"].as_str().context("에셋 해시 누락")?;
        ensure!(
            hash.len() == 40 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "잘못된 에셋 해시"
        );
        jobs.push(Download {
            url: format!(
                "https://resources.download.minecraft.net/{}/{hash}",
                &hash[..2]
            ),
            path: root.join("assets/objects").join(&hash[..2]).join(hash),
            hash: hash.to_owned(),
            size: a["size"].as_u64().unwrap_or(0),
        });
    }
    jobs.sort_by(|a, b| a.path.cmp(&b.path));
    jobs.dedup_by(|a, b| a.path == b.path);
    let count = AtomicUsize::new(0);
    let total = jobs.len();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(12).build()?;
    pool.install(|| {
        jobs.par_iter().try_for_each(|job| -> Result<()> {
            let mut last_error = None;
            for _ in 0..3 {
                match download(&client, job, &cancel) {
                    Ok(()) => {
                        last_error = None;
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                }
            }
            if let Some(e) = last_error {
                return Err(e);
            }
            let done = count.fetch_add(1, Ordering::Relaxed) + 1;
            if done.is_multiple_of(16) || done == total {
                report(
                    &format!("파일 준비 중 · {done} / {total}"),
                    0.05 + 0.9 * done as f32 / total.max(1) as f32,
                );
            }
            Ok(())
        })
    })?;
    report("네이티브 라이브러리를 준비하는 중", 0.97);
    let native_dir = dir.join("natives");
    fs::create_dir_all(&native_dir)?;
    for archive in natives {
        let mut zip = zip::ZipArchive::new(File::open(archive)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let Some(relative) = entry.enclosed_name() else {
                bail!("안전하지 않은 ZIP 경로");
            };
            if entry.is_dir() || relative.starts_with("META-INF") {
                continue;
            }
            let output = native_dir.join(relative);
            fs::create_dir_all(output.parent().context("네이티브 경로 오류")?)?;
            std::io::copy(&mut entry, &mut File::create(output)?)?;
        }
    }
    ensure!(!cancel.load(Ordering::Relaxed), "설치를 취소했습니다");
    fs::write(dir.join("ready"), b"1")?;
    Ok(())
}

fn java_major(path: &Path) -> Option<u32> {
    let mut command = Command::new(path);
    command.arg("-version");
    hide_console(&mut command);
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    let version = text.split('"').nth(1)?;
    let mut parts = version.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

pub fn find_java(settings: &Settings, required: u32) -> Result<PathBuf> {
    if !settings.java_path.trim().is_empty() {
        let path = PathBuf::from(&settings.java_path);
        ensure!(
            java_major(&path).is_some_and(|v| v >= required),
            "지정된 Java를 실행할 수 없거나 Java {required} 이상이 아닙니다"
        );
        return Ok(path);
    }
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin/java.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .map(|p| p.join("java.exe"))
                .filter(|p| p.exists()),
        );
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        for vendor in ["Eclipse Adoptium", "Java", "Microsoft", "Zulu", "BellSoft"] {
            if let Ok(entries) = fs::read_dir(PathBuf::from(&program_files).join(vendor)) {
                candidates.extend(
                    entries
                        .flatten()
                        .map(|e| e.path().join("bin/java.exe"))
                        .filter(|p| p.exists()),
                );
            }
        }
    }
    let found: Vec<_> = candidates
        .into_iter()
        .filter_map(|p| java_major(&p).map(|v| (p, v)))
        .collect();
    found
        .iter()
        .find(|(_, v)| *v == required)
        .or_else(|| found.iter().find(|(_, v)| *v > required))
        .map(|(p, _)| p.clone())
        .with_context(|| format!("Java {required} 이상을 설치하거나 설정의 java_path를 지정하세요"))
}

#[cfg(windows)]
pub(crate) fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}
#[cfg(not(windows))]
pub(crate) fn hide_console(_: &mut Command) {}

fn expand_arguments(arguments: &Value, variables: &HashMap<&str, String>) -> Result<Vec<String>> {
    let mut output = Vec::new();
    for entry in arguments.as_array().context("실행 인수 누락")? {
        let values: Vec<&str> = if let Some(s) = entry.as_str() {
            vec![s]
        } else if rules_allowed(entry) {
            match &entry["value"] {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            }
        } else {
            vec![]
        };
        for value in values {
            let mut text = value.to_owned();
            for (key, value) in variables {
                text = text.replace(&format!("${{{key}}}"), value);
            }
            ensure!(!text.contains("${"), "지원되지 않는 실행 인수가 있습니다");
            output.push(text);
        }
    }
    Ok(output)
}

pub(crate) fn launch_command(id: &str, settings: &Settings, session: &Session) -> Result<Command> {
    ensure!(
        session.expires > std::time::Instant::now(),
        "로그인이 만료되었습니다. 다시 로그인하세요."
    );
    ensure!(installed(id), "먼저 선택한 버전을 설치하세요");
    let root = settings::game_dir();
    let dir = safe_join(&root.join("versions"), id)?;
    let meta: Value = serde_json::from_slice(&fs::read(dir.join(format!("{id}.json")))?)?;
    let base_id = meta["jar"].as_str().unwrap_or(id);
    ensure!(
        installed(base_id),
        "바닐라 기반 파일을 먼저 설치 / 검사하세요"
    );
    let base_dir = safe_join(&root.join("versions"), base_id)?;
    let instance = crate::loaders::instance_dir(settings)?;
    fs::create_dir_all(&instance)?;
    let required = meta["javaVersion"]["majorVersion"].as_u64().unwrap_or(17) as u32;
    let java = find_java(settings, required)?;
    let (_, mut classpath, _) = library_jobs(&meta, &root)?;
    classpath.push(base_dir.join(format!("{base_id}.jar")));
    ensure!(
        classpath.iter().all(|p| p.exists()),
        "설치 파일이 없습니다. 버전 목록에서 다시 검사하세요."
    );
    let classpath = std::env::join_paths(classpath)?
        .to_string_lossy()
        .into_owned();
    let variables = HashMap::from([
        (
            "natives_directory",
            base_dir.join("natives").to_string_lossy().into_owned(),
        ),
        ("launcher_name", "RustyLauncher".to_owned()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_owned()),
        ("classpath", classpath),
        ("classpath_separator", ";".to_owned()),
        (
            "library_directory",
            root.join("libraries").to_string_lossy().into_owned(),
        ),
        ("auth_player_name", session.name.clone()),
        ("version_name", id.to_owned()),
        ("game_directory", instance.to_string_lossy().into_owned()),
        (
            "assets_root",
            root.join("assets").to_string_lossy().into_owned(),
        ),
        (
            "assets_index_name",
            meta["assetIndex"]["id"]
                .as_str()
                .context("에셋 ID 누락")?
                .to_owned(),
        ),
        ("auth_uuid", session.uuid.clone()),
        ("auth_access_token", session.token.clone()),
        ("user_type", "msa".to_owned()),
        (
            "version_type",
            meta["type"].as_str().unwrap_or("release").to_owned(),
        ),
        ("clientid", settings.microsoft_client_id.clone()),
        ("auth_xuid", String::new()),
        ("user_properties", "{}".to_owned()),
    ]);
    let mut jvm = expand_arguments(&meta["arguments"]["jvm"], &variables)?;
    if let Some(argument) = meta["logging"]["client"]["argument"].as_str() {
        let id = meta["logging"]["client"]["file"]["id"]
            .as_str()
            .context("로깅 설정 ID 누락")?;
        let path = safe_join(&root.join("assets/log_configs"), id)?;
        ensure!(
            path.is_file(),
            "로깅 설정이 없습니다. 파일 검사를 실행하세요."
        );
        jvm.push(argument.replace("${path}", &path.to_string_lossy()));
    }
    let game = expand_arguments(&meta["arguments"]["game"], &variables)?;
    let mut command = Command::new(java);
    command
        .current_dir(&instance)
        .arg(format!("-Xmx{}M", settings.memory_mb.clamp(1024, 32768)))
        .args(jvm)
        .arg(meta["mainClass"].as_str().context("메인 클래스 누락")?)
        .args(game);
    hide_console(&mut command);
    Ok(command)
}

pub fn launch(
    id: &str,
    settings: &Settings,
    session: &Session,
    tx: &async_channel::Sender<Event>,
) -> Result<()> {
    let mut command = launch_command(id, settings, session)?;
    let log = File::create(settings::data_dir().join("game.log"))?;
    command
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    let mut child = command.spawn().context("Java 실행 실패")?;
    tx.send_blocking(Event::Started).ok();
    let status = child.wait()?;
    tx.send_blocking(Event::Exited(if status.success() {
        "게임이 종료되었습니다".to_owned()
    } else {
        format!("게임 종료 코드: {status} · 게임 로그를 확인하세요")
    }))
    .ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ordered_rules_and_optional_features() {
        assert!(rules_allowed(&json!({})));
        assert!(!rules_allowed(
            &json!({"rules":[{"action":"allow","os":{"name":"osx"}}]})
        ));
        assert!(!rules_allowed(
            &json!({"rules":[{"action":"allow"},{"action":"disallow","os":{"name":"windows"}}]})
        ));
        assert!(!rules_allowed(
            &json!({"rules":[{"action":"allow","features":{"is_demo_user":true}}]})
        ));
        assert!(rules_allowed(
            &json!({"rules":[{"action":"allow","features":{"is_demo_user":false}}]})
        ));
    }

    #[test]
    fn rejects_path_escape() {
        for path in ["../escape", "C:\\escape", "\\escape", "a/../../b", ""] {
            assert!(safe_join(Path::new("game"), path).is_err());
        }
        assert!(safe_join(Path::new("game"), "org/example/lib.jar").is_ok());
    }

    #[test]
    fn expands_arguments_without_shell_splitting() {
        let vars = HashMap::from([("game_directory", "C:\\My Games\\Minecraft".to_owned())]);
        let args = expand_arguments(&json!(["--gameDir", "${game_directory}", {"rules":[{"action":"allow","os":{"name":"linux"}}],"value":"bad"}]), &vars).unwrap();
        assert_eq!(args, vec!["--gameDir", "C:\\My Games\\Minecraft"]);
        assert!(expand_arguments(&json!(["${unknown}"]), &vars).is_err());
    }

    #[test]
    fn detects_installed_java() {
        if Command::new("java").arg("-version").output().is_ok() {
            assert!(java_major(Path::new("java")).is_some());
        }
    }

    #[test]
    fn validates_cached_files_and_cancellation() {
        let dir = std::env::temp_dir().join(format!("rusty-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cache.bin");
        fs::write(&path, b"hello").unwrap();
        let job = Download {
            url: "invalid://unreachable".to_owned(),
            path: path.clone(),
            hash: format!("{:x}", Sha1::digest(b"hello")),
            size: 5,
        };
        let client = client().unwrap();
        assert!(download(&client, &job, &AtomicBool::new(false)).is_ok());
        fs::write(&path, b"wrong").unwrap();
        assert!(download(&client, &job, &AtomicBool::new(false)).is_err());
        assert!(download(&client, &job, &AtomicBool::new(true)).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }

    #[test]
    #[ignore = "accesses official Mojang services"]
    fn official_metadata_and_asset_download() -> Result<()> {
        let client = client()?;
        let manifest: Value = client.get(MANIFEST).send()?.error_for_status()?.json()?;
        let list: Vec<Version> = serde_json::from_value(manifest["versions"].clone())?;
        let latest = manifest["latest"]["release"]
            .as_str()
            .context("latest release")?;
        let dir = std::env::temp_dir().join(format!("rusty-network-test-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        for id in [latest, "1.18.2"] {
            let version = list.iter().find(|v| v.id == id).context("version")?;
            ensure!(
                supported_id(&version.id, &version.kind),
                "release filtered out"
            );
            let path = dir.join(format!("{id}.json"));
            download(
                &client,
                &Download {
                    url: version.url.clone(),
                    path: path.clone(),
                    hash: version.sha1.clone(),
                    size: 0,
                },
                &AtomicBool::new(false),
            )?;
            let meta: Value = serde_json::from_slice(&fs::read(path)?)?;
            let (jobs, classpath, _) = library_jobs(&meta, &dir)?;
            ensure!(
                !jobs.is_empty() && !classpath.is_empty(),
                "empty library plan"
            );
            let index_path = dir.join(format!("{id}-assets.json"));
            download(
                &client,
                &artifact(&meta["assetIndex"], index_path.clone())?,
                &AtomicBool::new(false),
            )?;
            let assets: Value = serde_json::from_slice(&fs::read(index_path)?)?;
            let asset = assets["objects"]
                .as_object()
                .context("objects")?
                .values()
                .find(|a| a["size"].as_u64().is_some_and(|s| s > 0 && s < 4096))
                .context("small asset")?;
            let hash = asset["hash"].as_str().context("hash")?;
            download(
                &client,
                &Download {
                    url: format!(
                        "https://resources.download.minecraft.net/{}/{hash}",
                        &hash[..2]
                    ),
                    path: dir.join(hash),
                    hash: hash.to_owned(),
                    size: asset["size"].as_u64().unwrap(),
                },
                &AtomicBool::new(false),
            )?;
            println!(
                "Verified official {id}: {} libraries and one asset",
                jobs.len()
            );
        }
        // This test only creates regular files in its own directory.
        for entry in fs::read_dir(&dir)? {
            fs::remove_file(entry?.path())?;
        }
        fs::remove_dir(dir)?;
        Ok(())
    }
}
