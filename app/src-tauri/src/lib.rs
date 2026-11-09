mod engine;
mod spotify;
mod store;

use engine::{Account, Engine, SignInResult};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use store::{now_ms, Chat, Message, Reaction, Store};
use tauri::{AppHandle, Emitter, Manager, State};

struct App {
    store: Mutex<Store>,
    engine: Arc<Engine>,
}

type R<T> = Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

fn normalize_handle(input: &str) -> String {
    let t = input.trim();
    if t.contains(':') {
        return t.to_string();
    }
    if t.contains('@') {
        return format!("mailto:{t}");
    }
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return format!("mailto:{t}");
    }
    if t.starts_with('+') {
        return format!("tel:+{digits}");
    }
    if digits.len() == 10 {
        return format!("tel:+1{digits}");
    }
    if digits.len() == 11 && digits.starts_with('1') {
        return format!("tel:+{digits}");
    }
    format!("tel:+{digits}")
}

fn handle_display(handle: &str) -> String {
    handle.trim_start_matches("mailto:").trim_start_matches("tel:").to_string()
}

#[derive(serde::Serialize)]
struct Boot {
    account: Account,
    chats: Vec<Chat>,
    spotify: bool,
}

#[tauri::command]
async fn boot(app: State<'_, App>) -> R<Boot> {
    let account = app.engine.account_state().await;
    let chats = app.store.lock().unwrap().chats().map_err(e)?;
    Ok(Boot {
        account,
        chats,
        spotify: spotify::available(),
    })
}

#[tauri::command]
fn chats(app: State<App>) -> R<Vec<Chat>> {
    app.store.lock().unwrap().chats().map_err(e)
}

#[tauri::command]
fn messages(app: State<App>, chat_id: String) -> R<Vec<Message>> {
    app.store.lock().unwrap().messages(&chat_id).map_err(e)
}

#[tauri::command]
fn mark_read(handle: AppHandle, app: State<App>, chat_id: String) -> R<()> {
    app.store.lock().unwrap().mark_read(&chat_id).map_err(e)?;
    let _ = handle.emit("chats", ());
    Ok(())
}

#[tauri::command]
fn set_pinned(handle: AppHandle, app: State<App>, chat_id: String, pinned: bool) -> R<()> {
    app.store.lock().unwrap().set_pinned(&chat_id, pinned).map_err(e)?;
    let _ = handle.emit("chats", ());
    Ok(())
}

#[tauri::command]
fn delete_chat(handle: AppHandle, app: State<App>, chat_id: String) -> R<()> {
    app.store.lock().unwrap().delete_chat(&chat_id).map_err(e)?;
    let _ = handle.emit("chats", ());
    Ok(())
}

#[tauri::command]
async fn new_chat(handle: AppHandle, app: State<'_, App>, to: String) -> R<String> {
    let to = to.trim().to_string();
    if to.is_empty() {
        return Err("who's it to?".into());
    }
    let h = normalize_handle(&to);
    let id = format!("chat-{}", h);
    let name = handle_display(&h);
    let registered = app.engine.is_registered().await || app.engine.has_saved_login();
    {
        let store = app.store.lock().unwrap();
        // a chat made while signed out is a demo chat: flip it to real once registered, or sends stay simulated
        let stale_demo = store.chat(&id).map_err(e)?.map(|c| c.demo && registered);
        if stale_demo.unwrap_or(true) {
            store.upsert_chat(&id, &name, &[h.clone()], "iMessage", !registered).map_err(e)?;
        }
    }
    let _ = handle.emit("chats", ());
    Ok(id)
}

fn store_incoming(handle: &AppHandle, inc: engine::IncomingMessage) {
    let h = normalize_handle(&inc.from);
    let name = handle_display(&h);
    if name.is_empty() {
        return;
    }
    let chat_id = format!("chat-{}", h);
    let other = h.clone();
    let st = handle.state::<App>();
    let s = st.store.lock().unwrap();
    let _ = s.upsert_chat(&chat_id, &name, &[other.clone()], "iMessage", false);
    let m = Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat_id.clone(),
        from_me: false,
        sender: other,
        text: inc.text,
        ts: now_ms(),
        status: "received".into(),
        read_ts: None,
        reactions: vec![],
        edited: false,
        reply_to: None,
    };
    let _ = s.insert(&m);
    let _ = s.bump_unread(&chat_id);
    drop(s);
    let _ = handle.emit("message", m);
    let _ = handle.emit("chats", ());
}

fn real_send(handle: AppHandle, engine: Arc<Engine>, msg_id: String, to: Vec<String>, text: String) {
    tauri::async_runtime::spawn(async move {
        let result = engine.send(to, text).await;
        let st = handle.state::<App>();
        let s = st.store.lock().unwrap();
        match result {
            Ok(_) => {
                let _ = s.set_status(&msg_id, "delivered", None);
            }
            Err(err) => {
                eprintln!("send failed: {err}");
                let _ = s.set_status(&msg_id, "failed", None);
            }
        }
        if let Ok(Some(m)) = s.message(&msg_id) {
            let _ = handle.emit("message", m);
        }
    });
}

fn simulate_delivery(handle: AppHandle, id: String) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(880)).await;
        {
            let st = handle.state::<App>();
            let s = st.store.lock().unwrap();
            let _ = s.set_status(&id, "delivered", None);
            if let Ok(Some(m)) = s.message(&id) {
                let _ = handle.emit("message", m);
            }
        }
        tokio::time::sleep(Duration::from_millis(2200)).await;
        let st = handle.state::<App>();
        let s = st.store.lock().unwrap();
        let _ = s.set_status(&id, "read", Some(now_ms()));
        if let Ok(Some(m)) = s.message(&id) {
            let _ = handle.emit("message", m);
        }
    });
}

// Waits on the wall clock in short steps: a monotonic sleep does not count time spent suspended.
fn schedule_send(handle: AppHandle, engine: Arc<Engine>, id: String, due_ms: i64) {
    tauri::async_runtime::spawn(async move {
        loop {
            let left = due_ms - now_ms();
            if left <= 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(left.min(30_000) as u64)).await;
        }
        let claimed = {
            let st = handle.state::<App>();
            let s = st.store.lock().unwrap();
            s.claim_scheduled(&id)
        };
        match claimed {
            Ok(Some((m, chat))) => {
                let _ = handle.emit("message", m.clone());
                if chat.demo {
                    simulate_delivery(handle, id);
                } else {
                    real_send(handle, engine, id, chat.handles, m.text);
                }
            }
            Ok(None) => {}
            Err(err) => eprintln!("scheduled send lookup failed: {err}"),
        }
    });
}

#[tauri::command]
fn send(handle: AppHandle, app: State<App>, chat_id: String, text: String, reply_to: Option<String>, send_at: Option<i64>) -> R<Message> {
    let text = text.trim_end().to_string();
    if text.trim().is_empty() {
        return Err("empty".into());
    }
    let store = app.store.lock().unwrap();
    let chat = store.chat(&chat_id).map_err(e)?.ok_or("chat not found")?;
    let scheduled = send_at.map(|t| t > now_ms() + 1000).unwrap_or(false);
    let m = Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat_id.clone(),
        from_me: true,
        sender: "me".into(),
        text,
        ts: if scheduled { send_at.unwrap() } else { now_ms() },
        status: if scheduled { "scheduled".into() } else { "sending".into() },
        read_ts: None,
        reactions: vec![],
        edited: false,
        reply_to,
    };
    store.insert(&m).map_err(e)?;
    drop(store);
    let _ = handle.emit("message", m.clone());
    let _ = handle.emit("chats", ());
    if scheduled {
        schedule_send(handle, app.engine.clone(), m.id.clone(), send_at.unwrap());
    } else if chat.demo {
        simulate_delivery(handle, m.id.clone());
    } else {
        real_send(handle, app.engine.clone(), m.id.clone(), chat.handles.clone(), m.text.clone());
    }
    Ok(m)
}

#[tauri::command]
fn react(handle: AppHandle, app: State<App>, message_id: String, kind: String) -> R<Message> {
    let store = app.store.lock().unwrap();
    let mut m = store.message(&message_id).map_err(e)?.ok_or("message not found")?;
    let had = m.reactions.iter().any(|r| r.from == "me" && r.kind == kind);
    m.reactions.retain(|r| r.from != "me");
    if !had {
        m.reactions.push(Reaction { from: "me".into(), kind });
    }
    store.set_reactions(&message_id, &m.reactions).map_err(e)?;
    let _ = handle.emit("message", m.clone());
    Ok(m)
}

#[tauri::command]
fn edit_message(handle: AppHandle, app: State<App>, message_id: String, text: String) -> R<Message> {
    let store = app.store.lock().unwrap();
    store.edit(&message_id, &text).map_err(e)?;
    let m = store.message(&message_id).map_err(e)?.ok_or("message not found")?;
    let _ = handle.emit("message", m.clone());
    Ok(m)
}

#[tauri::command]
fn unsend(handle: AppHandle, app: State<App>, message_id: String) -> R<()> {
    let store = app.store.lock().unwrap();
    let m = store.message(&message_id).map_err(e)?.ok_or("message not found")?;
    store.delete_message(&message_id).map_err(e)?;
    let _ = handle.emit("removed", (m.chat_id, message_id));
    let _ = handle.emit("chats", ());
    Ok(())
}

fn promote_chats(handle: &AppHandle) {
    let result = handle.state::<App>().store.lock().unwrap().promote_demo_chats();
    match result {
        Ok(_) => {
            let _ = handle.emit("chats", ());
        }
        Err(err) => eprintln!("couldn't mark chats as real after sign-in: {err}"),
    }
}

#[tauri::command]
async fn sign_in(handle: AppHandle, app: State<'_, App>, apple_id: String, password: String) -> R<SignInResult> {
    let result = app.engine.sign_in(&apple_id, &password).await?;
    if matches!(result, SignInResult::LoggedIn) {
        promote_chats(&handle);
    }
    Ok(result)
}

#[tauri::command]
async fn use_device(app: State<'_, App>) -> R<()> {
    app.engine.use_device().await
}

#[tauri::command]
async fn use_sms(app: State<'_, App>, phone_id: u32) -> R<()> {
    app.engine.use_sms(phone_id).await
}

#[tauri::command]
async fn submit_code(handle: AppHandle, app: State<'_, App>, code: String) -> R<Account> {
    app.engine.submit_code(code).await?;
    promote_chats(&handle);
    let acc = app.engine.account_state().await;
    let _ = handle.emit("chats", ());
    Ok(acc)
}

#[tauri::command]
async fn spotify_now() -> R<spotify::NowPlaying> {
    tauri::async_runtime::spawn_blocking(spotify::now_playing).await.map_err(e)?
}

#[tauri::command]
async fn spotify_search(q: String) -> R<Vec<spotify::Track>> {
    tauri::async_runtime::spawn_blocking(move || spotify::search(&q)).await.map_err(e)?
}

#[tauri::command]
async fn spotify_lookup(text: String) -> R<Option<spotify::Track>> {
    let uri = match spotify::url_to_uri(&text) {
        Some(u) => u,
        None => return Ok(None),
    };
    tauri::async_runtime::spawn_blocking(move || spotify::lookup(&uri).map(Some)).await.map_err(e)?
}

#[tauri::command]
async fn spotify_play(uri: Option<String>) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || spotify::play(uri.as_deref())).await.map_err(e)?
}

#[tauri::command]
async fn spotify_pause() -> R<()> {
    tauri::async_runtime::spawn_blocking(spotify::pause).await.map_err(e)?
}

#[tauri::command]
async fn spotify_queue(uri: String) -> R<()> {
    tauri::async_runtime::spawn_blocking(move || spotify::queue_add(&uri)).await.map_err(e)?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = pretty_env_logger::try_init();
    let store = Store::open().expect("couldn't open the message database");
    let data_dir = store::data_dir();
    let anisette_dir = data_dir.join("anisette");
    let _ = std::fs::create_dir_all(&anisette_dir);
    let engine = Arc::new(Engine::new(data_dir, anisette_dir));
    let engine_start = engine.clone();
    let engine_incoming = engine.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(App { store: Mutex::new(store), engine })
        .setup(move |app| {
            let win = app.get_webview_window("main").unwrap();
            #[cfg(target_os = "windows")]
            {
                if window_vibrancy::apply_mica(&win, None).is_err() {
                    let _ = window_vibrancy::apply_acrylic(&win, Some((240, 240, 240, 160)));
                }
            }
            let _ = win.show();
            let start_h = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) = engine_start.start().await {
                    eprintln!("engine start failed: {err}");
                }
                let acc = engine_start.account_state().await;
                if acc.state == "signed_in" {
                    promote_chats(&start_h);
                }
                // after start: a send claimed before the engine has a client would fail
                let pending = start_h.state::<App>().store.lock().unwrap().pending_scheduled();
                match pending {
                    Ok(list) => {
                        for (id, ts) in list {
                            schedule_send(start_h.clone(), engine_start.clone(), id, ts);
                        }
                    }
                    Err(err) => eprintln!("couldn't reload scheduled messages: {err}"),
                }
                let _ = start_h.emit("account", acc);
                let _ = start_h.emit("chats", ());
            });
            let app_h = app.handle().clone();
            let eng_in = engine_incoming.clone();
            tauri::async_runtime::spawn(async move {
                let mut rx = eng_in.subscribe_incoming();
                while let Ok(inc) = rx.recv().await {
                    store_incoming(&app_h, inc);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            boot,
            chats,
            messages,
            mark_read,
            set_pinned,
            delete_chat,
            new_chat,
            send,
            react,
            edit_message,
            unsend,
            sign_in,
            use_device,
            use_sms,
            submit_code,
            spotify_now,
            spotify_search,
            spotify_lookup,
            spotify_play,
            spotify_pause,
            spotify_queue
        ])
        .run(tauri::generate_context!())
        .expect("error while running winmessages");
}
