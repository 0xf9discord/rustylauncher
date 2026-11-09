use crate::launcher::Event;
use anyhow::{Context, Result, bail};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

// Deliberately memory-only: no Microsoft, Xbox or Minecraft tokens are written to disk.
#[derive(Clone)]
pub struct Session {
    pub name: String,
    pub uuid: String,
    pub token: String,
    pub expires: Instant,
}

fn string(value: &Value, key: &str) -> Result<String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .with_context(|| format!("인증 응답에 {key} 필드가 없습니다"))
}

const MICROSOFT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";

fn minecraft_rejection(value: &Value) -> (&'static str, &'static str) {
    let message = value["errorMessage"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if message.contains("invalid app registration") {
        (
            "INVALID_APP_REGISTRATION",
            "현재 앱 ID가 Minecraft 서비스에서 허용되지 않았습니다. Minecraft 앱 ID 검토·승인이 필요합니다.",
        )
    } else if value["details"]["reason"] == "ACCOUNT_SUSPENDED"
        || message.contains("account has been suspended")
    {
        (
            "ACCOUNT_SUSPENDED",
            "Minecraft 서비스에서 계정 이용을 제한했습니다. Minecraft 고객 지원에서 계정 상태를 확인하세요.",
        )
    } else if value.is_null() {
        (
            "NON_JSON_RESPONSE",
            "서버 또는 네트워크 게이트웨이가 요청을 거절했습니다. 앱 승인 문제인지는 확인되지 않았습니다.",
        )
    } else {
        (
            "MINECRAFT_FORBIDDEN",
            "Minecraft 서버가 인증 요청을 거절했습니다. 앱 등록, 계정 상태 또는 서비스 문제일 수 있으며 앱 승인 여부는 확인되지 않았습니다.",
        )
    }
}

pub fn validate_client_id(client_id: &str) -> Result<String> {
    let client_id = client_id.trim();
    if client_id.is_empty() {
        bail!("Microsoft 앱 ID가 비어 있습니다. 설정의 microsoft_client_id를 입력하세요.");
    }
    if !regex::Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")?
        .is_match(client_id)
    {
        bail!(
            "Microsoft 애플리케이션(클라이언트) ID 형식이 아닙니다. 콜백 주소나 클라이언트 비밀값 대신 앱 ID를 입력하세요."
        );
    }
    Ok(client_id.to_owned())
}

// Never include response bodies or error_description: they may contain identifiers/tokens.
fn service_error(stage: &str, status: StatusCode, value: &Value) -> String {
    if stage == "Minecraft 인증" && status == StatusCode::FORBIDDEN {
        let (reason, hint) = minecraft_rejection(value);
        return format!("{stage} 실패 (HTTP 403, {reason}) · {hint}");
    }
    let code = value["error"]
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .take(64)
        .collect::<String>();
    let hint = match code.as_str() {
        "invalid_client" | "unauthorized_client" => {
            "앱 ID, 개인 Microsoft 계정 지원, 공용 클라이언트 로그인을 확인하세요."
        }
        "invalid_scope" => "이 앱에서 XboxLive.signin 권한을 사용할 수 있는지 확인하세요.",
        "authorization_declined" | "access_denied" => {
            "로그인이 취소되거나 거절되었습니다. 다시 로그인하세요."
        }
        "expired_token" | "bad_verification_code" => {
            "로그인 코드가 만료되었거나 유효하지 않습니다. 다시 로그인하세요."
        }
        "temporarily_unavailable" | "server_error" => {
            "Microsoft 서비스가 일시적으로 응답하지 않습니다. 잠시 후 다시 시도하세요."
        }
        _ if stage == "Minecraft 프로필" && status == StatusCode::NOT_FOUND => {
            "Minecraft Java Edition 소유권과 게임 프로필을 확인하세요."
        }
        _ if stage == "Xbox Live" || stage == "Xbox XSTS" => {
            "Xbox 프로필과 계정의 가족·이용 설정을 확인하세요."
        }
        _ if status == StatusCode::UNAUTHORIZED => {
            "인증 정보가 거절되었습니다. 앱 ID를 확인하고 다시 로그인하세요."
        }
        _ => "잠시 후 다시 시도하고 앱 등록 및 계정 권한을 확인하세요.",
    };
    let details = if code.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}, {code}", status.as_u16())
    };
    format!("{stage} 실패 ({details}) · {hint}")
}

fn response_json(response: Response, stage: &str) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        let value = response.json().unwrap_or(Value::Null);
        let message = service_error(stage, status, &value);
        // Store only our fixed classifications, never the server's raw body or tokens.
        if std::fs::create_dir_all(crate::settings::data_dir()).is_ok() {
            use std::io::Write;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(crate::settings::data_dir().join("auth.log"))
            {
                let _ = writeln!(file, "{message}");
            }
        }
        bail!("{message}");
    }
    response
        .json()
        .with_context(|| format!("{stage} 응답을 읽을 수 없습니다"))
}

#[derive(Deserialize)]
struct DeviceAuthorization {
    user_code: String,
    verification_uri: String,
    device_code: String,
    expires_in: u64,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

fn auth_client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent("RustyLauncher/0.1.0")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(40))
        .build()?)
}

fn request_device_code(client: &Client, client_id: &str) -> Result<DeviceAuthorization> {
    let client_id = validate_client_id(client_id)?;
    let response = client
        .post(format!("{MICROSOFT}/devicecode"))
        .form(&[
            ("client_id", client_id.as_str()),
            ("scope", "XboxLive.signin"),
        ])
        .send()
        .context("Microsoft 로그인 서버 연결 실패")?;
    let device: DeviceAuthorization =
        serde_json::from_value(response_json(response, "Microsoft 코드 발급")?)
            .context("Microsoft 로그인 코드 응답이 올바르지 않습니다")?;
    anyhow::ensure!(
        !device.user_code.is_empty() && !device.device_code.is_empty() && device.expires_in > 0,
        "Microsoft 로그인 코드가 유효하지 않습니다"
    );
    let uri = reqwest::Url::parse(&device.verification_uri)?;
    let host = uri.host_str().unwrap_or("");
    anyhow::ensure!(
        uri.scheme() == "https"
            && (host == "microsoft.com"
                || host.ends_with(".microsoft.com")
                || host == "live.com"
                || host.ends_with(".live.com")),
        "Microsoft 로그인 주소를 확인할 수 없습니다"
    );
    Ok(device)
}

fn post(client: &Client, url: &str, body: Value, stage: &str) -> Result<Value> {
    response_json(
        client
            .post(url)
            .json(&body)
            .send()
            .with_context(|| format!("{stage} 서버 연결 실패"))?,
        stage,
    )
}

pub fn login(
    client_id: &str,
    tx: &async_channel::Sender<Event>,
    cancel: Arc<AtomicBool>,
) -> Result<Session> {
    let client_id = validate_client_id(client_id)?;
    let client = auth_client()?;
    let device = request_device_code(&client, &client_id)?;
    tx.send_blocking(Event::DeviceCode(device.user_code, device.verification_uri))
        .ok();
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(device.expires_in))
        .context("잘못된 로그인 만료 시간")?;
    let mut interval = device.interval.max(1);
    let access = loop {
        let next_poll = Instant::now()
            .checked_add(Duration::from_secs(interval))
            .context("잘못된 로그인 대기 시간")?;
        while Instant::now() < next_poll {
            if cancel.load(Ordering::Relaxed) {
                bail!("로그인을 취소했습니다");
            }
            if Instant::now() >= deadline {
                bail!("로그인 코드가 만료되었습니다. 다시 로그인하세요.");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if Instant::now() >= deadline {
            bail!("로그인 코드가 만료되었습니다. 다시 로그인하세요.");
        }
        let response = client
            .post(format!("{MICROSOFT}/token"))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id.as_str()),
                ("device_code", device.device_code.as_str()),
            ])
            .send()?;
        let status = response.status();
        let ok = status.is_success();
        let value: Value = response
            .json()
            .context("Microsoft 토큰 응답을 읽을 수 없습니다")?;
        if ok {
            break string(&value, "access_token")?;
        }
        match value["error"].as_str().unwrap_or("") {
            "authorization_pending" => {}
            "slow_down" => interval += 5,
            _ => bail!("{}", service_error("Microsoft 토큰 발급", status, &value)),
        }
    };
    if cancel.load(Ordering::Relaxed) {
        bail!("로그인을 취소했습니다");
    }
    let xbl = post(
        &client,
        "https://user.auth.xboxlive.com/user/authenticate",
        json!({
            "Properties": {"AuthMethod":"RPS", "SiteName":"user.auth.xboxlive.com", "RpsTicket":format!("d={access}")},
            "RelyingParty":"http://auth.xboxlive.com", "TokenType":"JWT"
        }),
        "Xbox Live",
    )?;
    let xsts = post(
        &client,
        "https://xsts.auth.xboxlive.com/xsts/authorize",
        json!({
            "Properties":{"SandboxId":"RETAIL", "UserTokens":[string(&xbl, "Token")?]},
            "RelyingParty":"rp://api.minecraftservices.com/", "TokenType":"JWT"
        }),
        "Xbox XSTS",
    )?;
    let uhs = xsts["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .context("Xbox 프로필을 찾을 수 없습니다")?;
    let mc = post(
        &client,
        "https://api.minecraftservices.com/authentication/login_with_xbox",
        json!({
            "identityToken":format!("XBL3.0 x={uhs};{}", string(&xsts, "Token")?)
        }),
        "Minecraft 인증",
    )?;
    let token = string(&mc, "access_token")?;
    let profile = response_json(
        client
            .get("https://api.minecraftservices.com/minecraft/profile")
            .bearer_auth(&token)
            .send()?,
        "Minecraft 프로필",
    )?;
    if cancel.load(Ordering::Relaxed) {
        bail!("로그인을 취소했습니다");
    }
    Ok(Session {
        name: string(&profile, "name")?,
        uuid: string(&profile, "id")?,
        token,
        expires: Instant::now()
            + Duration::from_secs(mc["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_application_id_before_network_requests() {
        assert_eq!(
            validate_client_id(" 00001111-aaaa-2222-bbbb-3333cccc4444\n").unwrap(),
            "00001111-aaaa-2222-bbbb-3333cccc4444"
        );
        for invalid in [
            "",
            "myapp://auth",
            "client-secret",
            "00001111-aaaa-2222-bbbb-3333cccc444z",
        ] {
            assert!(validate_client_id(invalid).is_err());
        }
    }

    #[test]
    fn authentication_errors_show_stage_without_leaking_response_data() {
        let error = service_error(
            "Microsoft 코드 발급",
            StatusCode::UNAUTHORIZED,
            &json!({"error":"invalid_client","error_description":"sensitive details","access_token":"secret-token"}),
        );
        assert!(
            error.contains("401") && error.contains("invalid_client") && error.contains("앱 ID")
        );
        assert!(!error.contains("sensitive") && !error.contains("secret-token"));
        let profile = service_error("Minecraft 프로필", StatusCode::NOT_FOUND, &json!({}));
        assert!(profile.contains("소유권"));
        let api = service_error("Minecraft 인증", StatusCode::FORBIDDEN, &json!({}));
        assert!(api.contains("MINECRAFT_FORBIDDEN"));
    }

    #[test]
    fn distinguishes_unapproved_app_account_restriction_and_gateway_errors() {
        let (reason, _) = minecraft_rejection(
            &json!({"errorMessage":"Invalid app registration, see https://aka.ms/AppRegInfo for more information","access_token":"secret"}),
        );
        assert_eq!(reason, "INVALID_APP_REGISTRATION");
        let (reason, _) = minecraft_rejection(&json!({"details":{"reason":"ACCOUNT_SUSPENDED"}}));
        assert_eq!(reason, "ACCOUNT_SUSPENDED");
        let (reason, hint) = minecraft_rejection(&Value::Null);
        assert_eq!(reason, "NON_JSON_RESPONSE");
        assert!(hint.contains("확인되지"));
        let (reason, hint) = minecraft_rejection(&json!({"errorMessage":"unrecognized error"}));
        assert_eq!(reason, "MINECRAFT_FORBIDDEN");
        assert!(hint.contains("확인되지"));
    }

    #[test]
    #[ignore = "requests a device code using the locally configured application ID"]
    fn configured_device_code() -> Result<()> {
        let settings = crate::settings::Settings::load()?;
        let device = request_device_code(&auth_client()?, &settings.microsoft_client_id)?;
        anyhow::ensure!(!device.user_code.is_empty(), "user code missing");
        println!("Microsoft device-code request succeeded; no account credentials were submitted.");
        Ok(())
    }

    #[test]
    #[ignore = "requires the account owner to complete Microsoft login in their browser"]
    fn configured_account_login() -> Result<()> {
        let settings = crate::settings::Settings::load()?;
        let (tx, rx) = async_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = std::thread::spawn(move || login(&settings.microsoft_client_id, &tx, cancel));
        while let Ok(event) = rx.recv_blocking() {
            if let Event::DeviceCode(code, uri) = event {
                println!("USER_LOGIN_URL={uri}\nUSER_LOGIN_CODE={code}");
            }
        }
        let _session = worker
            .join()
            .map_err(|_| anyhow::anyhow!("login worker failed"))??;
        println!("Microsoft, Xbox Live, XSTS and Minecraft profile login succeeded.");
        Ok(())
    }
}
