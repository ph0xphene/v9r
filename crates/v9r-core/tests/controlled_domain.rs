//! Controlled execution domain: can the runtime own every writer of the
//! state it captures?
//!
//! The runtime starts an agent in a domain, lets it run an effect, waits
//! for the agent's top process to exit, freezes the domain, captures a
//! content-addressed snapshot, and destroys the domain. The agent leaves
//! behind a writer by one ordinary process behaviour (fork, daemon,
//! setsid, double fork, delayed start, or moving itself to another
//! cgroup). The writer replaces `dist/w` atomically every millisecond.
//!
//! A row is only reported if the writer is proven to have run: before
//! the freeze, two captures 50 ms apart must differ (**live**). A row
//! that fails this is `INVALID` and prints the writer's stderr. The
//! `none` level runs the agent without any domain and is the positive
//! control: its writers must survive, or the detector is blind.
//!
//! Observations per run, the first two with the plain snapshot verifier:
//!
//! - **quiescent**: two captures 200 ms apart while the domain is frozen
//!   have the same root;
//! - **contained**: after destruction, two captures 300 ms apart have the
//!   same root (no writer outlived the domain);
//! - **survivors**: ground truth from the host's `/proc`, independent of
//!   snapshots: writer processes still alive after destruction, and
//!   whether each was inside the domain's cgroup subtree at freeze.
//!
//! Domain levels (all unprivileged):
//!
//! - `none`: no domain (positive control);
//! - `scope`: a delegated systemd user scope (cgroup v2 only);
//! - `scope+userns+cgroupns`: the agent runs in a new user and cgroup
//!   namespace rooted at that scope;
//! - `scope+userns+cgroupns+pidns`: also a new PID namespace (with its own
//!   `/proc`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use uuid::Uuid;
use v9r_core::fs_raw::RawFsObserver;
use v9r_core::graph::{Key, Registry, Term, Trust};
use v9r_core::snapshot::{FsSnapshot, ObjectStore};

const WRITER: &str = r#"import os, sys, time
ws, target = sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else ""
open(os.path.join(ws, "outside/started.%d" % os.getpid()), "w").close()
if target:
    try:
        os.makedirs(target, exist_ok=True)
        with open(os.path.join(target, "cgroup.procs"), "w") as f:
            f.write(str(os.getpid()))
        open(os.path.join(ws, "outside/moved"), "w").write("ok")
    except OSError as e:
        open(os.path.join(ws, "outside/moved"), "w").write(repr(e))
i = 0
while True:
    i += 1
    with open(os.path.join(ws, "dist/.w"), "w") as f:
        f.write(str(i))
    os.rename(os.path.join(ws, "dist/.w"), os.path.join(ws, "dist/w"))
    time.sleep(0.001)
"#;

/// A host daemon outside every domain that runs whatever a client sends
/// it: the stand-in for sshd, a CI runner, a container daemon. It listens
/// on an abstract unix socket, a unix socket in `/tmp`, and TCP localhost.
const SPAWNER: &str = r#"import os, socket, subprocess, sys, threading
ws, id = sys.argv[1], sys.argv[2]
def serve(s):
    s.listen()
    while True:
        c, _ = s.accept()
        cmd = c.makefile().readline().strip()
        c.close()
        subprocess.Popen(cmd, shell=True, start_new_session=True,
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
a = socket.socket(socket.AF_UNIX)
a.bind("\0v9r-spawn-" + id)
p = socket.socket(socket.AF_UNIX)
p.bind("/tmp/v9r-spawn-%s.sock" % id)
t = socket.socket()
t.bind(("127.0.0.1", 0))
threading.Thread(target=serve, args=(a,), daemon=True).start()
threading.Thread(target=serve, args=(p,), daemon=True).start()
with open(os.path.join(ws, "outside/.port"), "w") as f:
    f.write(str(t.getsockname()[1]))
os.rename(os.path.join(ws, "outside/.port"), os.path.join(ws, "outside/port"))
serve(t)
"#;

/// Each behaviour: the agent's shell code. `$W` starts a writer; its
/// stderr goes to `outside/writer.err` unless the behaviour redirects it.
/// The last five ask a process outside the domain to start the writer.
const BEHAVIOURS: &[(&str, &str)] = &[
    ("fork child", "$W 2>>\"$ERR\" &"),
    ("background daemon", "nohup $W </dev/null >/dev/null 2>>\"$ERR\" &"),
    ("setsid", "setsid $W </dev/null >/dev/null 2>>\"$ERR\" &"),
    ("double fork", "( $W </dev/null >/dev/null 2>>\"$ERR\" & ) ; true"),
    (
        "delayed writer after parent exits",
        "( sleep 0.5; exec $W ) </dev/null >/dev/null 2>>\"$ERR\" &",
    ),
    (
        "move to another cgroup",
        "$W \"$SIBLING\" </dev/null >/dev/null 2>>\"$ERR\" &",
    ),
    (
        "service manager (systemd-run)",
        "systemd-run --user --quiet --collect --unit=v9r-esc-$ID-run $W 2>>\"$ERR\"",
    ),
    (
        "service manager (raw D-Bus)",
        "PY=$(command -v python3)\n\
         busctl --user call org.freedesktop.systemd1 /org/freedesktop/systemd1 \
         org.freedesktop.systemd1.Manager StartTransientUnit 'ssa(sv)a(sa(sv))' \
         v9r-esc-$ID-bus.service fail 2 ExecStart 'a(sasb)' 1 \"$PY\" 3 \"$PY\" \
         \"$WS/outside/writer.py\" \"$WS\" false CollectMode s inactive-or-failed 0 \
         >/dev/null 2>>\"$ERR\"",
    ),
    (
        "host daemon (abstract unix socket)",
        "python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); \
         s.connect(\"\\0v9r-spawn-\"+sys.argv[2]); s.sendall(sys.argv[1].encode()+b\"\\n\")' \
         \"$W\" \"$ID\" 2>>\"$ERR\"",
    ),
    (
        "host daemon (unix socket in /tmp)",
        "python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); \
         s.connect(\"/tmp/v9r-spawn-%s.sock\" % sys.argv[2]); s.sendall(sys.argv[1].encode()+b\"\\n\")' \
         \"$W\" \"$ID\" 2>>\"$ERR\"",
    ),
    (
        "host daemon (TCP localhost)",
        "python3 -c 'import socket,sys; s=socket.create_connection((\"127.0.0.1\", int(sys.argv[2]))); \
         s.sendall(sys.argv[1].encode()+b\"\\n\")' \"$W\" \"$(cat $WS/outside/port)\" 2>>\"$ERR\"",
    ),
];

const UNSHARE_CG: &[&str] = &["unshare", "--user", "--map-root-user", "--cgroup"];
const UNSHARE_PID: &[&str] = &[
    "unshare",
    "--user",
    "--map-root-user",
    "--cgroup",
    "--pid",
    "--fork",
    "--mount-proc",
];
const UNSHARE_NET: &[&str] = &[
    "unshare",
    "--user",
    "--map-root-user",
    "--cgroup",
    "--pid",
    "--fork",
    "--mount-proc",
    "--net",
];
const UNSHARE_NO_PID: &[&str] = &[
    "unshare",
    "--user",
    "--map-root-user",
    "--cgroup",
    "--mount",
    "--net",
];

/// Runtime-held init that only waits.
const HOLD: &str = "sh \"$0\"; exec sleep 3600";
/// Runtime-held init that first hides the user's runtime directory (the
/// service manager's and session bus's sockets) in the domain's mounts.
const HOLD_HIDE_RUN: &str = "mount -t tmpfs tmpfs \"$XDG_RUNTIME_DIR\" && sh \"$0\"; exec sleep 3600";
/// Also gives the domain a private `/tmp` holding only its workspace,
/// which lives in the host's `/tmp`.
const HOLD_HIDE_RUN_TMP: &str = "WS=$(dirname \"$(dirname \"$0\")\") \
    && mount -t tmpfs tmpfs \"$XDG_RUNTIME_DIR\" \
    && mkdir \"$XDG_RUNTIME_DIR/ws\" && mount --bind \"$WS\" \"$XDG_RUNTIME_DIR/ws\" \
    && mount -t tmpfs tmpfs /tmp && mkdir -p \"$WS\" \
    && mount --bind \"$XDG_RUNTIME_DIR/ws\" \"$WS\" && umount \"$XDG_RUNTIME_DIR/ws\" \
    && sh \"$0\"; exec sleep 3600";

/// (name, uses a scope, command prefix, runtime-held init)
///
/// When the agent's shell is PID 1 of its namespace, its exit kills every
/// process left in the namespace, including writers the effect wanted.
/// A runtime-held init keeps PID 1 as a runtime process for the whole
/// effect window, so the agent's top process exits like at the other
/// levels.
const LEVELS: &[(&str, bool, &[&str], Option<&str>)] = &[
    ("none", false, &[], None),
    ("scope", true, &[], None),
    ("scope+userns+cgroupns", true, UNSHARE_CG, None),
    ("+pidns (agent is init)", true, UNSHARE_PID, None),
    ("+pidns (runtime holds init)", true, UNSHARE_PID, Some(HOLD)),
    ("+pidns held +mountns hiding runtime dir", true, UNSHARE_PID, Some(HOLD_HIDE_RUN)),
    ("+pidns held +mountns hiding runtime dir +netns", true, UNSHARE_NET, Some(HOLD_HIDE_RUN)),
    ("+pidns held +mountns hiding runtime dir, private /tmp +netns", true, UNSHARE_NET, Some(HOLD_HIDE_RUN_TMP)),
    ("scope+userns+cgroupns+mountns hiding runtime dir, private /tmp +netns (no pidns)", true, UNSHARE_NO_PID, Some(HOLD_HIDE_RUN_TMP)),
];

fn snapshot(ws: &Path) -> Option<String> {
    let registry = Registry::new();
    let store = ObjectStore::new();
    registry.register(RawFsObserver::new("fs", ws), Trust::Attesting);
    registry.register(store.clone(), Trust::ClaimsOnly);
    registry.add_verifier(FsSnapshot::new(store));
    match registry.query(&Key::new("snapshot", ["dist"])) {
        Some(Term::Id(root)) => Some(root),
        _ => None,
    }
}

/// Two captures `gap` apart agree.
fn stable(ws: &Path, gap: u64) -> bool {
    let a = snapshot(ws);
    std::thread::sleep(Duration::from_millis(gap));
    a.is_some() && a == snapshot(ws)
}

fn cgroup_of(unit: &str) -> Option<PathBuf> {
    let out = Command::new("systemctl")
        .args(["--user", "show", "-P", "ControlGroup", unit])
        .output()
        .ok()?;
    let cg = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!cg.is_empty()).then(|| PathBuf::from(format!("/sys/fs/cgroup{cg}")))
}

fn events(cg: &Path) -> String {
    fs::read_to_string(cg.join("cgroup.events")).unwrap_or_default()
}

/// Freeze and wait until the kernel reports the whole subtree frozen.
/// `false` if there is no cgroup left to freeze.
fn freeze(cg: &Path, on: bool) -> bool {
    if fs::write(cg.join("cgroup.freeze"), if on { "1" } else { "0" }).is_err() {
        return false;
    }
    let want = if on { "frozen 1" } else { "frozen 0" };
    for _ in 0..5000 {
        if events(cg).contains(want) {
            return true;
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    panic!("{} did not reach {want}", cg.display());
}

/// Kill every process in the cgroup subtree and wait until it is empty.
fn destroy(cg: &Path) {
    let _ = fs::write(cg.join("cgroup.freeze"), "0");
    if fs::write(cg.join("cgroup.kill"), "1").is_err() {
        return;
    }
    for _ in 0..500 {
        if !cg.exists() || events(cg).contains("populated 0") {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Host-side ground truth: live writer processes for this workspace, as
/// (host pid, cgroup v2 path). Zombies and shells that merely carry the
/// command line are not writers.
fn writers(ws: &Path) -> Vec<(u32, String)> {
    let needle = format!("{}/outside/writer.py", ws.display());
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let dir = entry.path();
        let Ok(cmdline) = fs::read(dir.join("cmdline")) else {
            continue;
        };
        if !String::from_utf8_lossy(&cmdline).contains(&needle) {
            continue;
        }
        let comm = fs::read_to_string(dir.join("comm")).unwrap_or_default();
        if !comm.starts_with("python") {
            continue;
        }
        let stat = fs::read_to_string(dir.join("stat")).unwrap_or_default();
        let state = stat.rsplit(')').next().and_then(|s| s.split_whitespace().next());
        if matches!(state, Some("Z" | "X") | None) {
            continue;
        }
        let cg = fs::read_to_string(dir.join("cgroup")).unwrap_or_default();
        let cg = cg.lines().find_map(|l| l.strip_prefix("0::")).unwrap_or("?");
        found.push((pid, format!("/sys/fs/cgroup{cg}")));
    }
    found
}

/// Wait until no writer for `ws` remains, up to `ms`.
fn survivors_after(ws: &Path, ms: u64) -> Vec<(u32, String)> {
    for _ in 0..ms / 5 {
        let w = writers(ws);
        if w.is_empty() {
            return w;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    writers(ws)
}

fn reap(ws: &Path) {
    for _ in 0..100 {
        let w = writers(ws);
        if w.is_empty() {
            return;
        }
        for (pid, _) in w {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("could not reap writers for {}", ws.display());
}

struct Row {
    level: &'static str,
    behaviour: &'static str,
    /// Writers that reached their first line.
    started: usize,
    /// A writer was observably writing at the end of the effect window.
    live: bool,
    domain: String,
    in_domain: usize,
    outside: usize,
    quiescent: Option<bool>,
    survivors: usize,
    contained: bool,
    moved: String,
    /// Last line of the writer's (or its requester's) stderr.
    reason: String,
    diag: String,
}

fn run(
    level: &'static str,
    scoped: bool,
    prefix: &[&str],
    init: Option<&str>,
    behaviour: &'static str,
    code: &str,
) -> Row {
    let id = Uuid::new_v4().simple().to_string();
    let ws = std::env::temp_dir().join(format!("v9r-domain-{id}"));
    fs::create_dir_all(ws.join("dist")).unwrap();
    fs::create_dir_all(ws.join("outside")).unwrap();
    fs::write(ws.join("dist/w"), "0").unwrap();
    fs::write(ws.join("outside/writer.py"), WRITER).unwrap();
    let unit = format!("v9r-domain-{id}.scope");
    // The agent learns the host path of a sibling cgroup: generous to the
    // agent, which under a cgroup namespace cannot see it.
    let sibling = cgroup_of("app.slice")
        .map(|p| p.join(format!("v9r-escape-{id}")))
        .expect("no app.slice cgroup");
    let err = ws.join("outside/writer.err");
    fs::write(
        ws.join("outside/agent.sh"),
        format!(
            "ID={id}\nWS={ws}\nW=\"python3 {ws}/outside/writer.py {ws}\"\nERR={err}\nSIBLING={sib}\n{code}\n",
            ws = ws.display(),
            err = err.display(),
            sib = sibling.display()
        ),
    )
    .unwrap();

    // The host daemon, outside every domain, ready before the agent runs.
    fs::write(ws.join("outside/spawner.py"), SPAWNER).unwrap();
    let mut spawner = Command::new("python3")
        .arg(ws.join("outside/spawner.py"))
        .arg(&ws)
        .arg(&id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cannot start spawner");
    for _ in 0..1000 {
        if ws.join("outside/port").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(ws.join("outside/port").exists(), "spawner did not start");

    // Start the agent. Its top process exits at once; unless the runtime
    // holds the namespace init, that is also our child's exit.
    let mut cmd = if scoped {
        let mut c = Command::new("systemd-run");
        c.args(["--user", "--scope", "--quiet", "-p", "Delegate=yes"])
            .arg(format!("--unit={unit}"))
            .args(prefix)
            .arg("sh");
        c
    } else {
        assert!(prefix.is_empty());
        Command::new("sh")
    };
    if let Some(init) = init {
        cmd.args(["-c", init]);
    }
    let mut child = cmd
        .arg(ws.join("outside/agent.sh"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(ws.join("outside/agent.err")).unwrap())
        .spawn()
        .expect("cannot start agent");
    let status = if init.is_some() { None } else { child.wait().ok() };

    // The effect window: the delayed writer starts in it.
    std::thread::sleep(Duration::from_millis(1000));

    // Liveness gate: the writer must be observably writing right now.
    let live = !stable(&ws, 50);

    let cg = if scoped { cgroup_of(&unit).filter(|p| p.exists()) } else { None };
    let froze = cg.as_deref().is_some_and(|c| freeze(c, true));
    let present = writers(&ws);
    let in_domain = cg.as_ref().map_or(0, |c| {
        let root = c.display().to_string();
        present
            .iter()
            .filter(|(_, g)| *g == root || g.starts_with(&format!("{root}/")))
            .count()
    });
    let quiescent = froze.then(|| stable(&ws, 200));
    if let Some(c) = &cg {
        destroy(c);
    }
    let survivors = survivors_after(&ws, 200).len();
    if init.is_some() {
        // Destruction killed the held init and with it our child.
        let _ = child.kill();
        let _ = child.wait();
    }
    let contained = stable(&ws, 300);

    let started = fs::read_dir(ws.join("outside"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("started."))
        .count();
    let moved = fs::read_to_string(ws.join("outside/moved")).unwrap_or_else(|_| "-".into());
    let reason = fs::read_to_string(&err)
        .unwrap_or_default()
        .lines()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    let diag = format!(
        "agent exit {:?}; agent.err: {:?}; writer.err: {:?}",
        status.and_then(|s| s.code()),
        fs::read_to_string(ws.join("outside/agent.err")).unwrap_or_default().trim(),
        fs::read_to_string(&err).unwrap_or_default().trim(),
    );

    // Reap anything that escaped, then clean up.
    let _ = spawner.kill();
    let _ = spawner.wait();
    let _ = fs::remove_file(format!("/tmp/v9r-spawn-{id}.sock"));
    reap(&ws);
    for suffix in ["run.service", "bus.service"] {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("v9r-esc-{id}-{suffix}")])
            .stderr(Stdio::null())
            .status();
    }
    if sibling.exists() {
        for _ in 0..200 {
            if fs::remove_dir(&sibling).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = Command::new("systemctl")
        .args(["--user", "stop", &unit])
        .stderr(Stdio::null())
        .status();
    let _ = fs::remove_dir_all(&ws);

    Row {
        level,
        behaviour,
        started,
        live,
        domain: match (scoped, &cg) {
            (false, _) => "-".into(),
            (true, None) => "gone".into(),
            (true, Some(_)) => "frozen".into(),
        },
        in_domain,
        outside: present.len() - in_domain,
        quiescent,
        survivors,
        contained,
        moved: moved.trim().to_string(),
        reason,
        diag,
    }
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "**NO**" }
}

#[test]
#[ignore = "needs a systemd user session with delegation and unprivileged user namespaces; run with --ignored --nocapture"]
fn controlled_domain_escapes() {
    if Command::new("systemd-run").arg("--version").output().is_err() {
        println!("systemd-run unavailable");
        return;
    }
    println!(
        "| level | behaviour | writer | domain at freeze | writers in / out of domain | quiescent | survivors after destroy | contained | migration / why not started |\n|---|---|---|---|---|---|---|---|---|"
    );
    let mut invalid = 0;
    let mut control_ok = true;
    // Behaviours whose writer ran without a domain: the agent code works,
    // so a writer missing under a domain was stopped by the domain.
    let mut proven: Vec<&str> = Vec::new();
    for (level, scoped, prefix, init) in LEVELS {
        for (behaviour, code) in BEHAVIOURS {
            let r = run(level, *scoped, prefix, *init, behaviour, code);
            let writer = match (r.live, r.started) {
                (true, _) => "live",
                (false, n) if n > 0 => "killed before freeze",
                (false, _) if proven.contains(behaviour) => "never started",
                (false, _) => {
                    invalid += 1;
                    println!("| {} | {} | INVALID | | | | | | | <!-- {} -->", r.level, r.behaviour, r.diag);
                    continue;
                }
            };
            if !*scoped {
                if r.live {
                    proven.push(behaviour);
                }
                if r.survivors == 0 {
                    control_ok = false;
                }
            }
            // Snapshot verdict and /proc ground truth must agree.
            assert_eq!(
                r.contained,
                r.survivors == 0,
                "{} / {}: snapshot says contained={} but {} writers survive",
                r.level,
                r.behaviour,
                r.contained,
                r.survivors
            );
            println!(
                "| {} | {} | {} | {} | {} / {} | {} | {} | {} | {} |",
                r.level,
                r.behaviour,
                writer,
                r.domain,
                r.in_domain,
                r.outside,
                r.quiescent.map_or("n/a", yn),
                r.survivors,
                yn(r.contained),
                if writer == "never started" { r.reason.as_str() } else { r.moved.as_str() }
            );
        }
    }
    assert_eq!(invalid, 0, "{invalid} rows have an unexplained missing writer; the table is void");
    assert!(control_ok, "positive control: a writer without a domain did not survive");
}
