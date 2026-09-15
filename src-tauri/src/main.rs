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
    collect()
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![stats])
        .run(tauri::generate_context!())
        .expect("error while running devin-usage");
}
