use base64::Engine as _;
use rustpush::facetime::{FACETIME_SERVICE, VIDEO_SERVICE};
use rustpush::findmy::MULTIPLEX_SERVICE;
use rustpush::macos::{HardwareConfig, MacOSConfig};
use rustpush::{
    authenticate_apple, default_provider, login_apple_delegates, register, APSConnection,
    APSConnectionResource, APSState, AppleAccount, ArcAnisetteClient, ConversationData,
    DefaultAnisetteProvider, IDSNGMIdentity, IDSUser, IMClient, LoginDelegate, LoginState, Message,
    MessageInst, MessageType, NormalMessage, OSConfig, MADRID_SERVICE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

#[derive(Serialize, Deserialize)]
struct SavedState {
    push: APSState,
    identity: IDSNGMIdentity,
    users: Vec<IDSUser>,
    apple_id: String,
}

#[derive(Serialize, Clone)]
pub struct IncomingMessage {
    pub from: String,
    pub participants: Vec<String>,
    pub text: String,
    pub ts: u64,
}

type Acc = AppleAccount<DefaultAnisetteProvider>;

fn b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

// A crash mid-write must not leave a half plist: the app would read it as signed out and lose the login.
fn save_plist<T: Serialize>(path: &std::path::Path, value: &T) -> Result<(), String> {
    let mut bytes = Vec::new();
    plist::to_writer_xml(&mut bytes, value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("plist.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })
}

fn device_id(data_dir: &std::path::Path) -> String {
    // IDS ties the registration to this id: a fresh one per launch would orphan the saved login
    let path = data_dir.join("device_id");
    match std::fs::read_to_string(&path) {
        Ok(id) if !id.trim().is_empty() => return id.trim().to_string(),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        // an unreadable file must not silently mint a new id and orphan the saved login
        Err(err) => panic!("couldn't read the device id: {err}"),
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, &id).expect("couldn't save the device id");
    id
}

fn baked(device_id: String) -> MacOSConfig {
    let mac: [u8; 6] = b64("8BiYGS+5").try_into().unwrap();
    MacOSConfig {
        inner: HardwareConfig {
            product_name: "Macmini7,1".to_string(),
            io_mac_address: mac,
            platform_serial_number: "C02XW5ZNG1J0".to_string(),
            platform_uuid: "0A2E9382-D51C-4400-AEFB-1B19B6D5EE14".to_string(),
            root_disk_uuid: "F8C0D135-3F3C-40E5-B199-D5FF21D8F4B3".to_string(),
            board_id: "Mac-35C5E08120C7EEAF".to_string(),
            os_build_num: "22G120".to_string(),
            platform_serial_number_enc: b64("xRxStSaRbkOaGFgcYvUKq94="),
            platform_uuid_enc: b64("HIKkWRTCX+4XZlTVL/P/Nf4="),
            root_disk_uuid_enc: b64("ljmdTna7NJMWlrfSCEJxSIQ="),
            rom: b64("8BiYGS+5"),
            rom_enc: b64("sNBW72jXL8UDlCeTwURdA2E="),
            mlb: "C028521014NG0MCUE".to_string(),
            mlb_enc: b64("ZutchoS+gUSE9psDi70FQqo="),
        },
        version: "13.6.4".to_string(),
        protocol_version: 1660,
        device_id,
        icloud_ua: "com.apple.iCloudHelper/282 CFNetwork/1408.0.4 Darwin/22.5.0".to_string(),
        aoskit_version: "com.apple.AOSKit/282 (com.apple.accountsd/113)".to_string(),
        udid: Some("55A1CFBF5BB56AD1159BD2CB7D6FF546E48EAAE4BF16188A07B1FB9C83138CA2".to_string()),
    }
}

#[allow(dead_code)]
fn activator() -> MacOSConfig {
    let mac: [u8; 6] = b64("qGC2H1Am").try_into().unwrap();
    MacOSConfig {
        inner: HardwareConfig {
            product_name: "Macmini7,1".to_string(),
            io_mac_address: mac,
            platform_serial_number: "C07SW06DG1J1".to_string(),
            platform_uuid: "9CA28140-E736-59AB-9AC6-792E507D2184".to_string(),
            root_disk_uuid: "2CB26D1C-D73C-4F92-970B-F87341D4144E".to_string(),
            board_id: "Mac-35C5E08120C7EEAF".to_string(),
            os_build_num: "22G120".to_string(),
            platform_serial_number_enc: b64("zxsZESjSTtPw9SyBsELYoEM="),
            platform_uuid_enc: b64("N7rgxPQOa8kAoaUFzrgOoJI="),
            root_disk_uuid_enc: b64("RMD8Rrsnoq/nRCd5qG5pxvc="),
            rom: b64("qGC26soK"),
            rom_enc: b64("oygSjpinM48Z7vkz7Ttqaek="),
            mlb: "C076497007FGV6XDW".to_string(),
            mlb_enc: b64("mqJmiIGO40uj6irTvezB4K4="),
        },
        version: "13.6.4".to_string(),
        protocol_version: 1660,
        device_id: uuid::Uuid::new_v4().to_string(),
        icloud_ua: "com.apple.iCloudHelper/282 CFNetwork/1408.0.4 Darwin/22.5.0".to_string(),
        aoskit_version: "com.apple.AOSKit/282 (com.apple.accountsd/113)".to_string(),
        udid: Some("55A1CFBF5BB56AD1159BD2CB7D6FF546E48EAAE4BF16188A07B1FB9C83138CA2".to_string()),
    }
}

#[derive(Serialize, Clone)]
pub struct TrustedPhone {
    pub id: u32,
    pub display: String,
}

#[derive(Serialize, Clone)]
#[serde(tag = "kind")]
pub enum SignInResult {
    NeedsTwoFactor { phones: Vec<TrustedPhone> },
    LoggedIn,
}

#[derive(Serialize, Clone)]
pub struct Account {
    pub state: String,
    pub apple_id: String,
    pub handles: Vec<String>,
    pub detail: String,
}

impl Account {
    pub fn signed_out() -> Account {
        Account {
            state: "signed_out".into(),
            apple_id: String::new(),
            handles: vec![],
            detail: String::new(),
        }
    }
}

struct Session {
    account: Acc,
    sms_body: Option<rustpush::VerifyBody>,
    email: String,
}

pub struct Engine {
    data_dir: PathBuf,
    anisette_dir: PathBuf,
    config: Arc<MacOSConfig>,
    connection: Mutex<Option<APSConnection>>,
    anisette: Mutex<Option<ArcAnisetteClient<DefaultAnisetteProvider>>>,
    session: Mutex<Option<Session>>,
    client: Mutex<Option<Arc<IMClient>>>,
    apple_id: Mutex<String>,
    incoming: broadcast::Sender<IncomingMessage>,
}

pub fn engine_ready() -> bool {
    false
}

pub fn not_ready() -> String {
    "engine still starting".into()
}

impl Engine {
    pub fn new(data_dir: PathBuf, anisette_dir: PathBuf) -> Engine {
        Engine {
            config: Arc::new(baked(device_id(&data_dir))),
            data_dir,
            anisette_dir,
            connection: Mutex::new(None),
            anisette: Mutex::new(None),
            session: Mutex::new(None),
            client: Mutex::new(None),
            apple_id: Mutex::new(String::new()),
            incoming: broadcast::channel(256).0,
        }
    }

    pub fn subscribe_incoming(&self) -> broadcast::Receiver<IncomingMessage> {
        self.incoming.subscribe()
    }

    pub async fn start(&self) -> Result<(), String> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let ks_path = self.data_dir.join("keystore.plist");
        keystore::init_keystore(keystore::software::SoftwareKeystore {
            state: plist::from_file(&ks_path).unwrap_or_default(),
            update_state: {
                let ks_path = ks_path.clone();
                Box::new(move |state| {
                    if let Err(err) = save_plist(&ks_path, state) {
                        eprintln!("keystore save failed: {err}");
                    }
                })
            },
            encryptor: keystore::software::NoEncryptor,
        });
        let saved: Option<SavedState> = plist::from_file(self.reg_path()).ok();
        let (connection, _err) =
            APSConnectionResource::new(self.config.clone(), saved.as_ref().map(|s| s.push.clone()))
                .await;
        let gsa = self.config.get_gsa_config(&*connection.state.read().await, false);
        let anisette = default_provider(gsa, self.anisette_dir.clone());
        *self.connection.lock().await = Some(connection.clone());
        *self.anisette.lock().await = Some(anisette);
        if let Some(saved) = saved {
            *self.apple_id.lock().await = saved.apple_id.clone();
            self.build_client(connection, saved.users, saved.identity).await;
        }
        Ok(())
    }

    fn reg_path(&self) -> PathBuf {
        self.data_dir.join("registration.plist")
    }

    // The engine is not "registered" until start() has built the client; a saved login already means real chats.
    pub fn has_saved_login(&self) -> bool {
        self.reg_path().exists()
    }

    async fn build_client(&self, connection: APSConnection, users: Vec<IDSUser>, identity: IDSNGMIdentity) {
        let services = &[&MADRID_SERVICE, &MULTIPLEX_SERVICE, &FACETIME_SERVICE, &VIDEO_SERVICE];
        let cache_path = self.data_dir.join("id_cache.plist");
        let recv_sub = connection.messages_cont.subscribe();
        let client = Arc::new(
            IMClient::new(
                connection,
                users,
                identity,
                services,
                cache_path,
                self.config.clone(),
                Box::new(|_| {}),
            )
            .await,
        );
        *self.client.lock().await = Some(client.clone());
        let incoming = self.incoming.clone();
        let recv_client = client.clone();
        tokio::spawn(async move {
            let mut sub = recv_sub;
            loop {
                match sub.recv().await {
                    Ok(aps) => {
                        if let Ok(Some(inst)) = recv_client.handle(aps).await {
                            if let Message::Message(normal) = &inst.message {
                                let text = normal.parts.raw_text();
                                if text.is_empty() {
                                    continue;
                                }
                                let from = inst.sender.clone().unwrap_or_default();
                                let participants = inst
                                    .conversation
                                    .as_ref()
                                    .map(|c| c.participants.clone())
                                    .unwrap_or_default();
                                let _ = incoming.send(IncomingMessage {
                                    from,
                                    participants,
                                    text,
                                    ts: inst.sent_timestamp,
                                });
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
    }

    pub async fn is_registered(&self) -> bool {
        self.client.lock().await.is_some()
    }

    pub async fn account_state(&self) -> Account {
        if self.client.lock().await.is_some() {
            let id = self.apple_id.lock().await.clone();
            Account {
                state: "signed_in".into(),
                apple_id: id,
                handles: vec![],
                detail: "registered".into(),
            }
        } else {
            Account::signed_out()
        }
    }

    pub async fn sign_in(&self, apple_id: &str, password: &str) -> Result<SignInResult, String> {
        let connection = self.connection.lock().await.clone().ok_or("not started")?;
        let anisette = self.anisette.lock().await.clone().ok_or("not started")?;
        let gsa = self.config.get_gsa_config(&*connection.state.read().await, false);
        let mut account = AppleAccount::new_with_anisette(gsa, anisette, None, Box::new(|_| {}))
            .map_err(|e| format!("account init: {e:?}"))?;
        let hashed = Sha256::digest(password.as_bytes()).to_vec();
        let state = account
            .login_email_pass(apple_id, &hashed)
            .await
            .map_err(|e| format!("{e:?}"))?;
        *self.apple_id.lock().await = apple_id.to_string();
        if matches!(state, LoginState::LoggedIn) {
            self.finish_register(account, apple_id.to_string()).await?;
            return Ok(SignInResult::LoggedIn);
        }
        let phones = match account.get_auth_extras().await {
            Ok(extras) => extras
                .trusted_phone_numbers
                .iter()
                .map(|p| TrustedPhone {
                    id: p.id,
                    display: format!("{} (...{})", p.number_with_dial_code, p.last_two_digits),
                })
                .collect(),
            Err(err) => {
                eprintln!("couldn't list trusted phones: {err:?}");
                vec![]
            }
        };
        *self.session.lock().await = Some(Session {
            account,
            sms_body: None,
            email: apple_id.to_string(),
        });
        Ok(SignInResult::NeedsTwoFactor { phones })
    }

    pub async fn use_device(&self) -> Result<(), String> {
        let mut guard = self.session.lock().await;
        let sess = guard.as_mut().ok_or("no sign-in in progress")?;
        sess.account.send_2fa_to_devices().await.map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    pub async fn use_sms(&self, phone_id: u32) -> Result<(), String> {
        let mut guard = self.session.lock().await;
        let sess = guard.as_mut().ok_or("no sign-in in progress")?;
        let sent = sess
            .account
            .send_sms_2fa_to_devices(phone_id)
            .await
            .map_err(|e| format!("{e:?}"))?;
        if let LoginState::NeedsSMS2FAVerification(body) = sent {
            sess.sms_body = Some(body);
            Ok(())
        } else {
            Err(format!("unexpected after sms request: {sent:?}"))
        }
    }

    pub async fn submit_code(&self, code: String) -> Result<(), String> {
        let (mut account, sms_body, email) = {
            let mut guard = self.session.lock().await;
            let sess = guard.take().ok_or("no sign-in in progress")?;
            (sess.account, sess.sms_body, sess.email)
        };
        let state = if let Some(body) = sms_body {
            account.verify_sms_2fa(code, body).await.map_err(|e| format!("{e:?}"))?
        } else {
            account.verify_2fa(code).await.map_err(|e| format!("{e:?}"))?
        };
        if !matches!(state, LoginState::LoggedIn) {
            return Err(format!("2fa not accepted: {state:?}"));
        }
        self.finish_register(account, email).await
    }

    async fn finish_register(&self, mut account: Acc, email: String) -> Result<(), String> {
        let connection = self.connection.lock().await.clone().ok_or("not started")?;
        account
            .update_postdata("Apple Device", None, &["icloud", "imessage", "facetime"])
            .await
            .map_err(|e| format!("postdata: {e:?}"))?;
        let services = &[&MADRID_SERVICE, &MULTIPLEX_SERVICE, &FACETIME_SERVICE, &VIDEO_SERVICE];
        let identity = IDSNGMIdentity::new().map_err(|e| format!("identity: {e:?}"))?;

        let delegates = login_apple_delegates(
            &account,
            None,
            self.config.as_ref(),
            &[LoginDelegate::IDS, LoginDelegate::MobileMe],
        )
        .await
        .map_err(|e| format!("delegates: {e:?}"))?;
        let user = authenticate_apple(delegates.ids.ok_or("no ids delegate")?, self.config.as_ref())
            .await
            .map_err(|e| format!("authenticate: {e:?}"))?;
        let mut users = vec![user];
        register(
            self.config.as_ref(),
            &*connection.state.read().await,
            services,
            &mut users,
            &identity,
        )
        .await
        .map_err(|e| format!("register: {e}"))?;
        let saved = SavedState {
            push: connection.state.read().await.clone(),
            identity: identity.clone(),
            users: users.clone(),
            apple_id: email.clone(),
        };
        save_plist(&self.reg_path(), &saved).map_err(|e| format!("couldn't save the login: {e}"))?;
        *self.apple_id.lock().await = email;
        self.build_client(connection, users, identity).await;
        Ok(())
    }

    pub async fn handles(&self) -> Result<Vec<String>, String> {
        let guard = self.client.lock().await;
        let client = guard.as_ref().ok_or("not registered")?;
        Ok(client.identity.get_handles().await)
    }

    pub async fn send(&self, to: Vec<String>, text: String) -> Result<(), String> {
        let guard = self.client.lock().await;
        let client = guard.as_ref().ok_or("not registered")?;
        let handle = client.identity.get_handles().await.first().cloned().ok_or("no handle")?;
        let participants: Vec<String> = to
            .into_iter()
            .map(|t| {
                if t.contains(':') {
                    t
                } else if t.starts_with('+') {
                    format!("tel:{t}")
                } else {
                    format!("mailto:{t}")
                }
            })
            .collect();
        let mut msg = MessageInst::new(
            ConversationData {
                participants,
                cv_name: None,
                sender_guid: Some(uuid::Uuid::new_v4().to_string()),
                after_guid: None,
            },
            &handle,
            Message::Message(NormalMessage::new(text, MessageType::IMessage)),
        );
        client.send(&mut msg).await.map_err(|e| format!("{e:?}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_plist_replaces_the_file_whole_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("winmessages-test-plist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registration.plist");
        std::fs::write(&path, "old content that is not a plist").unwrap();
        save_plist(&path, &plist::Dictionary::from_iter([("k".to_string(), plist::Value::String("v".into()))])).unwrap();
        let back: plist::Dictionary = plist::from_file(&path).unwrap();
        assert_eq!(back.get("k").and_then(|v| v.as_string()), Some("v"));
        let leftovers: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers.len(), 1, "only the plist remains: {leftovers:?}");
    }
}
