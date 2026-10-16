//! The file-based `sushiai ask`, the only ask channel of a routine run: the run writes
//! `<routine>/work/asks/<id>.req`; while the run is live the service forwards it to the
//! daemon's owner-ask (`hook.ask` of the run's session) and writes `<id>.ans`. The owner's
//! "yes" is recorded by this module alone, in `<routine>/state/asks/<session>/`, a folder a
//! run cannot write.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use super::*;
use crate::store::{ask_inbox, plain_token};

static ANSWER_SEQ: AtomicU64 = AtomicU64::new(0);

/// How often the inbox of a live run is looked at.
const ASK_POLL: Duration = Duration::from_millis(500);

impl Routines {
    /// Serves the ask inbox of run `n` until it concludes, then removes what it left.
    pub(super) fn watch_asks(&self, id: &str, n: u64) {
        let me = self.clone();
        let id = id.to_string();
        let _ = std::thread::Builder::new()
            .name("routines-asks".into())
            .spawn(move || loop {
                match me.inner.store.run(&id, n) {
                    Some(run) if run["status"] == "running" => me.serve_asks(&id, &run, false),
                    Some(run) => return me.serve_asks(&id, &run, true),
                    None => return,
                }
                std::thread::sleep(ASK_POLL);
            });
    }

    /// One pass over the inbox for the sessions of `run`. When `finished`, requests of those
    /// sessions are removed instead of served.
    pub(super) fn serve_asks(&self, id: &str, run: &Value, finished: bool) {
        let sessions: Vec<String> = ["session", "retrySession"]
            .iter()
            .filter_map(|k| run[*k].as_str().filter(|s| valid_session(s)))
            .map(String::from)
            .collect();
        let inbox = ask_inbox(&self.inner.store.dir(id));
        for entry in std::fs::read_dir(&inbox).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((stem, ext)) = name.rsplit_once('.') else {
                continue;
            };
            if !plain_token(stem) || !matches!(ext, "req" | "work") || !path.is_file() {
                continue;
            }
            let Ok(req) = std::fs::read_to_string(&path)
                .map_err(|_| ())
                .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ()))
            else {
                continue;
            };
            if !req["session"]
                .as_str()
                .is_some_and(|s| sessions.iter().any(|x| x == s))
            {
                continue;
            }
            if finished {
                let _ = std::fs::remove_file(&path);
                let _ = std::fs::remove_file(inbox.join(format!("{stem}.ans")));
                continue;
            }
            if ext == "work" {
                continue;
            }
            let age = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| SystemTime::now().duration_since(t).ok());
            let late = match (req["ttlSec"].as_u64(), age) {
                (Some(ttl), Some(age)) => age >= Duration::from_secs(ttl),
                _ => false,
            };
            // Claiming the request keeps the next pass from forwarding it again.
            let work = inbox.join(format!("{stem}.work"));
            if std::fs::rename(&path, &work).is_err() {
                continue;
            }
            let me = self.clone();
            let (id, stem, inbox) = (id.to_string(), stem.to_string(), inbox.clone());
            std::thread::spawn(move || me.answer_ask(&id, &inbox, &stem, &req, late));
        }
    }

    /// Asks the owner through the daemon and writes the answer file. An answer for a request
    /// whose run already ended (its `.work` file is gone) is dropped.
    fn answer_ask(&self, id: &str, inbox: &Path, stem: &str, req: &Value, late: bool) {
        let mut params = json!({
            "session": req["session"], "token": req["token"], "agent": req["agent"],
            "text": req["text"], "options": req["options"],
        });
        if let Some(ttl) = req["ttlSec"].as_u64() {
            params["ttlSec"] = json!(ttl);
        }
        let asked = if late {
            Ok(Value::Null)
        } else {
            self.conn().hook_ask(params)
        };
        let answer = match asked {
            Ok(Value::Null) => json!({"expired": true}),
            Ok(a) => {
                if a["decision"] == "allow" {
                    self.record_yes(id, req, stem);
                }
                json!({
                    "decision": a["decision"], "choice": a["choice"], "message": a["message"],
                })
            }
            Err(e) => json!({"decision": "deny", "message": clip(&e, 300)}),
        };
        let work = inbox.join(format!("{stem}.work"));
        if !work.exists() {
            return;
        }
        // A fresh name, opened exclusively: a file or symlink the run planted is never
        // written through.
        let tmp = inbox.join(format!(
            "{stem}.{}-{}.tmp",
            std::process::id(),
            ANSWER_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut f| std::io::Write::write_all(&mut f, answer.to_string().as_bytes()));
        if written.is_ok() {
            let _ = std::fs::rename(&tmp, inbox.join(format!("{stem}.ans")));
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
        let _ = std::fs::remove_file(work);
    }

    /// Counts the owner's "yes": one record in the session's daemon-owned folder, read when
    /// the run concludes. Called only after the owner allowed the ask.
    fn record_yes(&self, id: &str, req: &Value, stem: &str) {
        let Some(session) = req["session"].as_str().filter(|s| valid_session(s)) else {
            return;
        };
        let dir = self.inner.store.asks_dir(id, session);
        let at = self.now();
        let _ = std::fs::create_dir_all(&dir).and_then(|()| {
            std::fs::write(
                dir.join(format!("{at}-{stem}.json")),
                json!({"kind": ask_kind(req["kind"].as_str()), "at": at}).to_string(),
            )
        });
    }
}
