use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Reaction {
    pub from: String,
    pub kind: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Message {
    pub id: String,
    pub chat_id: String,
    pub from_me: bool,
    pub sender: String,
    pub text: String,
    pub ts: i64,
    pub status: String,
    pub read_ts: Option<i64>,
    pub reactions: Vec<Reaction>,
    pub edited: bool,
    pub reply_to: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Chat {
    pub id: String,
    pub name: String,
    pub handles: Vec<String>,
    pub service: String,
    pub pinned: bool,
    pub unread: i64,
    pub last_text: String,
    pub last_ts: i64,
    pub demo: bool,
}

pub struct Store {
    pub db: Connection,
}

pub fn data_dir() -> PathBuf {
    let d = dirs::data_dir().expect("no user data directory").join("winmessages");
    std::fs::create_dir_all(&d).expect("couldn't create the data directory");
    // keystore.plist and registration.plist hold plaintext Apple tokens: owner-only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).expect("couldn't restrict the data directory");
    }
    d
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Store {
    pub fn open() -> rusqlite::Result<Store> {
        Store::open_at(&data_dir().join("messages.db"))
    }

    fn open_at(path: &std::path::Path) -> rusqlite::Result<Store> {
        let db = Connection::open(path)?;
        db.execute_batch(
            "pragma journal_mode=wal;
             create table if not exists chats(
               id text primary key, name text not null, handles text not null,
               service text not null default 'iMessage', pinned integer not null default 0,
               unread integer not null default 0, demo integer not null default 0);
             create table if not exists messages(
               id text primary key, chat_id text not null, from_me integer not null,
               sender text not null, text text not null, ts integer not null,
               status text not null default 'sent', read_ts integer,
               reactions text not null default '[]', edited integer not null default 0,
               reply_to text);
             create index if not exists messages_chat on messages(chat_id, ts);
             create table if not exists kv(k text primary key, v text not null);",
        )?;
        // a message still "sending" or past its schedule at startup was never sent: say so, except in demo chats
        db.execute("update messages set status='failed' where from_me=1 and status='sending' and chat_id not in (select id from chats where demo=1)", [])?;
        db.execute("update messages set status='delivered' where from_me=1 and status='sending'", [])?;
        let now = chrono::Utc::now().timestamp_millis();
        db.execute("update messages set status='failed' where from_me=1 and status='scheduled' and ts<=?1 and chat_id not in (select id from chats where demo=1)", params![now])?;
        db.execute("update messages set status='delivered' where from_me=1 and status='scheduled' and ts<=?1", params![now])?;
        let s = Store { db };
        if s.get_kv("seeded")?.is_none() {
            s.seed()?;
            s.set_kv("seeded", "1")?;
        }
        Ok(s)
    }

    pub fn get_kv(&self, k: &str) -> rusqlite::Result<Option<String>> {
        self.db
            .query_row("select v from kv where k=?1", params![k], |r| r.get(0))
            .optional()
    }

    pub fn set_kv(&self, k: &str, v: &str) -> rusqlite::Result<()> {
        self.db.execute(
            "insert into kv(k,v) values(?1,?2) on conflict(k) do update set v=excluded.v",
            params![k, v],
        )?;
        Ok(())
    }

    fn row_msg(r: &rusqlite::Row) -> rusqlite::Result<Message> {
        let reactions: String = r.get(8)?;
        Ok(Message {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            from_me: r.get::<_, i64>(2)? == 1,
            sender: r.get(3)?,
            text: r.get(4)?,
            ts: r.get(5)?,
            status: r.get(6)?,
            read_ts: r.get(7)?,
            reactions: serde_json::from_str(&reactions).unwrap_or_default(),
            edited: r.get::<_, i64>(9)? == 1,
            reply_to: r.get(10)?,
        })
    }

    pub fn chats(&self) -> rusqlite::Result<Vec<Chat>> {
        let mut st = self.db.prepare(
            "select c.id, c.name, c.handles, c.service, c.pinned, c.unread, c.demo,
                    coalesce((select text from messages m where m.chat_id=c.id order by ts desc limit 1), ''),
                    coalesce((select ts from messages m where m.chat_id=c.id order by ts desc limit 1), 0)
             from chats c order by 9 desc",
        )?;
        let rows = st.query_map([], |r| {
            let handles: String = r.get(2)?;
            Ok(Chat {
                id: r.get(0)?,
                name: r.get(1)?,
                handles: serde_json::from_str(&handles).unwrap_or_default(),
                service: r.get(3)?,
                pinned: r.get::<_, i64>(4)? == 1,
                unread: r.get(5)?,
                demo: r.get::<_, i64>(6)? == 1,
                last_text: r.get(7)?,
                last_ts: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn chat(&self, id: &str) -> rusqlite::Result<Option<Chat>> {
        Ok(self.chats()?.into_iter().find(|c| c.id == id))
    }

    pub fn messages(&self, chat_id: &str) -> rusqlite::Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "select id, chat_id, from_me, sender, text, ts, status, read_ts, reactions, edited, reply_to
             from messages where chat_id=?1 order by ts asc",
        )?;
        let rows = st.query_map(params![chat_id], Store::row_msg)?;
        rows.collect()
    }

    pub fn message(&self, id: &str) -> rusqlite::Result<Option<Message>> {
        self.db
            .query_row(
                "select id, chat_id, from_me, sender, text, ts, status, read_ts, reactions, edited, reply_to
                 from messages where id=?1",
                params![id],
                Store::row_msg,
            )
            .optional()
    }

    pub fn insert(&self, m: &Message) -> rusqlite::Result<()> {
        self.db.execute(
            "insert or replace into messages(id, chat_id, from_me, sender, text, ts, status, read_ts, reactions, edited, reply_to)
             values(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                m.id,
                m.chat_id,
                m.from_me as i64,
                m.sender,
                m.text,
                m.ts,
                m.status,
                m.read_ts,
                serde_json::to_string(&m.reactions).unwrap(),
                m.edited as i64,
                m.reply_to
            ],
        )?;
        Ok(())
    }

    pub fn set_status(&self, id: &str, status: &str, read_ts: Option<i64>) -> rusqlite::Result<()> {
        self.db.execute(
            "update messages set status=?2, read_ts=coalesce(?3, read_ts) where id=?1",
            params![id, status, read_ts],
        )?;
        Ok(())
    }

    // Chats made while signed out are demo chats; once registered they are real. Seeded demo chats keep their demo- ids.
    // What was typed in the sandbox was never sent, so it must not look sent or go out later.
    pub fn promote_demo_chats(&self) -> rusqlite::Result<usize> {
        self.db.execute(
            "update messages set status='failed', read_ts=null where from_me=1 and chat_id in (select id from chats where demo=1 and id not like 'demo-%')",
            [],
        )?;
        self.db.execute("update chats set demo=0 where demo=1 and id not like 'demo-%'", [])
    }

    pub fn pending_scheduled(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        let mut st = self.db.prepare("select id, ts from messages where from_me=1 and status='scheduled'")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    // The timer fires long after scheduling: send what the row says now, and only once.
    pub fn claim_scheduled(&self, id: &str) -> rusqlite::Result<Option<(Message, Chat)>> {
        let claimed = self.db.execute(
            "update messages set status='sending' where id=?1 and status='scheduled'",
            params![id],
        )?;
        if claimed == 0 {
            return Ok(None);
        }
        let Some(m) = self.message(id)? else { return Ok(None) };
        Ok(self.chat(&m.chat_id)?.map(|c| (m, c)))
    }

    pub fn set_reactions(&self, id: &str, r: &[Reaction]) -> rusqlite::Result<()> {
        self.db.execute(
            "update messages set reactions=?2 where id=?1",
            params![id, serde_json::to_string(r).unwrap()],
        )?;
        Ok(())
    }

    pub fn edit(&self, id: &str, text: &str) -> rusqlite::Result<()> {
        self.db
            .execute("update messages set text=?2, edited=1 where id=?1", params![id, text])?;
        Ok(())
    }

    pub fn delete_message(&self, id: &str) -> rusqlite::Result<()> {
        self.db.execute("delete from messages where id=?1", params![id])?;
        Ok(())
    }

    pub fn mark_read(&self, chat_id: &str) -> rusqlite::Result<()> {
        self.db.execute("update chats set unread=0 where id=?1", params![chat_id])?;
        Ok(())
    }

    pub fn bump_unread(&self, chat_id: &str) -> rusqlite::Result<()> {
        self.db.execute("update chats set unread=unread+1 where id=?1", params![chat_id])?;
        Ok(())
    }

    pub fn set_pinned(&self, chat_id: &str, pinned: bool) -> rusqlite::Result<()> {
        self.db
            .execute("update chats set pinned=?2 where id=?1", params![chat_id, pinned as i64])?;
        Ok(())
    }

    pub fn delete_chat(&self, chat_id: &str) -> rusqlite::Result<()> {
        self.db.execute("delete from messages where chat_id=?1", params![chat_id])?;
        self.db.execute("delete from chats where id=?1", params![chat_id])?;
        Ok(())
    }

    pub fn upsert_chat(&self, id: &str, name: &str, handles: &[String], service: &str, demo: bool) -> rusqlite::Result<()> {
        self.db.execute(
            "insert into chats(id, name, handles, service, demo) values(?1,?2,?3,?4,?5)
             on conflict(id) do update set name=excluded.name, handles=excluded.handles, demo=excluded.demo",
            params![id, name, serde_json::to_string(handles).unwrap(), service, demo as i64],
        )?;
        Ok(())
    }

    fn seed(&self) -> rusqlite::Result<()> {
        let t = now_ms();
        let min = 60_000i64;
        let hr = 60 * min;
        let day = 24 * hr;
        let chats: Vec<(&str, &str, &str, bool, Vec<(bool, &str, i64)>)> = vec![
            (
                "demo-aiyan",
                "Aiyan",
                "iMessage",
                true,
                vec![
                    (false, "yo did u finish the thing", 3 * hr),
                    (true, "almost, imessage on windows fr", 3 * hr - 2 * min),
                    (false, "no way", 3 * hr - 3 * min),
                    (false, "send it when its done", 3 * hr - 3 * min + 4000),
                    (true, "https://open.spotify.com/track/4aTYZL8uyGXAdmWOQy4Wgx", 25 * min),
                    (true, "this is the soundtrack", 25 * min - 3000),
                    (false, "2000 watts is crazy", 20 * min),
                ],
            ),
            (
                "demo-mom",
                "Mom",
                "iMessage",
                true,
                vec![
                    (false, "Dinner at 7", day + 2 * hr),
                    (true, "ok", day + 2 * hr - min),
                    (false, "Did you do your homework?", 5 * hr),
                ],
            ),
            (
                "demo-ram",
                "RAM",
                "iMessage",
                true,
                vec![
                    (false, "video goes up friday", 2 * day),
                    (true, "lets goo", 2 * day - 5 * min),
                ],
            ),
            (
                "demo-group",
                "the pop server",
                "iMessage",
                false,
                vec![
                    (false, "who's on tonight", 6 * day),
                    (true, "me", 6 * day - 2 * min),
                ],
            ),
            (
                "demo-sms",
                "+1 (555) 013-2244",
                "SMS",
                false,
                vec![(false, "Your verification code is 482913", 3 * day)],
            ),
        ];
        for (id, name, service, pinned, msgs) in chats {
            let handle = if service == "SMS" { name.to_string() } else { format!("{}@icloud.com", name.to_lowercase().replace(' ', "")) };
            self.upsert_chat(id, name, &[handle.clone()], service, true)?;
            self.set_pinned(id, pinned)?;
            let n = msgs.len();
            for (i, (me, text, ago)) in msgs.into_iter().enumerate() {
                let ts = t - ago;
                let last = i == n - 1;
                let m = Message {
                    id: uuid::Uuid::new_v4().to_string(),
                    chat_id: id.to_string(),
                    from_me: me,
                    sender: if me { "me".into() } else { handle.clone() },
                    text: text.to_string(),
                    ts,
                    status: if me { if last { "read".into() } else { "delivered".into() } } else { "received".into() },
                    read_ts: if me && last { Some(ts + 90_000) } else { None },
                    reactions: if text.starts_with("2000") {
                        vec![Reaction { from: "me".into(), kind: "love".into() }]
                    } else {
                        vec![]
                    },
                    edited: false,
                    reply_to: None,
                };
                self.insert(&m)?;
            }
        }
        self.db.execute("update chats set unread=1 where id in ('demo-aiyan','demo-mom')", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outgoing(id: &str, chat: &str, status: &str) -> Message {
        Message {
            id: id.into(),
            chat_id: chat.into(),
            from_me: true,
            sender: "me".into(),
            text: "hi".into(),
            ts: 1,
            status: status.into(),
            read_ts: None,
            reactions: vec![],
            edited: false,
            reply_to: None,
        }
    }

    fn scratch_db(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("winmessages-test-{name}-{}.db", uuid::Uuid::new_v4()));
        p
    }

    #[test]
    fn unsent_messages_in_real_chats_fail_at_startup_demo_ones_do_not() {
        let path = scratch_db("startup");
        let s = Store::open_at(&path).unwrap();
        s.upsert_chat("chat-real", "r", &["mailto:r@x".into()], "iMessage", false).unwrap();
        s.upsert_chat("chat-demo", "d", &["mailto:d@x".into()], "iMessage", true).unwrap();
        s.insert(&outgoing("m1", "chat-real", "sending")).unwrap();
        s.insert(&outgoing("m2", "chat-real", "scheduled")).unwrap();
        s.insert(&outgoing("m3", "chat-demo", "sending")).unwrap();
        s.insert(&outgoing("m4", "chat-demo", "scheduled")).unwrap();
        s.insert(&outgoing("m6", "chat-gone", "sending")).unwrap();
        let mut future = outgoing("m5", "chat-real", "scheduled");
        future.ts = 4_102_444_800_000;
        s.insert(&future).unwrap();
        drop(s);
        let s = Store::open_at(&path).unwrap();
        assert_eq!(s.message("m5").unwrap().unwrap().status, "scheduled", "a future schedule must survive startup");
        assert_eq!(s.message("m1").unwrap().unwrap().status, "failed");
        assert_eq!(s.message("m2").unwrap().unwrap().status, "failed");
        assert_eq!(s.message("m3").unwrap().unwrap().status, "delivered");
        assert_eq!(s.message("m4").unwrap().unwrap().status, "delivered");
        assert_eq!(s.message("m6").unwrap().unwrap().status, "failed", "a message with no chat was never sent either");
    }

    #[test]
    fn upsert_flips_a_demo_chat_to_real() {
        let s = Store::open_at(&scratch_db("flip")).unwrap();
        s.upsert_chat("chat-x", "x", &["mailto:x@x".into()], "iMessage", true).unwrap();
        s.upsert_chat("chat-x", "x", &["mailto:x@x".into()], "iMessage", false).unwrap();
        assert!(!s.chat("chat-x").unwrap().unwrap().demo);
    }

    #[test]
    fn promote_demo_chats_makes_user_chats_real_and_keeps_the_seeded_ones() {
        let s = Store::open_at(&scratch_db("promote")).unwrap();
        s.upsert_chat("chat-a", "a", &["mailto:a@x".into()], "iMessage", true).unwrap();
        let mut simulated = outgoing("p1", "chat-a", "read");
        simulated.read_ts = Some(5);
        s.insert(&simulated).unwrap();
        s.insert(&outgoing("p2", "chat-a", "scheduled")).unwrap();
        assert_eq!(s.promote_demo_chats().unwrap(), 1);
        assert!(!s.chat("chat-a").unwrap().unwrap().demo);
        assert_eq!(s.message("p1").unwrap().unwrap().status, "failed", "simulated history must not look sent");
        assert_eq!(s.message("p1").unwrap().unwrap().read_ts, None);
        assert_eq!(s.message("p2").unwrap().unwrap().status, "failed", "a sandbox schedule must not send for real");
        assert!(s.chat("demo-aiyan").unwrap().unwrap().demo, "seeded demo chats stay demo");
        assert!(s.messages("demo-ram").unwrap().iter().any(|m| m.status == "read"), "seeded history is untouched");
    }

    #[test]
    fn claim_scheduled_sends_current_text_once_and_skips_deleted() {
        let s = Store::open_at(&scratch_db("claim")).unwrap();
        s.upsert_chat("chat-r", "r", &["mailto:r@x".into()], "iMessage", false).unwrap();
        s.insert(&outgoing("a", "chat-r", "scheduled")).unwrap();
        s.insert(&outgoing("b", "chat-r", "scheduled")).unwrap();
        s.edit("a", "edited text").unwrap();
        s.delete_message("b").unwrap();
        assert_eq!(s.pending_scheduled().unwrap(), vec![("a".to_string(), 1)]);
        let (m, c) = s.claim_scheduled("a").unwrap().expect("a is still scheduled");
        assert_eq!(m.text, "edited text");
        assert_eq!(c.handles, vec!["mailto:r@x".to_string()]);
        assert_eq!(s.message("a").unwrap().unwrap().status, "sending");
        assert!(s.claim_scheduled("a").unwrap().is_none(), "second claim must not send again");
        assert!(s.claim_scheduled("b").unwrap().is_none(), "deleted message must not send");
    }
}
