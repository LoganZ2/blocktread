//! Accounts and their progress, in this server's SQLite file. Each server keeps its own: an
//! account belongs to the region it was made in.
//!
//! Passwords are kept as argon2 hashes. Logging in hands out a session token (kept only as its
//! SHA-256), so a client remembers that rather than the password. Hashing is slow on purpose, so
//! the store runs on a thread of its own (`StoreThread`) and the 60 Hz loop never waits on it.
//!
//! The names taken by accounts are also kept in memory (`Registered`), so the hub can check a
//! guest's or an AI's name at once: nobody but an account's owner goes by its name.
//!
//! "Today" (the day's first win) is the server's local date: its time zone, or `TZ`.

use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use blocktread_shared::blocks::BlockType;
use blocktread_shared::economy::{Reward, Showing, Unlocks, economy};
use blocktread_shared::protocol::{AuthReq, Profile};
use crossbeam_channel::{Receiver, Sender, unbounded};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Account names in use, folded to lower case: shared between the store and the hub.
#[derive(Clone, Default)]
pub struct Registered(Arc<Mutex<HashSet<String>>>);

impl Registered {
    /// Whether an account goes by `name` (case aside).
    pub fn has(&self, name: &str) -> bool {
        self.0.lock().unwrap().contains(&name.trim().to_lowercase())
    }

    fn add(&self, name: &str) {
        self.0.lock().unwrap().insert(name.trim().to_lowercase());
    }
}

/// Today in the server's time zone, as days since 1 January 1970.
pub fn local_day() -> i64 {
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
    (chrono::Local::now().date_naive() - epoch).num_days()
}

/// Where a server keeps its accounts.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Accounts {
    /// None: no accounts and no progression rules (a server inside the client, tests of the rest).
    #[default]
    Off,
    /// Kept in memory, gone on restart (a dev server without `--data`, tests).
    Memory,
    /// Kept in this file.
    File(PathBuf),
}

/// A logged-in account.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub id: i64,
    pub user: String,
    pub token: String,
    pub profile: Profile,
}

pub struct Store {
    db: Connection,
    rng: SystemRandom,
    names: Registered,
}

pub const USER_MIN: usize = 2;
pub const USER_MAX: usize = 12;
pub const PASS_MIN: usize = 6;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn sha256_hex(s: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// A user name as it is kept: trimmed, or why it can't be one.
pub fn clean_user(user: &str) -> Result<String, String> {
    let u = user.trim();
    let n = u.chars().count();
    if !(USER_MIN..=USER_MAX).contains(&n) {
        return Err(format!("用户名要 {USER_MIN}–{USER_MAX} 个字"));
    }
    if u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("用户名不能有空格".into());
    }
    Ok(u.to_string())
}

impl Store {
    pub fn open(at: Option<&Path>) -> rusqlite::Result<Store> {
        let db = match at {
            Some(p) => Connection::open(p)?,
            None => Connection::open_in_memory()?,
        };
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS accounts (
                 id INTEGER PRIMARY KEY,
                 user TEXT NOT NULL UNIQUE COLLATE NOCASE,
                 pass TEXT NOT NULL,
                 created INTEGER NOT NULL,
                 coins INTEGER NOT NULL DEFAULT 0,
                 xp INTEGER NOT NULL DEFAULT 0,
                 unlocks INTEGER NOT NULL DEFAULT 0,
                 last_win_day INTEGER NOT NULL DEFAULT -1
             );
             CREATE TABLE IF NOT EXISTS sessions (
                 token TEXT PRIMARY KEY,
                 account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 created INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS rewards (
                 account INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                 game TEXT NOT NULL,
                 coins INTEGER NOT NULL,
                 xp INTEGER NOT NULL,
                 at INTEGER NOT NULL,
                 PRIMARY KEY (account, game)
             );",
        )?;
        let names = Registered::default();
        {
            let mut q = db.prepare("SELECT user FROM accounts")?;
            for user in q.query_map([], |r| r.get::<_, String>(0))? {
                names.add(&user?);
            }
        }
        Ok(Store { db, rng: SystemRandom::new(), names })
    }

    fn token(&self) -> String {
        let mut b = [0u8; 24];
        self.rng.fill(&mut b).expect("system randomness");
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    fn new_session(&self, id: i64, user: String) -> rusqlite::Result<Session> {
        let token = self.token();
        self.db.execute("INSERT INTO sessions (token, account, created) VALUES (?1, ?2, ?3)", params![sha256_hex(&token), id, now()])?;
        Ok(Session { id, user, token, profile: self.profile(id)? })
    }

    pub fn profile(&self, id: i64) -> rusqlite::Result<Profile> {
        self.db.query_row("SELECT coins, xp, unlocks FROM accounts WHERE id = ?1", [id], |r| {
            Ok(Profile { coins: r.get::<_, i64>(0)? as u32, xp: r.get::<_, i64>(1)? as u32, unlocks: r.get::<_, i64>(2)? as u64 })
        })
    }

    pub fn register(&mut self, user: &str, pass: &str) -> Result<Session, String> {
        let user = clean_user(user)?;
        if pass.chars().count() < PASS_MIN {
            return Err(format!("密码至少 {PASS_MIN} 位"));
        }
        let hash = Argon2::default().hash_password(pass.as_bytes()).map_err(|e| format!("密码处理失败：{e}"))?.to_string();
        match self.db.execute("INSERT INTO accounts (user, pass, created) VALUES (?1, ?2, ?3)", params![user, hash, now()]) {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => return Err("这个用户名已经有人用了".into()),
            Err(e) => return Err(e.to_string()),
        }
        let id = self.db.last_insert_rowid();
        self.names.add(&user);
        self.new_session(id, user).map_err(|e| e.to_string())
    }

    pub fn login(&mut self, user: &str, pass: &str) -> Result<Session, String> {
        let wrong = || "用户名或密码不对".to_string();
        let row: Option<(i64, String, String)> =
            self.db.query_row("SELECT id, user, pass FROM accounts WHERE user = ?1", [user.trim()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional().map_err(|e| e.to_string())?;
        let Some((id, user, hash)) = row else { return Err(wrong()) };
        Argon2::default().verify_password(pass.as_bytes(), hash.as_str()).map_err(|_| wrong())?;
        self.new_session(id, user).map_err(|e| e.to_string())
    }

    pub fn resume(&mut self, token: &str) -> Result<Session, String> {
        let row: Option<(i64, String)> = self
            .db
            .query_row("SELECT a.id, a.user FROM sessions s JOIN accounts a ON a.id = s.account WHERE s.token = ?1", [sha256_hex(token)], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((id, user)) = row else { return Err("登录已失效，请重新登录".into()) };
        Ok(Session { id, user, token: token.to_string(), profile: self.profile(id).map_err(|e| e.to_string())? })
    }

    pub fn logout(&mut self, token: &str) {
        let _ = self.db.execute("DELETE FROM sessions WHERE token = ?1", [sha256_hex(token)]);
    }

    pub fn auth(&mut self, req: &AuthReq) -> Result<Session, String> {
        match req {
            AuthReq::Register { user, pass } => self.register(user, pass),
            AuthReq::Login { user, pass } => self.login(user, pass),
            AuthReq::Resume { token } => self.resume(token),
        }
    }

    pub fn unlock(&mut self, id: i64, ty: BlockType) -> Result<Profile, String> {
        let p = self.profile(id).map_err(|e| e.to_string())?;
        let have = Unlocks(p.unlocks);
        if have.has(ty) {
            return Ok(p);
        }
        let price = economy().price(ty).unwrap_or(0);
        if p.coins < price {
            return Err(format!("金币不够：要 {price}，你有 {}", p.coins));
        }
        // the coins and the check in one statement: two unlocks at once can't overspend
        let n = self
            .db
            .execute("UPDATE accounts SET coins = coins - ?2, unlocks = ?3 WHERE id = ?1 AND coins >= ?2", params![id, price, have.with(ty).0 as i64])
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("金币不够".into());
        }
        self.profile(id).map_err(|e| e.to_string())
    }

    /// Pay `id` for a showing in game `game` (once: paying the same game again pays nothing).
    /// Returns what it paid, the profile after, and the level reached if it went up.
    pub fn reward(&mut self, id: i64, game: &str, s: &Showing) -> rusqlite::Result<(Reward, Profile, Option<u32>)> {
        self.reward_on(id, game, s, local_day())
    }

    /// `reward` on a given day (`local_day`'s count).
    pub fn reward_on(&mut self, id: i64, game: &str, s: &Showing, day: i64) -> rusqlite::Result<(Reward, Profile, Option<u32>)> {
        let e = economy();
        let tx = self.db.transaction()?;
        let (xp, last_win_day): (i64, i64) = tx.query_row("SELECT xp, last_win_day FROM accounts WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let first_today = last_win_day != day;
        let mut r = e.reward(s, first_today);
        let paid = tx.execute("INSERT OR IGNORE INTO rewards (account, game, coins, xp, at) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, game, r.coins, r.xp, now()])?;
        let mut up = None;
        if paid == 0 {
            r = Reward::default();
        } else {
            let (before, after) = (e.level(xp as u32).0, e.level(xp as u32 + r.xp).0);
            let bonus = (after - before) * e.level_up_coins;
            up = (after > before).then_some(after);
            let win_day = if first_today && blocktread_shared::economy::Economy::counts_as_win(s) { day } else { last_win_day };
            tx.execute("UPDATE accounts SET coins = coins + ?2, xp = xp + ?3, last_win_day = ?4 WHERE id = ?1", params![id, r.coins + bonus, r.xp, win_day])?;
            r.coins += bonus;
        }
        tx.commit()?;
        Ok((r, self.profile(id)?, up))
    }
}

/// Work for the store's thread, each with the connection it is for.
pub enum Job {
    Auth { pid: u16, req: AuthReq },
    Logout { token: String },
    Unlock { pid: u16, account: i64, block: BlockType },
    Reward { account: i64, game: String, showing: Showing },
}

/// What came of it, back on the 60 Hz loop.
pub enum Done {
    Auth { pid: u16, result: Result<Session, String> },
    Unlock { pid: u16, account: i64, result: Result<Profile, String> },
    Reward { account: i64, reward: Reward, profile: Profile, level_up: Option<u32> },
}

/// The store on a thread of its own: jobs in, results out, never blocking the caller.
pub struct StoreThread {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    pub names: Registered,
}

impl StoreThread {
    pub fn start(at: &Accounts) -> anyhow::Result<Option<StoreThread>> {
        let path = match at {
            Accounts::Off => return Ok(None),
            Accounts::Memory => None,
            Accounts::File(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Some(p.clone())
            }
        };
        // opened here so a bad file stops the server at start, not later
        let mut store = Store::open(path.as_deref())?;
        let names = store.names.clone();
        let (jobs, job_rx) = unbounded::<Job>();
        let (done_tx, done) = unbounded::<Done>();
        std::thread::Builder::new().name("blocktread-store".into()).spawn(move || {
            for job in job_rx {
                let out = match job {
                    Job::Auth { pid, req } => Some(Done::Auth { pid, result: store.auth(&req) }),
                    Job::Logout { token } => {
                        store.logout(&token);
                        None
                    }
                    Job::Unlock { pid, account, block } => Some(Done::Unlock { pid, account, result: store.unlock(account, block) }),
                    Job::Reward { account, game, showing } => match store.reward(account, &game, &showing) {
                        Ok((reward, profile, level_up)) => Some(Done::Reward { account, reward, profile, level_up }),
                        Err(e) => {
                            eprintln!("[store] reward for account {account} in {game} failed: {e}");
                            None
                        }
                    },
                };
                if let Some(d) = out
                    && done_tx.send(d).is_err()
                {
                    break;
                }
            }
        })?;
        Ok(Some(StoreThread { jobs, done, names }))
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    /// Results ready now.
    pub fn ready(&self) -> Vec<Done> {
        self.done.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_log_in_resume_and_log_out() {
        let mut s = Store::open(None).unwrap();
        let a = s.register("老王", "secret1").unwrap();
        assert_eq!(a.profile, Profile::default());
        assert!(s.register("老王", "another").is_err(), "names are taken once");
        assert!(s.register(" x ", "secret1").is_err(), "too short");
        assert!(s.register("新人", "123").is_err(), "password too short");
        assert!(s.login("老王", "wrong").is_err());
        let b = s.login("老王", "secret1").unwrap();
        assert_eq!(b.id, a.id);
        assert_ne!(b.token, a.token, "each login its own session");
        assert_eq!(s.resume(&a.token).unwrap().id, a.id);
        s.logout(&a.token);
        assert!(s.resume(&a.token).is_err());
        assert!(s.resume(&b.token).is_ok(), "the other session lives on");
    }

    #[test]
    fn coins_buy_parts_and_a_game_pays_once() {
        let mut s = Store::open(None).unwrap();
        let a = s.register("车手", "secret1").unwrap();
        assert!(s.unlock(a.id, BlockType::Hover).is_err(), "no coins yet");
        let full = Showing { secs: 480.0, kills: 4, won: true, place: None };
        let (r, p, up) = s.reward(a.id, "g1", &full).unwrap();
        assert!(r.coins >= 150 + 160, "the day's first win pays extra: {r:?}");
        assert_eq!(up, Some(2), "and it is enough for a level");
        let (again, p2, _) = s.reward(a.id, "g1", &full).unwrap();
        assert_eq!(again, Reward::default(), "the same game pays once");
        assert_eq!(p2, p);
        // the second win of the day pays without the bonus
        let (r2, _, _) = s.reward(a.id, "g2", &full).unwrap();
        assert_eq!(r2.coins, 80 + 40 + 40);
        for i in 0..10 {
            s.reward(a.id, &format!("g{}", i + 3), &full).unwrap();
        }
        let before = s.profile(a.id).unwrap();
        let after = s.unlock(a.id, BlockType::Hover).unwrap();
        assert!(Unlocks(after.unlocks).has(BlockType::Hover));
        assert_eq!(after.coins, before.coins - 1000);
        assert_eq!(s.unlock(a.id, BlockType::Hover).unwrap().coins, after.coins, "buying it twice costs nothing");
    }

    #[test]
    fn the_first_win_bonus_comes_once_a_day() {
        let mut s = Store::open(None).unwrap();
        let a = s.register("车手", "secret1").unwrap();
        let win = Showing { secs: 60.0, kills: 0, won: true, place: None };
        // XP: what the match paid, without the coins a new level adds
        assert_eq!(s.reward_on(a.id, "g1", &win, 100).unwrap().0.xp, 10 + 40 + 150);
        assert_eq!(s.reward_on(a.id, "g2", &win, 100).unwrap().0.xp, 10 + 40, "once that day");
        assert_eq!(s.reward_on(a.id, "g3", &win, 101).unwrap().0.xp, 10 + 40 + 150, "and again the next");
    }

    #[test]
    fn names_taken_by_accounts_are_known_case_aside() {
        let mut s = Store::open(None).unwrap();
        s.register("TankGirl", "secret1").unwrap();
        assert!(s.names.has("tankgirl") && s.names.has(" TANKGIRL "));
        assert!(!s.names.has("TankBoy"));
        // and known again after a restart, from the file
        let dir = std::env::temp_dir().join(format!("blocktread-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.sqlite");
        Store::open(Some(&path)).unwrap().register("老兵", "secret1").unwrap();
        assert!(Store::open(Some(&path)).unwrap().names.has("老兵"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accounts_outlive_a_restart() {
        let dir = std::env::temp_dir().join(format!("blocktread-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.sqlite");
        let token = Store::open(Some(&path)).unwrap().register("老兵", "secret1").unwrap().token;
        assert_eq!(Store::open(Some(&path)).unwrap().resume(&token).unwrap().user, "老兵");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
