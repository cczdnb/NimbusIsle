//! Windows 待办：用 OAuth 2.0 + PKCE 本地回环取得 Microsoft Graph 令牌。
//! 刷新令牌存放在 Windows 凭据管理器，不进明文配置。

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SERVICE: &str = "Daozai.TodoSync";
const SCOPES: &str = "User.Read Tasks.ReadWrite offline_access";
const AUTH: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

fn looks_like_guid(s: &str) -> bool {
    let s = s.trim();
    let mut parts = s.split('-');
    let lens = [8, 4, 4, 4, 12];
    for len in lens {
        let p = match parts.next() { Some(p) => p, None => return false };
        if p.len() != len || !p.chars().all(|c| c.is_ascii_hexdigit()) { return false; }
    }
    parts.next().is_none()
}

fn slot(client_id: &str) -> String {
    format!("oauth-windows-{}", client_id.trim().to_ascii_lowercase())
}

#[cfg(windows)]
fn cred_save(client_id: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, &slot(client_id))
        .map_err(|_| "无法打开 Windows 凭据管理器".to_string())?;
    entry.set_password(value).map_err(|_| "无法保存微软登录".to_string())
}

#[cfg(not(windows))]
fn cred_save(_client_id: &str, _value: &str) -> Result<(), String> {
    Err("当前系统不支持保存微软登录".into())
}

#[cfg(windows)]
fn cred_load(client_id: &str) -> Result<String, String> {
    let entry = keyring::Entry::new(SERVICE, &slot(client_id))
        .map_err(|_| "无法打开 Windows 凭据管理器".to_string())?;
    entry.get_password().map_err(|e| match e {
        keyring::Error::NoEntry => "尚未登录微软账号".into(),
        _ => "暂时无法读取 Windows 凭据".into(),
    })
}

#[cfg(not(windows))]
fn cred_load(_client_id: &str) -> Result<String, String> {
    Err("当前系统不支持读取微软登录".into())
}

#[derive(Clone, Debug)]
struct Stored {
    access: String,
    refresh: String,
    expires_at: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn parse_stored(raw: &str) -> Option<Stored> {
    let v: Value = serde_json::from_str(raw).ok()?;
    Some(Stored {
        access: v.get("access")?.as_str()?.to_string(),
        refresh: v.get("refresh")?.as_str()?.to_string(),
        expires_at: v.get("exp")?.as_u64().unwrap_or(0),
    })
}

fn save_tokens(client_id: &str, access: &str, refresh: &str, expires_in: u64) -> Result<(), String> {
    let stored = serde_json::json!({
        "access": access,
        "refresh": refresh,
        "exp": now() + expires_in.saturating_sub(60),
    });
    cred_save(client_id, &stored.to_string())
}

fn refresh_token(client_id: &str, refresh: &str) -> Result<Stored, String> {
    let body = form(&[
        ("client_id", client_id),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("scope", SCOPES),
    ]);
    let doc = token_request(&body)?;
    let access = doc.get("access_token").and_then(|v| v.as_str()).ok_or("刷新令牌响应没有 access_token")?;
    let new_refresh = doc.get("refresh_token").and_then(|v| v.as_str()).unwrap_or(refresh);
    let expires = doc.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    save_tokens(client_id, access, new_refresh, expires)?;
    Ok(Stored { access: access.to_string(), refresh: new_refresh.to_string(), expires_at: now() + expires.saturating_sub(60) })
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", url::form_urlencoded::byte_serialize(k.as_bytes()).collect::<String>(), url::form_urlencoded::byte_serialize(v.as_bytes()).collect::<String>())).collect::<Vec<_>>().join("&")
}

fn token_request(body: &str) -> Result<Value, String> {
    let resp = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .user_agent(&format!("Daozai/{} (Windows; TodoSync)", env!("CARGO_PKG_VERSION")))
        .build()
        .post(TOKEN)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(body);
    match resp {
        Ok(r) => {
            let t = r.into_string().map_err(|e| format!("读取令牌响应失败：{e}"))?;
            serde_json::from_str(&t).map_err(|e| format!("令牌响应无法解析：{e}"))
        }
        Err(ureq::Error::Status(code, r)) => {
            let t = r.into_string().unwrap_or_default();
            Err(format!("获取令牌失败 HTTP {code}：{}", t.chars().take(160).collect::<String>()))
        }
        Err(ureq::Error::Transport(e)) => Err(format!("获取令牌网络错误：{e}")),
    }
}

/// 取得可用的 Graph access token。`interactive` 为 true 时允许弹出浏览器登录。
pub fn acquire_token(client_id: &str, interactive: bool) -> Result<String, String> {
    let client_id = client_id.trim();
    if client_id.is_empty() || !looks_like_guid(client_id) {
        return Err("请先粘贴 Azure 应用 Client ID（公共客户端 / 移动和桌面，需支持个人微软账户，并添加 http://localhost 回环重定向）".into());
    }
    if let Ok(raw) = cred_load(client_id) {
        if let Some(stored) = parse_stored(&raw) {
            if !stored.access.is_empty() && stored.expires_at > now() + 30 {
                return Ok(stored.access);
            }
            if !stored.refresh.is_empty() {
                match refresh_token(client_id, &stored.refresh) {
                    Ok(s) => return Ok(s.access),
                    Err(e) => {
                        if !interactive { return Err(format!("登录已过期，请在设置里重新登录：{e}")); }
                    }
                }
            }
        }
    }
    if !interactive {
        return Err("已填写 Client ID，但尚未授权；请点「登录并同步」完成一次浏览器授权（定时同步不会弹窗）".into());
    }
    interactive_login(client_id)
}

fn pkce_pair() -> (String, String) {
    use base64::Engine;
    let mut raw = [0u8; 32];
    // 不用额外随机库：用时间与地址搅乱，再用 ring。
    let seed = format!("{:?}{:?}", SystemTime::now(), raw.as_ptr() as usize);
    let digest = ring::digest::digest(&ring::digest::SHA256, seed.as_bytes());
    raw.copy_from_slice(&digest.as_ref()[..32]);
    // 再混一次进程时间
    let digest2 = ring::digest::digest(&ring::digest::SHA256, &raw);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest2.as_ref()[..32]);
    let challenge_digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(challenge_digest.as_ref());
    (verifier, challenge)
}

fn interactive_login(client_id: &str) -> Result<String, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("无法在本机打开回调端口：{e}"))?;
    listener.set_nonblocking(false).ok();
    let port = listener.local_addr().map_err(|e| format!("无法读取回调端口：{e}"))?.port();
    let redirect = format!("http://localhost:{port}");
    let (verifier, challenge) = pkce_pair();
    let url = format!(
        "{AUTH}?client_id={}&response_type=code&redirect_uri={}&response_mode=query&scope={}&code_challenge={}&code_challenge_method=S256&prompt=select_account",
        url_enc(client_id), url_enc(&redirect), url_enc(SCOPES), url_enc(&challenge)
    );
    open_browser(&url)?;
    let code = wait_for_code(&listener)?;
    let body = form(&[
        ("client_id", client_id),
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", &redirect),
        ("code_verifier", &verifier),
        ("scope", SCOPES),
    ]);
    let doc = token_request(&body)?;
    let access = doc.get("access_token").and_then(|v| v.as_str()).ok_or("授权响应没有 access_token")?;
    let refresh = doc.get("refresh_token").and_then(|v| v.as_str()).unwrap_or("");
    if refresh.is_empty() {
        return Err("授权成功但没有刷新令牌，请确认应用启用了 public client 与 offline_access".into());
    }
    let expires = doc.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(3600);
    save_tokens(client_id, access, refresh, expires)?;
    Ok(access.to_string())
}

fn url_enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("无法打开浏览器：{e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("当前系统无法打开浏览器完成登录".into())
    }
}

fn wait_for_code(listener: &TcpListener) -> Result<String, String> {
    listener.set_nonblocking(true).ok();
    let start = std::time::Instant::now();
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(pair) => break pair,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => {
                if start.elapsed() > Duration::from_secs(180) {
                    return Err("登录已取消或超时，请重试".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(_) => return Err("登录已取消或超时，请重试".into()),
        }
    };
    stream.set_nonblocking(false).ok();
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let req = String::from_utf8_lossy(&buf[..n]);
    let first = req.lines().next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("");
    let qs = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut error = None;
    let mut error_desc = None;
    for (k, v) in url::form_urlencoded::parse(qs.as_bytes()) {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            "error_description" => error_desc = Some(v.into_owned()),
            _ => {}
        }
    }
    let html = if code.is_some() {
        "<!doctype html><meta charset=utf-8><title>岛仔</title><body style='font-family:sans-serif;padding:40px'>已完成微软账号授权，可以关闭此页，返回岛仔。</body>"
    } else {
        "<!doctype html><meta charset=utf-8><title>岛仔</title><body style='font-family:sans-serif;padding:40px'>授权未完成，可以关闭此页，返回岛仔重试。</body>"
    };
    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
    if let Some(c) = code { return Ok(c); }
    let why = error_desc.or(error).unwrap_or_else(|| "未返回授权码".into());
    Err(format!("微软账号授权失败：{}", why.replace('+', " ")))
}

pub fn has_saved_login(client_id: &str) -> bool {
    let client_id = client_id.trim();
    if client_id.is_empty() { return false; }
    cred_load(client_id).ok().and_then(|r| parse_stored(&r)).is_some_and(|s| !s.refresh.is_empty() || !s.access.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_check_accepts_standard_client_id() {
        assert!(looks_like_guid("01234567-89ab-cdef-0123-456789abcdef"));
        assert!(!looks_like_guid("not-a-guid"));
        assert!(!looks_like_guid(""));
    }

    #[test]
    fn pkce_verifier_and_challenge_are_urlsafe() {
        let (v, c) = pkce_pair();
        assert!(v.len() >= 32 && c.len() >= 32);
        assert!(!v.contains('+') && !v.contains('/'));
        assert!(!c.contains('+') && !c.contains('/'));
        assert_ne!(v, c);
    }
}
