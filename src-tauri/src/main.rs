#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Datelike, Local, TimeZone};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ---------- paths ----------

fn roaming() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join("AppData").join("Roaming"))
}
fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_home().join("AppData").join("Local"))
}
fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default()
}
fn db_path() -> PathBuf {
    let cands = [
        roaming().join("devin").join("cli").join("sessions.db"),
        dirs_home().join(".config").join("devin").join("cli").join("sessions.db"),
    ];
    cands
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| cands[0].clone())
}

// ---------- minimal protobuf reader ----------

enum F {
    V(u64),
    B(Vec<u8>),
}

fn read_varint(buf: &[u8], mut i: usize) -> (u64, usize) {
    let mut v: u64 = 0;
    let mut s = 0u32;
    while i < buf.len() {
        let b = buf[i];
        i += 1;
        v |= ((b & 0x7f) as u64) << s;
        if b & 0x80 == 0 {
            break;
        }
        s += 7;
    }
    (v, i)
}

fn proto_fields(buf: &[u8]) -> Vec<(u32, F)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let (tag, ni) = read_varint(buf, i);
        i = ni;
        let f = (tag >> 3) as u32;
        match tag & 7 {
            0 => {
                let (v, n2) = read_varint(buf, i);
                i = n2;
                out.push((f, F::V(v)));
            }
            2 => {
                let (len, n2) = read_varint(buf, i);
                i = n2;
                let end = (i + len as usize).min(buf.len());
                out.push((f, F::B(buf[i..end].to_vec())));
                i = end;
            }
            1 => {
                if i + 8 > buf.len() {
                    break;
                }
                i += 8;
            }
            5 => {
                if i + 4 > buf.len() {
                    break;
                }
                i += 4;
            }
            _ => break,
        }
    }
    out
}

fn fnum(fs: &[(u32, F)], n: u32) -> Option<u64> {
    fs.iter().find_map(|(f, v)| match (f, v) {
        (x, F::V(v)) if *x == n => Some(*v),
        _ => None,
    })
}
fn fsub(fs: &[(u32, F)], n: u32) -> Option<Vec<(u32, F)>> {
    fs.iter().find_map(|(f, v)| match (f, v) {
        (x, F::B(b)) if *x == n => Some(proto_fields(b)),
        _ => None,
    })
}
fn fstr(fs: &[(u32, F)], n: u32) -> Option<String> {
    fs.iter().find_map(|(f, v)| match (f, v) {
        (x, F::B(b)) if *x == n => String::from_utf8(b.clone()).ok(),
        _ => None,
    })
}

// ---------- quota (cached user_status.*.bin written by Devin CLI) ----------

fn read_quota() -> Value {
    (|| -> Option<Value> {
        let dir = local_appdata().join("devin").join("cli");
        let file = fs::read_dir(&dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("user_status.") && n.ends_with(".bin"))
            .max()?;
        let outer: Value = serde_json::from_str(&fs::read_to_string(dir.join(file)).ok()?).ok()?;
        let payload = B64.decode(outer["payload"].as_str()?).ok()?;
        let top = proto_fields(&payload);
        let ps = fsub(&top, 13)?; // PlanStatus
        let plan_info = fsub(&ps, 1).unwrap_or_default();
        let ts = |m: &Option<Vec<(u32, F)>>| {
            m.as_ref().and_then(|x| fnum(x, 1)).unwrap_or(0) * 1000
        };
        Some(json!({
            "fetchedAt": outer["fetched_at_secs"].as_u64().unwrap_or(0) * 1000,
            "user": fstr(&top, 3).unwrap_or_default(),
            "email": fstr(&top, 7).unwrap_or_default(),
            "plan": fstr(&plan_info, 2).unwrap_or_default(),
            "planStart": ts(&fsub(&ps, 2)),
            "planEnd": ts(&fsub(&ps, 3)),
            // PlanStatus fields per CLI binary schema:
            //   14 daily_quota_remaining_percent, 15 weekly_quota_remaining_percent,
            //   17 daily_quota_reset_at_unix,    18 weekly_quota_reset_at_unix
            "dailyRemainingPct": fnum(&ps, 14).map(|v| v as i64).unwrap_or(-1),
            "weeklyRemainingPct": fnum(&ps, 15).map(|v| v as i64).unwrap_or(-1),
            "dailyResetAt": fnum(&ps, 17).unwrap_or(0) * 1000,
            "weeklyResetAt": fnum(&ps, 18).unwrap_or(0) * 1000,
        }))
    })()
    .unwrap_or(Value::Null)
}

// ---------- session token (Devin Desktop, Electron SafeStorage in state.vscdb) ----------

#[repr(C)]
struct CryptBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptUnprotectData(
        p_in: *const CryptBlob,
        name: *const u16,
        entropy: *const CryptBlob,
        reserved: *const u8,
        prompt: *const u8,
        flags: u32,
        p_out: *mut CryptBlob,
    ) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(h: *mut u8) -> *mut u8;
}

fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let bin = CryptBlob {
            cb_data: input.len() as u32,
            pb_data: input.as_ptr() as *mut u8,
        };
        let mut out = CryptBlob {
            cb_data: 0,
            pb_data: std::ptr::null_mut(),
        };
        if CryptUnprotectData(
            &bin,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            &mut out,
        ) == 0
        {
            return Err("CryptUnprotectData failed".into());
        }
        let data = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
        LocalFree(out.pb_data);
        Ok(data)
    }
}

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use sha2::Digest as _;

// windsurf_auth.sessions in state.vscdb is an Electron SafeStorage secret:
// AES-256-GCM("v10" + nonce(12) + ct+tag), master key = DPAPI(os_crypt.encrypted_key)
fn get_session_token() -> Option<String> {
    (|| -> Option<String> {
        let ls: Value =
            serde_json::from_str(&fs::read_to_string(roaming().join("devin").join("Local State")).ok()?)
                .ok()?;
        let key_blob = B64.decode(ls["os_crypt"]["encrypted_key"].as_str()?).ok()?;
        let master = dpapi_unprotect(&key_blob[5..]).ok()?; // strip "DPAPI" prefix
        let conn = Connection::open_with_flags(
            roaming().join("devin").join("User").join("globalStorage").join("state.vscdb"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()?;
        let row: Option<String> = conn
            .query_row(
                "SELECT value FROM ItemTable WHERE key LIKE '%windsurf_auth.sessions%'",
                [],
                |r| r.get(0),
            )
            .ok();
        drop(conn);
        let enc_v: Value = serde_json::from_str(&row?).ok()?;
        let enc: Vec<u8> = enc_v["data"]
            .as_array()?
            .iter()
            .filter_map(|v| v.as_u64().map(|x| x as u8))
            .collect();
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&master));
        let plain = cipher
            .decrypt(Nonce::from_slice(&enc[3..15]), &enc[15..])
            .ok()?;
        let sessions: Value = serde_json::from_slice(&plain).ok()?;
        sessions[0]["accessToken"].as_str().map(String::from)
    })()
}

// ---------- account store + PKCE login (same flow as `devin auth login`) ----------
// %APPDATA%\devin-usage\accounts.json: {"accounts":[{id,label,email,userId,token(b64 dpapi),addedAt}],"activeId":"desktop"|id}
// "desktop" = follow whatever account Devin itself is logged in with.

fn store_path() -> PathBuf {
    roaming().join("devin-usage").join("accounts.json")
}

fn load_store() -> Value {
    fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|j| j["accounts"].is_array())
        .unwrap_or_else(|| json!({"accounts": [], "activeId": "desktop"}))
}

fn save_store(s: &Value) {
    if let Some(dir) = store_path().parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(store_path(), serde_json::to_string_pretty(s).unwrap());
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        p_in: *const CryptBlob,
        name: *const u16,
        entropy: *const CryptBlob,
        reserved: *const u8,
        prompt: *const u8,
        flags: u32,
        p_out: *mut CryptBlob,
    ) -> i32;
}

fn dpapi_protect(input: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let bin = CryptBlob {
            cb_data: input.len() as u32,
            pb_data: input.as_ptr() as *mut u8,
        };
        let mut out = CryptBlob { cb_data: 0, pb_data: std::ptr::null_mut() };
        if CryptProtectData(&bin, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out) == 0 {
            return Err("CryptProtectData failed".into());
        }
        let data = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
        LocalFree(out.pb_data);
        Ok(data)
    }
}

fn account_token(a: &Value) -> Option<String> {
    let enc = B64.decode(a["token"].as_str()?).ok()?;
    let plain = dpapi_unprotect(&enc).ok()?;
    String::from_utf8(plain).ok()
}

fn account_list_json() -> Value {
    let s = load_store();
    let active = s["activeId"].as_str().unwrap_or("desktop");
    let mut list = vec![json!({"id": "desktop", "label": "跟随 Devin 当前登录", "builtin": true, "active": active == "desktop"})];
    for a in s["accounts"].as_array().unwrap_or(&vec![]) {
        list.push(json!({
            "id": a["id"], "label": a["label"], "email": a["email"],
            "builtin": false, "active": a["id"].as_str() == Some(active),
        }));
    }
    json!(list)
}

fn resolve_token() -> Option<(String, &'static str)> {
    let s = load_store();
    let active = s["activeId"].as_str().unwrap_or("desktop");
    if active != "desktop" {
        if let Some(acc) = s["accounts"].as_array()?.iter().find(|a| a["id"].as_str() == Some(active)) {
            if let Some(t) = account_token(acc) {
                return Some((t, "account"));
            }
        }
    }
    get_session_token().map(|t| (t, "desktop"))
}

// token resolution touches the filesystem + DPAPI — cache briefly so the
// 2-second stats poll doesn't hammer the vscdb; account changes bump the gen.
static TOKEN_CACHE: std::sync::Mutex<Option<(u64, std::time::Instant, Option<(String, &'static str)>)>> =
    std::sync::Mutex::new(None);
fn resolve_token_cached() -> Option<(String, &'static str)> {
    let gen = TOKEN_RESET.load(std::sync::atomic::Ordering::SeqCst);
    let mut c = TOKEN_CACHE.lock().unwrap();
    let stale = c
        .as_ref()
        .map(|(g, t, r)| *g != gen || r.is_none() || t.elapsed().as_secs() > 30)
        .unwrap_or(true);
    if stale {
        *c = Some((gen, std::time::Instant::now(), resolve_token()));
    }
    c.as_ref().and_then(|(_, _, r)| r.clone())
}

// PKCE helpers — same flow as `devin auth login`
// default mode: loopback listener on 127.0.0.1 + redirect_uri (auto-capture)
// manual mode (--force-manual-token-flow equivalent): no redirect_uri, page shows a code to paste
struct PendingAuth {
    verifier: String,
    code: Option<String>,
}
static PENDING_PKCE: std::sync::LazyLock<std::sync::Mutex<HashMap<String, PendingAuth>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn pct_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

const OAUTH_OK_HTML: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
<html><body style='font-family:sans-serif;text-align:center;padding:50px;background:#0d1117;color:#e6edf3'>\
<h1 style='color:#3fb950'>登录成功</h1><p>可以关闭此页面，返回 Devin 用量。</p>\
<script>setTimeout(function(){window.close()},2000)</script></body></html>";
const OAUTH_FAIL_HTML: &str = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
<html><body style='font-family:sans-serif;text-align:center;padding:50px'>\
<h1 style='color:#f85149'>登录失败</h1><p>请返回应用重试。</p></body></html>";

fn spawn_callback_listener(listener: std::net::TcpListener, state: String) {
    use std::io::{Read, Write};
    std::thread::spawn(move || {
        let _ = listener.set_nonblocking(true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if std::time::Instant::now() > deadline {
                break;
            }
            // entry removed by auth_cancel / auth_complete / auth_poll → stop listening
            if !PENDING_PKCE.lock().unwrap().contains_key(&state) {
                break;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
                    let mut buf = [0u8; 8192];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("")
                        .to_string();
                    if !path.starts_with("/callback") {
                        let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        continue;
                    }
                    let query = path.split('?').nth(1).unwrap_or("");
                    let mut code = None;
                    let mut got_state = None;
                    for kv in query.split('&') {
                        if let Some((k, v)) = kv.split_once('=') {
                            match k {
                                "code" => code = Some(v.to_string()),
                                "state" => got_state = Some(v.to_string()),
                                _ => {}
                            }
                        }
                    }
                    let ok = got_state.as_deref() == Some(state.as_str()) && code.is_some();
                    let _ = stream.write_all(if ok { OAUTH_OK_HTML.as_bytes() } else { OAUTH_FAIL_HTML.as_bytes() });
                    let _ = stream.flush();
                    if let Some(c) = code.filter(|_| ok) {
                        if let Some(p) = PENDING_PKCE.lock().unwrap().get_mut(&state) {
                            p.code = Some(c);
                        }
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
                Err(_) => break,
            }
        }
    });
}

#[tauri::command]
fn auth_start(manual: Option<bool>) -> Result<Value, String> {
    let mut verifier_raw = [0u8; 48];
    getrandom::getrandom(&mut verifier_raw).map_err(|e| e.to_string())?;
    let verifier = b64url(&verifier_raw);
    let challenge = b64url(&sha2::Sha256::digest(verifier.as_bytes()));
    let state = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());

    let mut is_manual = manual.unwrap_or(false);
    let mut redirect_uri = String::new();
    if !is_manual {
        match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => {
                let port = l.local_addr().map_err(|e| e.to_string())?.port();
                redirect_uri = format!("http://127.0.0.1:{port}/callback");
                spawn_callback_listener(l, state.clone());
            }
            Err(_) => is_manual = true, // no loopback available → paste-code mode
        }
    }

    PENDING_PKCE.lock().unwrap().insert(
        state.clone(),
        PendingAuth { verifier, code: None },
    );
    let mut url = format!(
        "https://app.devin.ai/auth/cli/continue?state={state}&prompt=select_account&code_challenge={challenge}&code_challenge_method=S256&cli_pkce_marker=1"
    );
    if !is_manual {
        url = format!(
            "https://app.devin.ai/auth/cli/continue?redirect_uri={}&state={state}&prompt=select_account&code_challenge={challenge}&code_challenge_method=S256&cli_pkce_marker=1",
            pct_encode(&redirect_uri)
        );
    }
    Ok(json!({"state": state, "url": url, "manual": is_manual}))
}

// exchange a PKCE code for a session token + persist as an account
// (request shapes verified against the official Devin CLI binary)
fn exchange_pkce(code: &str, verifier: &str) -> Result<Value, String> {
    let token = (|| -> Result<String, String> {
        // primary: POST api.devin.ai/auth/cli/token {"code","code_verifier"} -> {"token"}
        let a = http_agent()
            .post("https://api.devin.ai/auth/cli/token")
            .set("Content-Type", "application/json")
            .set("Accept", "application/json")
            .send_json(json!({"code": code.trim(), "code_verifier": verifier}))
            .ok()
            .and_then(|r| r.into_json::<Value>().ok());
        if let Some(t) = a.as_ref().and_then(|j| j["token"].as_str()) {
            return Ok(t.to_string());
        }
        // fallback: Connect-RPC exchange on the reasoning backend
        let b = http_agent()
            .post("https://server.codeium.com/exa.seat_management_pb.SeatManagementService/ExchangeDevinCLIPKCECode")
            .set("Content-Type", "application/json")
            .set("Connect-Protocol-Version", "1")
            .send_json(json!({"code": code.trim(), "codeVerifier": verifier}))
            .ok()
            .and_then(|r| r.into_json::<Value>().ok());
        b.and_then(|j| {
            for k in ["sessionToken", "session_token", "apiKey", "token"] {
                if let Some(t) = j[k].as_str() {
                    return Some(t.to_string());
                }
            }
            None
        })
        .ok_or_else(|| "授权码交换失败".to_string())
    })()?;
    // Devin stores session tokens as `devin-session-token$<jwt>` — normalize
    let token = if token.starts_with("eyJ") {
        format!("devin-session-token${token}")
    } else {
        token
    };

    // identity: api.devin.ai/v3/self (Bearer) -> {user_name,user_id,org_id}; fallback GetUserStatus
    let self_prof = http_agent()
        .get("https://api.devin.ai/v3/self")
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call()
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .unwrap_or(Value::Null);
    let status = fetch_status_raw(&token).unwrap_or(Value::Null);
    let us = &status["userStatus"];
    let label = self_prof["user_name"].as_str().filter(|s| !s.is_empty())
        .or_else(|| us["name"].as_str())
        .unwrap_or("account").to_string();
    let email = us["email"].as_str().unwrap_or("").to_string();
    let user_id = self_prof["user_id"].as_str().filter(|s| !s.is_empty())
        .or_else(|| us["userId"].as_str())
        .unwrap_or("").to_string();

    let mut s = load_store();
    let id = format!("acc-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 0xFFFFFFFF);
    if let Some(arr) = s["accounts"].as_array_mut() {
        arr.retain(|a| a["label"].as_str() != Some(label.as_str()));
        arr.push(json!({
            "id": id, "label": label, "email": email, "userId": user_id,
            "token": B64.encode(dpapi_protect(token.as_bytes()).map_err(|e| e)?),
            "addedAt": chrono::Local::now().timestamp_millis(),
        }));
    }
    s["activeId"] = json!(id);
    save_store(&s);
    bump_token();
    Ok(json!({"id": id, "label": label}))
}

#[tauri::command]
fn auth_complete(state: String, code: String) -> Result<Value, String> {
    let p = PENDING_PKCE
        .lock()
        .unwrap()
        .remove(&state)
        .ok_or("登录会话已过期，请重新开始")?;
    exchange_pkce(&code, &p.verifier)
}

// loopback mode: the frontend polls this until the browser callback delivers the code
#[tauri::command]
fn auth_poll(state: String) -> Result<Value, String> {
    let taken = {
        let mut m = PENDING_PKCE.lock().unwrap();
        match m.get(&state) {
            None => return Err("登录会话已过期，请重新开始".into()),
            Some(p) => match p.code.clone() {
                None => return Ok(json!({"done": false})),
                Some(_) => m.remove(&state).unwrap(),
            },
        }
    };
    let r = exchange_pkce(&taken.code.unwrap(), &taken.verifier)?;
    let mut r = r;
    r["done"] = json!(true);
    Ok(r)
}

#[tauri::command]
fn auth_cancel(state: String) {
    PENDING_PKCE.lock().unwrap().remove(&state);
}

// write the selected account into Devin Desktop's own session store so Devin switches too
fn sync_devin_session(token: &str, label: &str, user_id: &str) -> Result<(), String> {
    let master = (|| -> Option<Vec<u8>> {
        let ls: Value =
            serde_json::from_str(&fs::read_to_string(roaming().join("devin").join("Local State")).ok()?).ok()?;
        let key_blob = B64.decode(ls["os_crypt"]["encrypted_key"].as_str()?).ok()?;
        dpapi_unprotect(&key_blob[5..]).ok()
    })()
    .ok_or("no master key")?;
    let vscdb = roaming().join("devin").join("User").join("globalStorage").join("state.vscdb");
    let conn = Connection::open(&vscdb).map_err(|e| e.to_string())?;
    let key_row: Option<String> = conn
        .query_row(
            "SELECT key FROM ItemTable WHERE key LIKE '%windsurf_auth.sessions%'",
            [],
            |r| r.get(0),
        )
        .ok();
    let key = key_row.ok_or("no sessions key")?;
    let sessions = json!([{
        "id": format!("{:x}-{:x}-{:x}-{:x}-{:x}",
            fastrand_u32(), fastrand_u32(), fastrand_u32(), fastrand_u32(), fastrand_u32()),
        "accessToken": token,
        "account": {"label": label, "id": user_id},
        "scopes": [],
    }]);
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|e| e.to_string())?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&master));
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), sessions.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let mut blob = b"v10".to_vec();
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    let v = json!({"data": blob});
    conn.execute("UPDATE ItemTable SET value = ?1 WHERE key = ?2", rusqlite::params![v.to_string(), key])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn fastrand_u32() -> u32 {
    let mut b = [0u8; 4];
    let _ = getrandom::getrandom(&mut b);
    u32::from_le_bytes(b)
}

// re-resolve token + clear network caches after any account change
static TOKEN_RESET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn bump_token() {
    TOKEN_RESET.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut c) = ONLINE_CACHE.lock() {
        *c = None;
    }
    if let Ok(mut c) = ACCT_STATUS_CACHE.lock() {
        c.clear();
    }
}

#[tauri::command]
fn auth_accounts() -> Value {
    account_list_json()
}

#[tauri::command]
fn auth_use(id: String) -> Result<Value, String> {
    let mut s = load_store();
    if id != "desktop" && !s["accounts"].as_array().map(|a| a.iter().any(|x| x["id"].as_str() == Some(id.as_str()))).unwrap_or(false) {
        return Err("账号不存在".into());
    }
    s["activeId"] = json!(id);
    save_store(&s);
    bump_token();
    Ok(json!({"ok": true}))
}

#[tauri::command]
fn auth_remove(id: String) -> Result<Value, String> {
    let mut s = load_store();
    if let Some(arr) = s["accounts"].as_array_mut() {
        arr.retain(|a| a["id"].as_str() != Some(&id));
    }
    if s["activeId"].as_str() == Some(id.as_str()) {
        s["activeId"] = json!("desktop");
    }
    save_store(&s);
    bump_token();
    Ok(json!({"ok": true}))
}

// also flip Devin itself to the selected account (requires Devin restart to take effect)
#[tauri::command]
fn auth_sync_devin(id: String) -> Result<Value, String> {
    let s = load_store();
    let acc = s["accounts"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"].as_str() == Some(id.as_str())))
        .ok_or("账号不存在")?;
    let token = account_token(acc).ok_or("token 损坏")?;
    let label = acc["label"].as_str().unwrap_or("").to_string();
    let user_id = acc["userId"].as_str().unwrap_or("").to_string();
    sync_devin_session(&token, &label, &user_id)?;
    Ok(json!({"ok": true, "note": "已写入 Devin 登录态，重启 Devin 生效"}))
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    // rundll32 avoids cmd's & splitting in query strings
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// per-account quota snapshots for the 账号管理 page
static ACCT_STATUS_CACHE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, Value)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

fn account_status(id: &str) -> Value {
    if let Some((at, v)) = ACCT_STATUS_CACHE.lock().unwrap().get(id) {
        if at.elapsed().as_secs() < 60 {
            return v.clone();
        }
    }
    let s = load_store();
    let token = if id == "desktop" {
        get_session_token()
    } else {
        s["accounts"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["id"].as_str() == Some(id)))
            .and_then(account_token)
    };
    let data = match token {
        None => json!({"error": "无登录态"}),
        Some(t) => match fetch_quota_online(&t) {
            Some(q) => {
                let mut q = q;
                q["fetchedAt"] = json!(chrono::Local::now().timestamp_millis());
                q
            }
            None => json!({"error": "请求失败"}),
        },
    };
    ACCT_STATUS_CACHE.lock().unwrap().insert(id.to_string(), (std::time::Instant::now(), data.clone()));
    data
}

#[tauri::command]
fn accounts_status(id: Option<String>) -> Value {
    if let Some(id) = id {
        ACCT_STATUS_CACHE.lock().unwrap().remove(&id);
    }
    let list = account_list_json();
    let handles: Vec<_> = list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|a| {
            let aid = a["id"].as_str().unwrap_or("").to_string();
            std::thread::spawn(move || (a, account_status(&aid)))
        })
        .collect();
    let out: Vec<Value> = handles
        .into_iter()
        .filter_map(|h| h.join().ok())
        .map(|(mut a, st)| {
            a["status"] = st;
            a
        })
        .collect();
    json!(out)
}

// ---------- network: quota + usage (the APIs windsurf.com/profile uses) ----------

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .build()
}

fn proxy_call(token: &str, method: &str, body: Value) -> Result<Value, String> {
    let url = format!("https://windsurf.com/api/backend/{method}");
    http_agent()
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("x-auth-token", token)
        .set("Origin", "https://windsurf.com")
        .set("Referer", "https://windsurf.com/profile")
        .send_json(body)
        .map_err(|e| format!("{method}: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("{method} json: {e}"))
}

fn fetch_status_raw(token: &str) -> Result<Value, String> {
    let body = json!({
        "metadata": {
            "apiKey": token, "ideName": "devin-cli", "ideVersion": "3000.10.23",
            "extensionName": "devin-cli", "extensionVersion": "3000.10.23", "locale": "en-US",
        }
    });
    http_agent()
        .post("https://server.codeium.com/exa.seat_management_pb.SeatManagementService/GetUserStatus")
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .send_json(body)
        .map_err(|e| format!("GetUserStatus: {e}"))?
        .into_json::<Value>()
        .map_err(|e| format!("GetUserStatus json: {e}"))
}

// protobuf-JSON encodes numbers as strings — accept both
fn jnum(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        .unwrap_or(0)
}
fn jf64(v: &Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
        .unwrap_or(0.0)
}
fn iso_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn quota_from_status(j: &Value) -> Option<Value> {
    let ps = j["userStatus"]["planStatus"].as_object()?;
    let iso = |s: &str| iso_ms(s);
    Some(json!({
        "fetchedAt": chrono::Local::now().timestamp_millis(),
        "live": true,
        "user": j["userStatus"]["name"].as_str().unwrap_or(""),
        "email": j["userStatus"]["email"].as_str().unwrap_or(""),
        "plan": ps["planInfo"]["planName"].as_str().unwrap_or(""),
        "planStart": iso(ps["planStart"].as_str().unwrap_or("")),
        "planEnd": iso(ps["planEnd"].as_str().unwrap_or("")),
        "dailyRemainingPct": jnum(&ps["dailyQuotaRemainingPercent"]) as i64,
        "weeklyRemainingPct": jnum(&ps["weeklyQuotaRemainingPercent"]) as i64,
        "dailyResetAt": jnum(&ps["dailyQuotaResetAtUnix"]) as i64 * 1000,
        "weeklyResetAt": jnum(&ps["weeklyQuotaResetAtUnix"]) as i64 * 1000,
    }))
}

fn quota_from_plan_status(j: &Value) -> Option<Value> {
    let ps = j["planStatus"].as_object()?;
    let iso = |s: &str| iso_ms(s);
    Some(json!({
        "fetchedAt": chrono::Local::now().timestamp_millis(),
        "live": true,
        "user": "", "email": "",
        "plan": ps["planInfo"]["planName"].as_str().unwrap_or(""),
        "planStart": iso(ps["planStart"].as_str().unwrap_or("")),
        "planEnd": iso(ps["planEnd"].as_str().unwrap_or("")),
        "dailyRemainingPct": jnum(&ps["dailyQuotaRemainingPercent"]) as i64,
        "weeklyRemainingPct": jnum(&ps["weeklyQuotaRemainingPercent"]) as i64,
        "dailyResetAt": jnum(&ps["dailyQuotaResetAtUnix"]) as i64 * 1000,
        "weeklyResetAt": jnum(&ps["weeklyQuotaResetAtUnix"]) as i64 * 1000,
    }))
}

fn fetch_quota_online(token: &str) -> Option<Value> {
    quota_from_status(&fetch_status_raw(token).ok()?).or_else(|| {
        proxy_call(
            token,
            "exa.seat_management_pb.SeatManagementService/GetPlanStatus",
            json!({"includeTopUpStatus": true}),
        )
        .ok()
        .and_then(|j| quota_from_plan_status(&j))
    })
}

const TOOL_LABELS: &[(&str, &str)] = &[
    ("RUN_COMMAND", "终端命令"),
    ("SEARCH_WEB", "网页搜索"),
    ("VIEW_FILE", "查看文件"),
    ("CODE_ACTION", "代码编辑"),
    ("GREP_SEARCH", "内容搜索"),
    ("ASK_USER_QUESTION", "提问"),
    ("MCP_TOOL", "MCP 调用"),
    ("PROXY_WEB_SERVER", "浏览器预览"),
    ("DEEPWIKI", "DeepWiki"),
    ("FIND", "查找文件"),
    ("SKILL", "技能"),
    ("OTHER", "其他"),
];

fn fetch_usage_online(token: &str) -> Option<Value> {
    let end = chrono::Local::now();
    let start = end - chrono::Duration::seconds(30 * 86400);
    let j = proxy_call(
        token,
        "exa.user_analytics_pb.UserAnalyticsService/GetAnalytics",
        json!({
            "queryRequests": [
                {"cascadeLines": {}},
                {"cascadeToolUsage": {}},
                {"cascadeRuns": {}},
                {"agentUsage": {}},
                {"userPageAnalytics": {}},
            ],
            "startTimestamp": start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "endTimestamp": end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }),
    )
    .ok()?;

    let inner = |key: &str| -> Vec<Value> {
        j["queryResults"]
            .as_array()
            .and_then(|rs| rs.iter().find(|r| r.get(key).is_some()))
            .and_then(|r| r[key][key].as_array().cloned())
            .unwrap_or_default()
    };
    let lines = inner("cascadeLines");
    let tools = inner("cascadeToolUsage");
    let runs = inner("cascadeRuns");
    let agents = inner("agentUsage");

    let mut by_model: HashMap<String, (u64, std::collections::HashSet<String>)> = HashMap::new();
    let mut by_day: HashMap<String, (u64, u64)> = HashMap::new(); // day -> (messages, lines)
    let mut convs = std::collections::HashSet::new();
    let mut messages_sent = 0u64;
    for r in &runs {
        let day = r["day"].as_str().unwrap_or("").to_string();
        let m = jnum(&r["messagesSent"]);
        messages_sent += m;
        let e = by_model
            .entry(r["model"].as_str().unwrap_or("unknown").to_string())
            .or_insert((0, std::collections::HashSet::new()));
        e.0 += m;
        e.1.insert(day.clone());
        convs.insert(r["cascadeId"].as_str().unwrap_or("").to_string());
        by_day.entry(day).or_insert((0, 0)).0 += m;
    }
    let mut lines_accepted = 0u64;
    for l in &lines {
        lines_accepted += jnum(&l["linesAccepted"]);
        by_day
            .entry(l["day"].as_str().unwrap_or("").to_string())
            .or_insert((0, 0))
            .1 += jnum(&l["linesAccepted"]);
    }

    let mut model_list: Vec<Value> = by_model
        .into_iter()
        .map(|(model, (messages, days))| {
            json!({"model": model, "messages": messages, "days": days.len()})
        })
        .collect();
    model_list.sort_by(|a, b| b["messages"].as_u64().cmp(&a["messages"].as_u64()));

    let mut day_list: Vec<Value> = by_day
        .into_iter()
        .map(|(day, (messages, lines))| {
            json!({"day": day.get(..10).unwrap_or(&day), "messages": messages, "lines": lines})
        })
        .collect();
    day_list.sort_by(|a, b| a["day"].as_str().cmp(&b["day"].as_str()));

    let mut tool_list: Vec<Value> = tools
        .iter()
        .map(|t| {
            let key = t["tool"].as_str().unwrap_or("OTHER");
            let label = TOOL_LABELS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| *v)
                .unwrap_or(key);
            json!({"tool": label, "key": key, "count": jnum(&t["count"])})
        })
        .collect();
    tool_list.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));

    let acu_used = agents.iter().map(|a| jf64(&a["acusUsed"])).fold(0.0, f64::max);

    Some(json!({
        "fetchedAt": chrono::Local::now().timestamp_millis(),
        "live": true,
        "windowDays": 30,
        "acuUsed": acu_used,
        "messagesSent": messages_sent,
        "conversations": convs.len(),
        "linesAccepted": lines_accepted,
        "byModel": model_list,
        "byDay": day_list,
        "tools": tool_list,
    }))
}

// background refresher: stats() is polled every 2s by the UI, network runs off-thread
static ONLINE_CACHE: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

fn spawn_online_refresher() {
    std::thread::spawn(|| loop {
        if let Some((token, _src)) = resolve_token_cached() {
            let quota = fetch_quota_online(&token);
            let usage = fetch_usage_online(&token);
            if quota.is_some() || usage.is_some() {
                let mut cache = ONLINE_CACHE.lock().unwrap();
                let entry = cache.get_or_insert_with(|| json!({}));
                if let Some(q) = quota {
                    entry["quota"] = q;
                }
                if let Some(u) = usage {
                    entry["online"] = u;
                }
                entry["fetchedAt"] = json!(chrono::Local::now().timestamp_millis());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(45));
    });
}

fn merge_online(d: &mut Value) {
    if let Ok(cache) = ONLINE_CACHE.lock() {
        if let Some(entry) = cache.as_ref() {
            if let Some(q) = entry.get("quota") {
                d["quota"] = q.clone();
            }
            if let Some(o) = entry.get("online") {
                d["online"] = o.clone();
            }
        }
    }
}



// ---------- sessions.db aggregation ----------

#[derive(Default, Clone)]
struct Acc {
    requests: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_create: u64,
    credits: f64,
    acus: f64,
}
impl Acc {
    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_create
    }
    fn to_json(&self) -> Value {
        json!({
            "requests": self.requests, "input": self.input, "output": self.output,
            "cacheRead": self.cache_read, "cacheCreate": self.cache_create,
            "credits": self.credits, "acus": self.acus, "total": self.total(),
        })
    }
}

struct Req {
    session_id: String,
    ts: i64, // ms
    model: String,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_create: u64,
}

fn acc_add(a: &mut Acc, q: &Req) {
    a.requests += 1;
    a.input += q.input;
    a.output += q.output;
    a.cache_read += q.cache_read;
    a.cache_create += q.cache_create;
}

fn local_midnight_ms() -> i64 {
    let today = Local::now().date_naive();
    let Some(naive) = today.and_hms_opt(0, 0, 0) else {
        return 0;
    };
    Local
        .from_local_datetime(&naive)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

fn collect() -> Result<Value, String> {
    let path = db_path();
    let conn = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", path.display()))?;

    struct Sess {
        title: String,
        dir: String,
        model: String,
        created_at: i64,
        credits: f64,
        acus: f64,
    }
    let mut sessions: HashMap<String, Sess> = HashMap::new();
    {
        let mut st = conn
            .prepare("SELECT id, title, model, working_directory, created_at, metadata FROM sessions")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let (id, title, model, dir, created, meta) = row;
            let (mut credits, mut acus) = (0.0, 0.0);
            if let Some(m) = meta.and_then(|m| serde_json::from_str::<Value>(&m).ok()) {
                credits = m["total_credit_cost"].as_f64().unwrap_or(0.0);
                acus = m["total_acu_cost"].as_f64().unwrap_or(0.0);
            }
            sessions.insert(
                id.clone(),
                Sess {
                    title: title.unwrap_or_else(|| "(untitled)".into()),
                    dir: dir.unwrap_or_default(),
                    model: model.unwrap_or_default(),
                    created_at: created * 1000,
                    credits,
                    acus,
                },
            );
        }
    }

    let mut requests: HashMap<String, Req> = HashMap::new();
    {
        let mut st = conn
            .prepare(
                "SELECT session_id, chat_message FROM message_nodes \
                 WHERE json_extract(chat_message,'$.metadata.metrics.input_tokens') IS NOT NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            let (sid, cm) = row;
            let Ok(msg) = serde_json::from_str::<Value>(&cm) else { continue };
            let md = &msg["metadata"];
            let mx = &md["metrics"];
            if !mx.is_object() {
                continue;
            }
            let rid = md["request_id"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| format!("{}:{}", sid, msg["message_id"].as_str().unwrap_or("")));
            if requests.contains_key(&rid) {
                continue;
            }
            let ts = md["created_at"]
                .as_str()
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis())
                .unwrap_or_else(|| sessions.get(&sid).map(|s| s.created_at).unwrap_or(0));
            let g = |k: &str| mx[k].as_u64().unwrap_or(0);
            requests.insert(
                rid,
                Req {
                    session_id: sid.clone(),
                    ts,
                    model: md["generation_model"]
                        .as_str()
                        .map(String::from)
                        .or_else(|| sessions.get(&sid).map(|s| s.model.clone()))
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "unknown".into()),
                    input: g("input_tokens"),
                    output: g("output_tokens"),
                    cache_read: g("cache_read_tokens"),
                    cache_create: g("cache_creation_tokens"),
                },
            );
        }
    }

    let day_start = local_midnight_ms();
    let days_from_monday = Local::now().weekday().num_days_from_monday() as i64;
    let week_start = day_start - days_from_monday * 86_400_000;

    let (mut all, mut today, mut week) = (Acc::default(), Acc::default(), Acc::default());
    let mut by_model: HashMap<String, Acc> = HashMap::new();
    let mut today_by_model: HashMap<String, Acc> = HashMap::new();
    let mut by_day: HashMap<String, Acc> = HashMap::new();
    let mut by_session: HashMap<String, Acc> = HashMap::new();

    for q in requests.values() {
        acc_add(&mut all, q);
        let day = DateTime::from_timestamp_millis(q.ts)
            .map(|utc| utc.with_timezone(&Local).format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".into());
        acc_add(by_model.entry(q.model.clone()).or_default(), q);
        acc_add(by_day.entry(day).or_default(), q);
        acc_add(by_session.entry(q.session_id.clone()).or_default(), q);
        if q.ts >= day_start {
            acc_add(&mut today, q);
            acc_add(today_by_model.entry(q.model.clone()).or_default(), q);
        }
        if q.ts >= week_start {
            acc_add(&mut week, q);
        }
    }

    let sort_desc = |m: HashMap<String, Acc>| -> Vec<(String, Acc)> {
        let mut v: Vec<_> = m.into_iter().collect();
        v.sort_by_key(|(_, a)| std::cmp::Reverse(a.total()));
        v
    };
    let to_list = |m: HashMap<String, Acc>, key: &str| -> Vec<Value> {
        sort_desc(m)
            .into_iter()
            .map(|(k, a)| {
                let mut j = a.to_json();
                j[key] = json!(k);
                j
            })
            .collect()
    };

    let mut session_list: Vec<Value> = sort_desc(by_session)
        .into_iter()
        .map(|(sid, a)| {
            let mut j = a.to_json();
            j["session"] = json!(sid);
            if let Some(s) = sessions.get(&sid) {
                j["title"] = json!(s.title);
                j["dir"] = json!(s.dir);
                j["createdAt"] = json!(s.created_at);
                j["credits"] = json!(s.credits);
                j["acus"] = json!(s.acus);
            }
            j
        })
        .collect();
    session_list.truncate(100);

    let total_credits: f64 = sessions.values().map(|s| s.credits).sum();
    let total_acus: f64 = sessions.values().map(|s| s.acus).sum();
    all.credits = total_credits;
    all.acus = total_acus;

    let mut by_day_list: Vec<Value> = to_list(by_day, "day");
    by_day_list.sort_by(|a, b| a["day"].as_str().cmp(&b["day"].as_str()));

    Ok(json!({
        "dbPath": path.to_string_lossy(),
        "generatedAt": Local::now().timestamp_millis(),
        "all": all.to_json(),
        "today": today.to_json(),
        "week": week.to_json(),
        "byModel": to_list(by_model, "model"),
        "todayByModel": to_list(today_by_model, "model"),
        "byDay": by_day_list,
        "bySession": session_list,
        "quota": read_quota(),
        "sessionCount": sessions.len(),
    }))
}

#[tauri::command]
fn stats() -> Result<Value, String> {
    let mut d = collect()?;
    merge_online(&mut d);
    let src = resolve_token_cached().map(|(_, s)| s);
    d["auth"] = json!({"accounts": account_list_json(), "source": src});
    Ok(d)
}

fn main() {
    // headless verification: dump one stats JSON without opening a window
    if std::env::args().any(|a| a == "--dump-stats") {
        if let Some((token, _)) = resolve_token_cached() {
            let quota = fetch_quota_online(&token);
            let usage = fetch_usage_online(&token);
            if quota.is_some() || usage.is_some() {
                let mut cache = ONLINE_CACHE.lock().unwrap();
                let entry = cache.get_or_insert_with(|| json!({}));
                if let Some(q) = quota {
                    entry["quota"] = q;
                }
                if let Some(u) = usage {
                    entry["online"] = u;
                }
            }
        }
        match stats() {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    spawn_online_refresher();
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            stats, auth_accounts, auth_start, auth_complete, auth_poll, auth_cancel,
            auth_use, auth_remove, auth_sync_devin, accounts_status, open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running devin-usage");
}
