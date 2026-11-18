//! The app's side: which index serves each patch, building a missing one in the background, letting go of idle ones.

use crate::build::{RefreshKind, refresh};
use crate::file::Index;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How the app uses the index. `HERTZ_EXPLORE_INDEX` picks it: unset is [`Mode::Background`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A missing index is built on a background thread; questions are answered from SQL until it is ready.
    Background,
    /// `wait`: a missing index is built before the question is answered (tests, to prove the index answers).
    Wait,
    /// `off`: always SQL.
    Off,
}

impl Mode {
    pub fn from_env() -> Mode {
        match std::env::var("HERTZ_EXPLORE_INDEX").as_deref() {
            Ok("off") => Mode::Off,
            Ok("wait") => Mode::Wait,
            _ => Mode::Background,
        }
    }
}

/// A newer file is looked for at most this often per patch.
const RESCAN: Duration = Duration::from_secs(1);
/// After a build fails (or finds no boards), it is tried again after this long.
const RETRY: Duration = Duration::from_secs(600);

#[derive(Default)]
struct Slot {
    index: Option<Arc<Index>>,
    building: bool,
    last_used: Option<Instant>,
    scanned: Option<Instant>,
    /// A file that would not open: not tried again.
    bad: Option<PathBuf>,
    failed: Option<(Instant, String)>,
}

/// One patch as the Data page shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchStatus {
    pub patch: String,
    /// ready (in use), idle (on disk, let go after idle minutes), building, failed
    pub state: &'static str,
    pub boards: Option<u64>,
    pub bytes: Option<u64>,
    pub built_at: Option<i64>,
    pub problem: Option<String>,
}

pub struct Explorer {
    db: PathBuf,
    dir: PathBuf,
    mode: Mode,
    slots: Mutex<HashMap<String, Slot>>,
}

impl Explorer {
    pub fn new(db: &Path, mode: Mode) -> Arc<Explorer> {
        Arc::new(Explorer { db: db.to_path_buf(), dir: crate::index_dir(db), mode, slots: Mutex::default() })
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The index to answer a question on `patch` from, or None to answer from SQL. Picks up a newer snapshot when
    /// the stats step has written one; a question already running keeps the snapshot it started with.
    pub fn get(self: &Arc<Self>, patch: &str) -> Option<Arc<Index>> {
        if self.mode == Mode::Off || patch.is_empty() {
            return None;
        }
        let now = Instant::now();
        {
            let mut slots = self.slots.lock().unwrap();
            let slot = slots.entry(patch.to_string()).or_default();
            if slot.index.is_none() || slot.scanned.is_none_or(|t| now.duration_since(t) >= RESCAN) {
                slot.scanned = Some(now);
                let have = slot.index.as_ref().map_or(0, |ix| ix.generation);
                if let Some((g, path)) = crate::newest(&self.dir, patch)
                    && g > have
                    && slot.bad.as_ref() != Some(&path)
                {
                    match Index::open(&path) {
                        Ok(ix) if ix.dir.patch == patch => {
                            // The older snapshot is let go of here (questions in flight keep it until they end),
                            // and its file deleted when nothing maps it any more: now, or at the next switch or
                            // write. On Windows a mapped file cannot be deleted, so this is where the app's own
                            // mapping stops holding back the stats step's clean-up.
                            slot.index = Some(Arc::new(ix));
                            crate::remove_older(&self.dir, patch, g);
                        }
                        _ => slot.bad = Some(path),
                    }
                }
            }
            if let Some(ix) = &slot.index {
                slot.last_used = Some(now);
                return Some(ix.clone());
            }
            if slot.building || slot.failed.as_ref().is_some_and(|(t, _)| now.duration_since(*t) < RETRY) {
                return None;
            }
            slot.building = true;
        }
        match self.mode {
            Mode::Wait => {
                self.build(patch);
                let mut slots = self.slots.lock().unwrap();
                let slot = slots.get_mut(patch)?;
                slot.last_used = Some(now);
                slot.index.clone()
            }
            _ => {
                let (me, p) = (Arc::clone(self), patch.to_string());
                std::thread::Builder::new().name(format!("explore-index {p}")).spawn(move || me.build(&p)).ok()?;
                None
            }
        }
    }

    /// Builds (or brings up to date) the file for `patch` and puts it in use.
    fn build(&self, patch: &str) {
        let result = Connection::open_with_flags(&self.db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(anyhow::Error::from)
            .and_then(|conn| {
                conn.busy_timeout(Duration::from_secs(10))?;
                refresh(&conn, &self.dir, patch)
            })
            .and_then(|r| match (r.kind, r.path) {
                (RefreshKind::Empty, _) | (_, None) => anyhow::bail!("no boards stored for this patch"),
                (_, Some(p)) => Index::open(&p),
            });
        let mut slots = self.slots.lock().unwrap();
        let slot = slots.entry(patch.to_string()).or_default();
        slot.building = false;
        match result {
            Ok(ix) => {
                let g = ix.generation;
                slot.index = Some(Arc::new(ix));
                crate::remove_older(&self.dir, patch, g);
                slot.failed = None;
                slot.scanned = Some(Instant::now());
            }
            Err(e) => {
                if !e.to_string().starts_with("no boards") {
                    eprintln!("Explorer index for patch {patch} could not be built: {e:#}. Answering from the database.");
                }
                slot.failed = Some((Instant::now(), format!("{e:#}")));
            }
        }
    }

    /// Lets go of indexes not used for `idle` (the file stays; the next question maps it again).
    pub fn release_idle(&self, idle: Duration) {
        let now = Instant::now();
        for slot in self.slots.lock().unwrap().values_mut() {
            if slot.index.is_some() && slot.last_used.is_none_or(|t| now.duration_since(t) >= idle) {
                slot.index = None;
            }
        }
    }

    /// Every patch with a file or a build, newest file first.
    pub fn status(&self) -> Vec<PatchStatus> {
        let slots = self.slots.lock().unwrap();
        let mut out: Vec<PatchStatus> = Vec::new();
        let mut on_disk: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.dir) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "hzx") {
                    on_disk.push((String::new(), e.path()));
                }
            }
        }
        for (_, path) in on_disk {
            let Ok(ix) = Index::open(&path) else { continue };
            let patch = ix.dir.patch.clone();
            if crate::newest(&self.dir, &patch).is_some_and(|(_, p)| p != path) {
                continue; // an older snapshot still being let go of
            }
            let slot = slots.get(&patch);
            let state = match slot {
                Some(s) if s.building => "building",
                Some(s) if s.index.is_some() => "ready",
                _ => "idle",
            };
            out.push(PatchStatus {
                patch,
                state,
                boards: Some(ix.dir.rows),
                bytes: Some(ix.bytes()),
                built_at: Some(ix.dir.built_at),
                problem: None,
            });
        }
        for (patch, s) in slots.iter() {
            if out.iter().any(|o| &o.patch == patch) {
                continue;
            }
            if s.building {
                out.push(PatchStatus { patch: patch.clone(), state: "building", boards: None, bytes: None, built_at: None, problem: None });
            } else if let Some((_, e)) = &s.failed
                && !e.starts_with("no boards")
            {
                out.push(PatchStatus { patch: patch.clone(), state: "failed", boards: None, bytes: None, built_at: None, problem: Some(e.clone()) });
            }
        }
        out.sort_by(|a, b| b.built_at.cmp(&a.built_at).then_with(|| a.patch.cmp(&b.patch)));
        out
    }
}
