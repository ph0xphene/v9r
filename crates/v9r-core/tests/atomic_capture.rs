//! Atomic capture: can a snapshot built from many system calls be shown to
//! represent a state that actually existed?
//!
//! The adversary is a same-user process mutating the directory while it is
//! being captured. Deterministic interleavings are produced by
//! `Interleaver`, an observer that applies mutations to the real directory
//! immediately before answering chosen requests. The ground truth is
//! computed, not assumed: the logged mutations are replayed on a scratch
//! copy, and every state that existed is snapshotted. A capture is a
//! **false snapshot** iff its root is none of them.
//!
//! Strategies: `PLAIN` (one walk), `A` (walk twice, compare roots), `B`
//! (`lstat` before and after, values only from inside the bracket), `W`
//! (inotify change journal), and combinations.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use uuid::Uuid;
use v9r_core::fs_raw::RawFsObserver;
use v9r_core::fs_watch::WatchObserver;
use v9r_core::graph::{Answer, Attestor, EvidenceProvider, Key, Registry, Term, Trust};
use v9r_core::snapshot::{Capture, FsSnapshot, ObjectStore};

const DIST: &str = "dist";

const PLAIN: Capture = Capture {
    double_walk: false,
    recheck: false,
    watch: false,
};
const A: Capture = Capture {
    double_walk: true,
    ..PLAIN
};
const B: Capture = Capture {
    recheck: true,
    ..PLAIN
};
const W: Capture = Capture {
    watch: true,
    ..PLAIN
};
const ALL: Capture = Capture {
    double_walk: true,
    recheck: true,
    watch: true,
};

// ------------------------------------------------------------ mutations

#[derive(Clone, Debug)]
enum Op {
    Write(&'static str, Vec<u8>),
    Remove(&'static str),
    Rename(&'static str, &'static str),
    Chmod(&'static str, u32),
    Relink(&'static str, &'static str),
    HardLink(&'static str, &'static str),
    Sleep(u64),
    /// Atomic replace: write aside (outside/), then rename into place.
    Replace(&'static str, Vec<u8>),
    /// Atomic retarget: new link aside, then rename over the old one.
    Retarget(&'static str, &'static str),
}

fn apply(root: &Path, op: &Op) {
    match op {
        Op::Write(p, bytes) => fs::write(root.join(p), bytes).unwrap(),
        Op::Remove(p) => fs::remove_file(root.join(p)).unwrap(),
        Op::Rename(a, b) => fs::rename(root.join(a), root.join(b)).unwrap(),
        Op::Chmod(p, mode) => {
            fs::set_permissions(root.join(p), fs::Permissions::from_mode(*mode)).unwrap()
        }
        Op::Relink(p, target) => {
            fs::remove_file(root.join(p)).unwrap();
            std::os::unix::fs::symlink(target, root.join(p)).unwrap();
        }
        Op::HardLink(existing, new) => fs::hard_link(root.join(existing), root.join(new)).unwrap(),
        Op::Sleep(ms) => std::thread::sleep(std::time::Duration::from_millis(*ms)),
        Op::Replace(p, bytes) => {
            let aside = root.join("outside/.replace");
            fs::write(&aside, bytes).unwrap();
            fs::rename(&aside, root.join(p)).unwrap();
        }
        Op::Retarget(p, target) => {
            let aside = root.join("outside/.retarget");
            let _ = fs::remove_file(&aside);
            std::os::unix::fs::symlink(target, &aside).unwrap();
            fs::rename(&aside, root.join(p)).unwrap();
        }
    }
}

/// dist/a/{link -> x, run.sh, x}, dist/m/big (64 KiB), dist/z/{sub/s, w};
/// outside/ for hard links. Then wait, so that later changes fall in a
/// later timestamp tick than the setup.
fn populate(root: &Path, setup: &[Op]) {
    for dir in ["dist/a", "dist/m", "dist/z/sub", "outside"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(root.join("dist/a/x"), "x0").unwrap();
    fs::write(root.join("dist/a/run.sh"), "run").unwrap();
    fs::set_permissions(
        root.join("dist/a/run.sh"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::os::unix::fs::symlink("x", root.join("dist/a/link")).unwrap();
    fs::write(root.join("dist/m/big"), vec![b'A'; 64 * 1024]).unwrap();
    fs::write(root.join("dist/z/w"), "w0").unwrap();
    fs::write(root.join("dist/z/sub/s"), "s0").unwrap();
    for op in setup {
        apply(root, op);
    }
    std::thread::sleep(std::time::Duration::from_millis(5));
}

// ------------------------------------------------------------ interleaving

/// Fire `ops` immediately before answering the first `times` requests for
/// `kind(path[, tag])`. With `torn`, the ops happen in the middle of
/// reading the file instead (a read of a file being rewritten).
struct Trigger {
    kind: &'static str,
    path: &'static str,
    tag: Option<&'static str>,
    ops: Vec<Op>,
    times: usize,
    torn: bool,
}

fn before(kind: &'static str, path: &'static str, ops: Vec<Op>) -> Trigger {
    Trigger {
        kind,
        path,
        tag: None,
        ops,
        times: 1,
        torn: false,
    }
}

fn tagged(mut t: Trigger, tag: &'static str) -> Trigger {
    t.tag = Some(tag);
    t
}

struct Interleaver {
    inner: RawFsObserver,
    root: PathBuf,
    triggers: Mutex<Vec<Trigger>>,
    log: Arc<Mutex<Vec<Op>>>,
}

impl EvidenceProvider for Interleaver {
    fn id(&self) -> &str {
        "fs"
    }
    fn answers(&self, key: &Key) -> bool {
        self.inner.answers(key)
    }
    fn provide(&self, keys: &[&Key], attestor: &Attestor) -> Vec<Answer> {
        let mut out = Vec::new();
        for key in keys {
            let mut torn = None;
            for t in self.triggers.lock().unwrap().iter_mut() {
                let tag_ok = t
                    .tag
                    .is_none_or(|tag| key.args.get(1).map(String::as_str) == Some(tag));
                if t.times == 0 || key.kind != t.kind || key.args[0] != t.path || !tag_ok {
                    continue;
                }
                t.times -= 1;
                if t.torn {
                    torn = Some(t.ops.clone());
                } else {
                    for op in &t.ops {
                        apply(&self.root, op);
                        self.log.lock().unwrap().push(op.clone());
                    }
                }
            }
            match torn {
                Some(ops) => {
                    // Read the first half, the file is rewritten, read the rest.
                    let path = self.root.join(&key.args[0]);
                    let old = fs::read(&path).unwrap();
                    let half = old.len() / 2;
                    for op in &ops {
                        apply(&self.root, op);
                        self.log.lock().unwrap().push(op.clone());
                    }
                    let mut mixed = old[..half].to_vec();
                    mixed.extend_from_slice(&fs::read(&path).unwrap()[half..]);
                    out.push(Answer::Verified(attestor.attest(
                        (*key).clone(),
                        Term::Bytes(mixed),
                        "read",
                    )));
                }
                None => out.extend(self.inner.provide(&[key], attestor)),
            }
        }
        out
    }
}

// ------------------------------------------------------------ harness

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// A snapshot of a state that existed.
    True,
    /// A snapshot of a state that never existed.
    False,
    Blocked(String),
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("v9r-atomic-{name}-{}", Uuid::new_v4()))
}

fn snapshot(registry: &Registry) -> Result<String, String> {
    match registry.query(&Key::new("snapshot", [DIST])) {
        Some(Term::Id(root)) => Ok(root),
        _ => Err(registry
            .discarded()
            .iter()
            .rev()
            .find(|d| d.provider == "verifier:fs-snapshot")
            .map_or("no snapshot".to_string(), |d| d.reason.clone())),
    }
}

fn honest_root(root: &Path) -> String {
    let registry = Registry::new();
    let store = ObjectStore::new();
    registry.register(RawFsObserver::new("fs", root), Trust::Attesting);
    registry.register(store.clone(), Trust::ClaimsOnly);
    registry.add_verifier(FsSnapshot::new(store));
    snapshot(&registry).expect("honest snapshot")
}

/// Every root the directory actually had: initial state, then after each
/// logged mutation.
fn existed(setup: &[Op], log: &[Op]) -> BTreeSet<String> {
    let root = scratch("replay");
    populate(&root, setup);
    let mut roots = BTreeSet::from([honest_root(&root)]);
    for op in log {
        if !matches!(op, Op::Sleep(_)) {
            apply(&root, op);
            roots.insert(honest_root(&root));
        }
    }
    let _ = fs::remove_dir_all(&root);
    roots
}

fn capture(strategy: Capture, setup: Vec<Op>, triggers: Vec<Trigger>) -> Outcome {
    let root = scratch("live");
    populate(&root, &setup);
    let log = Arc::new(Mutex::new(Vec::new()));
    let registry = Registry::new();
    let store = ObjectStore::new();
    registry.register(WatchObserver::new("watch", &root), Trust::Attesting);
    registry.register(
        Interleaver {
            inner: RawFsObserver::new("raw", &root),
            root: root.clone(),
            triggers: Mutex::new(triggers),
            log: log.clone(),
        },
        Trust::Attesting,
    );
    registry.register(store.clone(), Trust::ClaimsOnly);
    registry.add_verifier(FsSnapshot::with(store, strategy));
    let result = snapshot(&registry);
    let log = log.lock().unwrap().clone();
    let _ = fs::remove_dir_all(&root);
    match result {
        Err(reason) => Outcome::Blocked(reason),
        Ok(r) if existed(&setup, &log).contains(&r) => Outcome::True,
        Ok(_) => Outcome::False,
    }
}

fn blocked(o: &Outcome, needle: &str) -> bool {
    matches!(o, Outcome::Blocked(r) if r.contains(needle))
}

// ------------------------------------------------------------ the cases

/// The file is rewritten while it is being read.
fn torn() -> Vec<Trigger> {
    vec![Trigger {
        torn: true,
        ..before(
            "fs_file",
            "dist/m/big",
            vec![Op::Write("dist/m/big", vec![b'B'; 64 * 1024])],
        )
    }]
}

#[test]
fn file_content_changing_while_read() {
    assert_eq!(capture(PLAIN, vec![], torn()), Outcome::False);
    assert!(blocked(&capture(A, vec![], torn()), "second walk"));
    assert!(blocked(
        &capture(B, vec![], torn()),
        "dist/m/big changed during capture"
    ));
    assert!(blocked(&capture(W, vec![], torn()), "change event"));
}

#[test]
fn file_deleted_during_snapshot_is_incomplete_under_every_strategy() {
    let case = || vec![before("fs_file", "dist/z/w", vec![Op::Remove("dist/z/w")])];
    for strategy in [PLAIN, A, B, W] {
        assert!(
            blocked(&capture(strategy, vec![], case()), "dist/z/w"),
            "{strategy:?}"
        );
    }
}

/// `sub` moves from a not-yet-listed directory into an already-listed one:
/// the snapshot has it nowhere, which never happened (rename is atomic).
fn moved_into_walked_dir() -> Vec<Trigger> {
    vec![before(
        "fs_dir",
        "dist/z",
        vec![Op::Rename("dist/z/sub", "dist/a/sub")],
    )]
}

#[test]
fn directory_renamed_during_traversal() {
    assert_eq!(
        capture(PLAIN, vec![], moved_into_walked_dir()),
        Outcome::False
    );
    assert!(blocked(
        &capture(A, vec![], moved_into_walked_dir()),
        "second walk"
    ));
    assert!(blocked(
        &capture(B, vec![], moved_into_walked_dir()),
        "dist/a changed during capture"
    ));
    assert!(blocked(
        &capture(W, vec![], moved_into_walked_dir()),
        "change event"
    ));
}

#[test]
fn an_adaptive_adversary_repeats_the_rename_and_defeats_the_second_walk() {
    // It moves `sub` back before the second walk and again at the same
    // point in it: both walks find the same impossible state.
    let adaptive = || {
        vec![
            tagged(moved_into_walked_dir().pop().unwrap(), "w1"),
            tagged(
                before(
                    "fs_dir",
                    "dist",
                    vec![Op::Rename("dist/a/sub", "dist/z/sub")],
                ),
                "w2",
            ),
            tagged(moved_into_walked_dir().pop().unwrap(), "w2"),
        ]
    };
    assert_eq!(capture(A, vec![], adaptive()), Outcome::False);
    assert!(matches!(
        capture(Capture { recheck: true, ..A }, vec![], adaptive()),
        Outcome::Blocked(_)
    ));
    assert!(matches!(
        capture(Capture { watch: true, ..A }, vec![], adaptive()),
        Outcome::Blocked(_)
    ));
}

#[test]
fn permission_changed_during_traversal() {
    // chmod an already-listed file, then change a file not yet read.
    let case = || {
        vec![before(
            "fs_dir",
            "dist/z",
            vec![
                Op::Chmod("dist/a/run.sh", 0o755),
                Op::Write("dist/z/w", b"w1".to_vec()),
            ],
        )]
    };
    assert_eq!(capture(PLAIN, vec![], case()), Outcome::False);
    assert!(blocked(&capture(A, vec![], case()), "second walk"));
    // B takes the mode from its own bracketed lstat, after the chmod: the
    // result is the state after both changes, which existed.
    assert_eq!(capture(B, vec![], case()), Outcome::True);
    assert!(blocked(&capture(W, vec![], case()), "change event"));
}

#[test]
fn symlink_target_changed_during_traversal() {
    // Retarget an already-read link, then change a file not yet read.
    let case = || {
        vec![before(
            "fs_file",
            "dist/z/w",
            vec![
                Op::Relink("dist/a/link", "run.sh"),
                Op::Write("dist/z/w", b"w1".to_vec()),
            ],
        )]
    };
    assert_eq!(capture(PLAIN, vec![], case()), Outcome::False);
    assert!(blocked(&capture(A, vec![], case()), "second walk"));
    // Retargeting removes and recreates an entry: dist/a itself changes.
    let b = capture(B, vec![], case());
    assert!(blocked(&b, "dist/a changed during capture"), "{b:?}");
    assert!(blocked(&capture(W, vec![], case()), "change event"));
}

// ------------------------------------------------------------ where B and W fail

/// x is changed just before its lstat and again just after its read, both
/// within one timestamp tick; then dist/z/sub/s changes before its own
/// bracket begins. The snapshot (x old, s new) never existed.
fn same_tick(via: &'static str, s_via: &'static str) -> Vec<Trigger> {
    vec![
        before("fs_meta", "dist/a/x", vec![Op::Write(via, b"X1".to_vec())]),
        before(
            "fs_file",
            "dist/m/big",
            vec![
                Op::Write(via, b"X2".to_vec()),
                Op::Write(s_via, b"s1".to_vec()),
            ],
        ),
    ]
}

/// Attempts until `found` holds (the timestamp tick is a race).
fn within(attempts: usize, mut f: impl FnMut() -> bool) -> usize {
    (1..=attempts).find(|_| f()).unwrap_or(0)
}

#[test]
fn metadata_recheck_misses_changes_within_one_timestamp_tick() {
    let n = within(50, || {
        capture(B, vec![], same_tick("dist/a/x", "dist/z/sub/s")) == Outcome::False
    });
    assert!(
        n > 0,
        "no same-tick evasion in 50 attempts (finer timestamps?)"
    );
    println!("B evaded on attempt {n}");
    // The change journal is not fooled: x's directory is watched.
    for _ in 0..10 {
        assert!(blocked(
            &capture(W, vec![], same_tick("dist/a/x", "dist/z/sub/s")),
            "change event"
        ));
    }
}

fn linked() -> Vec<Op> {
    vec![
        Op::HardLink("dist/a/x", "outside/x"),
        Op::HardLink("dist/z/sub/s", "outside/s"),
    ]
}

#[test]
fn change_journal_misses_writes_through_a_hard_link_from_outside() {
    // Same mix, written through links in a directory nobody watches.
    let case = || {
        vec![before(
            "fs_file",
            "dist/m/big",
            vec![
                Op::Sleep(3),
                Op::Write("outside/x", b"X2".to_vec()),
                Op::Write("outside/s", b"s1".to_vec()),
            ],
        )]
    };
    assert_eq!(capture(W, linked(), case()), Outcome::False);
    // B sees x's inode change (a later tick: ctime is per inode).
    assert!(blocked(
        &capture(B, linked(), case()),
        "dist/a/x changed during capture"
    ));
    assert!(blocked(&capture(A, linked(), case()), "second walk"));
}

#[test]
fn all_three_strategies_together_accept_a_false_snapshot() {
    // Walk 1: same-tick flip of x and a change to s, through hard links.
    // Recheck: x unchanged in metadata (same tick), s bracketed afterwards.
    // Walk 2: s and x reverted (in that order), then flipped again at the
    // same point. No state with x = X1 and s = s1 ever existed.
    let case = || {
        vec![
            tagged(
                before(
                    "fs_meta",
                    "dist/a/x",
                    vec![Op::Write("outside/x", b"X1".to_vec())],
                ),
                "w1",
            ),
            tagged(
                before(
                    "fs_file",
                    "dist/m/big",
                    vec![
                        Op::Write("outside/x", b"X2".to_vec()),
                        Op::Write("outside/s", b"s1".to_vec()),
                    ],
                ),
                "w1",
            ),
            tagged(
                before(
                    "fs_dir",
                    "dist",
                    vec![
                        Op::Write("outside/s", b"s0".to_vec()),
                        Op::Write("outside/x", b"X1".to_vec()),
                    ],
                ),
                "w2",
            ),
            tagged(
                before(
                    "fs_file",
                    "dist/m/big",
                    vec![
                        Op::Write("outside/x", b"X2".to_vec()),
                        Op::Write("outside/s", b"s1".to_vec()),
                    ],
                ),
                "w2",
            ),
        ]
    };
    let n = within(50, || capture(ALL, linked(), case()) == Outcome::False);
    assert!(n > 0, "A+B+W were not defeated in 50 attempts");
    println!("A+B+W accepted a false snapshot on attempt {n}");
}

// ------------------------------------------------------------ measurements

/// The writer's cycle: each op leads to a state; the cycle returns to the
/// initial state, so the states it visits are exactly these.
fn cycle() -> Vec<Op> {
    // Every op is atomic, so every state a reader can see is one of the
    // states after an op.
    vec![
        Op::Rename("dist/z/sub", "dist/a/sub"),
        Op::Replace("dist/a/x", b"X1".to_vec()),
        Op::Chmod("dist/a/run.sh", 0o755),
        Op::Retarget("dist/a/link", "run.sh"),
        Op::Replace("dist/m/big", vec![b'B'; 64 * 1024]),
        Op::Rename("dist/a/sub", "dist/z/sub"),
        Op::Replace("dist/a/x", b"x0".to_vec()),
        Op::Chmod("dist/a/run.sh", 0o644),
        Op::Retarget("dist/a/link", "x"),
        Op::Replace("dist/m/big", vec![b'A'; 64 * 1024]),
    ]
}

#[test]
#[ignore = "timing-dependent measurement; run with --ignored --nocapture"]
fn continuous_writer() {
    let truth = existed(&[], &cycle());
    println!("| strategy | captures | true | false | blocked |\n|---|---|---|---|---|");
    for (name, strategy) in [
        ("plain", PLAIN),
        ("A", A),
        ("B", B),
        ("W", W),
        ("A+B+W", ALL),
    ] {
        let root = scratch("writer");
        populate(&root, &[]);
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (root, stop) = (root.clone(), stop.clone());
            std::thread::spawn(move || {
                let ops = cycle();
                let mut i = 0;
                while !stop.load(Ordering::Relaxed) {
                    apply(&root, &ops[i % ops.len()]);
                    i += 1;
                }
                // Finish the cycle so the directory can be removed cleanly.
                while i % ops.len() != 0 {
                    apply(&root, &ops[i % ops.len()]);
                    i += 1;
                }
            })
        };
        let (mut t, mut f, mut b) = (0, 0, 0);
        let captures = 300;
        for _ in 0..captures {
            let registry = Registry::new();
            let store = ObjectStore::new();
            registry.register(WatchObserver::new("watch", &root), Trust::Attesting);
            registry.register(RawFsObserver::new("fs", &root), Trust::Attesting);
            registry.register(store.clone(), Trust::ClaimsOnly);
            registry.add_verifier(FsSnapshot::with(store, strategy));
            match snapshot(&registry) {
                Ok(r) if truth.contains(&r) => t += 1,
                Ok(_) => f += 1,
                Err(_) => b += 1,
            }
        }
        stop.store(true, Ordering::Relaxed);
        writer.join().unwrap();
        let _ = fs::remove_dir_all(&root);
        println!("| {name} | {captures} | {t} | {f} | {b} |");
    }
}

#[test]
#[ignore = "timing-dependent measurement; run with --ignored --nocapture"]
fn observer_correspondence_race() {
    // dist/z flips between a directory and a symlink to outside/ (which
    // holds `secret`). Does a listing of dist/z ever show `secret`?
    let root = scratch("swap");
    populate(&root, &[]);
    fs::write(root.join("outside/secret"), "s").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flips = Arc::new(AtomicUsize::new(0));
    let flipper = {
        let (root, stop, flips) = (root.clone(), stop.clone(), flips.clone());
        std::thread::spawn(move || {
            let (z, aside) = (root.join("dist/z"), root.join("dist/z.real"));
            while !stop.load(Ordering::Relaxed) {
                fs::rename(&z, &aside).unwrap();
                std::os::unix::fs::symlink("../outside", &z).unwrap();
                fs::remove_file(&z).unwrap();
                fs::rename(&aside, &z).unwrap();
                flips.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    let observer = RawFsObserver::new("fs", &root);
    let registry = Registry::new();
    registry.register(RawFsObserver::new("fs", &root), Trust::Attesting);
    let key = Key::new("fs_dir", ["dist/z"]);
    let (mut escapes, mut listed, attempts) = (0, 0, 200_000);
    for _ in 0..attempts {
        if let Some(Term::Map(listing)) = registry.query(&key) {
            listed += 1;
            if listing.contains_key("secret") {
                escapes += 1;
            }
        }
    }
    let _ = observer;
    stop.store(true, Ordering::Relaxed);
    flipper.join().unwrap();
    let _ = fs::remove_dir_all(&root);
    println!(
        "attempts {attempts}, listings {listed}, escapes outside the root {escapes}, flips {}",
        flips.load(Ordering::Relaxed)
    );
}
