use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static CACHE: once_cell::sync::Lazy<Mutex<HashMap<String, Track>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, Clone, Debug)]
pub struct Track {
    pub uri: String,
    pub kind: String,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub image: String,
    pub url: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct NowPlaying {
    pub track: Option<Track>,
    pub is_playing: bool,
    pub context: String,
}

fn cli() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(base).join("Spotify").join("spotify_cli.exe")
}

pub fn available() -> bool {
    cli().exists()
}

fn run(args: &[&str]) -> Result<Value, String> {
    let exe = cli();
    if !exe.exists() {
        return Err("spotify desktop app isn't installed".into());
    }
    let mut full: Vec<&str> = args.to_vec();
    full.push("--format");
    full.push("json");
    let mut cmd = Command::new(exe);
    cmd.args(&full);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    if text.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.is_empty() {
            return Ok(Value::Null);
        }
        return Err(err);
    }
    serde_json::from_str(&text).map_err(|e| format!("{e}: {text}"))
}

fn big_image(s: &str) -> String {
    let s = if let Some(id) = s.strip_prefix("spotify:image:") {
        format!("https://i.scdn.co/image/{id}")
    } else if s.starts_with("spotify:mosaic:") {
        let first = s.trim_start_matches("spotify:mosaic:").split(':').next().unwrap_or("");
        format!("https://i.scdn.co/image/{first}")
    } else {
        s.to_string()
    };
    s.replace("ab67616d00004851", "ab67616d0000b273")
        .replace("ab67616d00001e02", "ab67616d0000b273")
}

pub fn uri_to_url(uri: &str) -> String {
    let parts: Vec<&str> = uri.split(':').collect();
    if parts.len() >= 3 {
        format!("https://open.spotify.com/{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        uri.to_string()
    }
}

pub fn url_to_uri(text: &str) -> Option<String> {
    let re = Regex::new(r"(?:https?://open\.spotify\.com/(?:intl-[a-z]{2}/)?(track|album|playlist|artist|episode|show)/([A-Za-z0-9]{22}))|(?:spotify:(track|album|playlist|artist|episode|show):([A-Za-z0-9]{22}))").ok()?;
    let c = re.captures(text)?;
    if let (Some(k), Some(id)) = (c.get(1), c.get(2)) {
        return Some(format!("spotify:{}:{}", k.as_str(), id.as_str()));
    }
    if let (Some(k), Some(id)) = (c.get(3), c.get(4)) {
        return Some(format!("spotify:{}:{}", k.as_str(), id.as_str()));
    }
    None
}

fn kind_of(uri: &str) -> String {
    uri.split(':').nth(1).unwrap_or("track").to_string()
}

pub fn lookup(uri: &str) -> Result<Track, String> {
    if let Some(t) = CACHE.lock().unwrap().get(uri) {
        return Ok(t.clone());
    }
    let v = run(&["lookup", uri])?;
    let e = v
        .get("entities")
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .ok_or("nothing found for that link")?;
    let artist = e
        .get("contributors")
        .and_then(|c| c.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.get("name").and_then(|n| n.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let t = Track {
        uri: uri.to_string(),
        kind: kind_of(uri),
        name: e.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        artist,
        album: e
            .get("parent")
            .and_then(|p| p.get("name"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        image: big_image(e.get("image_url").and_then(|x| x.as_str()).unwrap_or("")),
        url: uri_to_url(uri),
    };
    CACHE.lock().unwrap().insert(uri.to_string(), t.clone());
    Ok(t)
}

pub fn now_playing() -> Result<NowPlaying, String> {
    let v = run(&["now-playing"])?;
    let cp = match v.get("currently_playing") {
        Some(c) if !c.is_null() => c,
        _ => {
            return Ok(NowPlaying { track: None, is_playing: false, context: String::new() });
        }
    };
    let uri = cp.get("uri").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let track = if uri.is_empty() { None } else { lookup(&uri).ok() };
    Ok(NowPlaying {
        track,
        is_playing: cp.get("is_playing").and_then(|x| x.as_bool()).unwrap_or(false),
        context: cp
            .get("context_description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

pub fn search(q: &str) -> Result<Vec<Track>, String> {
    let v = run(&["search", q])?;
    let mut out = Vec::new();
    for (key, kind) in [("tracks", "track"), ("albums", "album"), ("playlists", "playlist")] {
        if let Some(arr) = v.get(key).and_then(|x| x.as_array()) {
            for e in arr {
                let uri = e.get("uri").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if uri.is_empty() {
                    continue;
                }
                let artist = e
                    .get("artists")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
                    .or_else(|| e.get("author").and_then(|x| x.as_str()).map(|s| s.to_string()))
                    .unwrap_or_default();
                let t = Track {
                    url: uri_to_url(&uri),
                    kind: kind.to_string(),
                    name: e.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    artist,
                    album: String::new(),
                    image: big_image(e.get("image").and_then(|x| x.as_str()).unwrap_or("")),
                    uri,
                };
                out.push(t);
            }
        }
    }
    Ok(out)
}

pub fn play(uri: Option<&str>) -> Result<(), String> {
    match uri {
        Some(u) => run(&["play", u]).map(|_| ()),
        None => run(&["resume"]).map(|_| ()),
    }
}

pub fn pause() -> Result<(), String> {
    run(&["pause"]).map(|_| ())
}

pub fn queue_add(uri: &str) -> Result<(), String> {
    run(&["queue", "add", uri]).map(|_| ())
}
