//! Capability manifest v0, live: construct a minimal world from a
//! manifest, place canaries outside it, observe from inside and outside,
//! and check "observed capabilities ⊆ declared capabilities".
//!
//! World A is the claim: it must be allowed, its declared capabilities
//! visible, every canary invisible. World B is the negative control: an
//! outside listener's socket is smuggled into a declared directory, so the
//! checker must deny. World B also allows descendants, the positive control
//! for the process dimension.
//!
//! Needs unprivileged user namespaces, `python3` and `nix-store` (the
//! interpreter's closure is the world's toolchain).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;

use v9r_core::capability::{
    self, CapSubject, CapValue, CapabilityManifest, Closing, Observation, Target, DESCENDANTS,
};
use v9r_core::kernel::{name_within, Name, Status, Verdict};

/// A nix store path's runtime closure.
fn closure(path: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let store_path: PathBuf = path.components().take(4).collect();
    if !store_path.starts_with("/nix/store") {
        return Err(format!("{path:?} is not in /nix/store"));
    }
    let out = Command::new("nix-store")
        .arg("-qR")
        .arg(&store_path)
        .output()
        .map_err(|e| format!("nix-store: {e}"))?;
    let closure = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if !out.status.success() || closure.is_empty() {
        return Err("nix-store -qR failed".into());
    }
    Ok(closure)
}

/// The `python3` on PATH (possibly a wrapper), and the interpreter it
/// really runs (`/proc/self/exe`), each with its closure, or why not.
fn toolchain() -> Result<[(PathBuf, Vec<PathBuf>); 2], String> {
    let out = Command::new("sh")
        .args(["-c", "command -v python3"])
        .output()
        .map_err(|e| e.to_string())?;
    let launcher = fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim())
        .map_err(|e| format!("python3: {e}"))?;
    let out = Command::new(&launcher)
        .args(["-I", "-c", "import os; print(os.readlink('/proc/self/exe'))"])
        .output()
        .map_err(|e| format!("python3: {e}"))?;
    let real = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    Ok([
        (launcher.clone(), closure(&launcher)?),
        (real.clone(), closure(&real)?),
    ])
}

fn decision_of(
    d: &v9r_core::kernel::Decision<CapSubject, CapValue>,
    invariant: &str,
    subject: Option<&CapSubject>,
) -> Vec<Status> {
    d.findings
        .iter()
        .filter(|f| f.obligation.invariant == invariant)
        .filter(|f| match (subject, &f.obligation.requirement) {
            (None, _) => true,
            (Some(s), v9r_core::kernel::Requirement::Fact { subject, .. }) => subject == s,
            _ => false,
        })
        .map(|f| f.status.clone())
        .collect()
}

fn report(label: &str, plan: &capability::ConstructionPlan, obs: &Observation) {
    let (entries, unknown) = obs.walk_len();
    eprintln!("== {label}");
    eprintln!("plan {}:", plan.digest());
    for (i, step) in plan.steps().iter().enumerate() {
        eprintln!("  {i:2} {step}");
    }
    eprintln!("probe sha256 {}", obs.probe_digest());
    eprintln!("walk: {entries} entries, {unknown} unknown");
    eprintln!("outside: {:?}", obs.outside());
    let json: serde_json::Value = serde_json::from_str(obs.inside_json()).unwrap();
    for key in ["identity", "env", "process", "network", "targets"] {
        eprintln!("inside {key}: {}", json[key]);
    }
    let known: Vec<String> = obs
        .names()
        .into_iter()
        .filter_map(|n| match n {
            Name::Known(n) if !n.starts_with("fs/") => Some(n),
            Name::Known(_) => None,
            Name::UnknownBelow(n) => Some(format!("unknown below {n}")),
        })
        .collect();
    eprintln!("non-fs names: {known:?}");
}

#[test]
fn constructed_world_holds_only_declared_capabilities() {
    let [(launcher, launcher_closure), (python, closure)] = match toolchain() {
        Ok(t) => t,
        Err(why) => {
            eprintln!("SKIPPED (environment, not evidence): {why}");
            return;
        }
    };
    let base = fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("v9r-capmanifest-{}", uuid::Uuid::new_v4()));
    for d in ["root-a", "root-b", "work", "shared", "outside", "smuggle"] {
        fs::create_dir_all(base.join(d)).unwrap();
    }
    let p = |s: &str| base.join(s);
    fs::write(p("work/inside-canary.txt"), "inside").unwrap();
    fs::write(p("shared/data.txt"), "read only").unwrap();
    fs::write(p("outside/canary-secret.txt"), "outside").unwrap();

    // Canaries outside the world, each with a live listener.
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let abstract_name = format!("v9r-canary-{tag}");
    let abstract_l =
        UnixListener::bind_addr(&SocketAddr::from_abstract_name(&abstract_name).unwrap()).unwrap();
    let tmp_sock = PathBuf::from(format!("/tmp/v9r-canary-{tag}.sock"));
    let tmp_l = UnixListener::bind(&tmp_sock).unwrap();
    let out_sock = p("outside/spawner.sock");
    let out_l = UnixListener::bind(&out_sock).unwrap();
    let tcp_l = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp_port = tcp_l.local_addr().unwrap().port();
    // World B's smuggled channel: a live outside listener inside a
    // declared directory.
    let smuggled = p("smuggle/agent.sock");
    let smuggled_l = UnixListener::bind(&smuggled).unwrap();

    // Positive control for the canaries: every one is reachable from the
    // runtime's own (ambient) world.
    UnixStream::connect_addr(&SocketAddr::from_abstract_name(&abstract_name).unwrap()).unwrap();
    UnixStream::connect(&tmp_sock).unwrap();
    UnixStream::connect(&out_sock).unwrap();
    UnixStream::connect(&smuggled).unwrap();
    TcpStream::connect(("127.0.0.1", tcp_port)).unwrap();
    for l in [&abstract_l, &tmp_l, &out_l, &smuggled_l] {
        l.accept().unwrap();
    }
    tcp_l.accept().unwrap();

    // An inherited-fd canary: open, without close-on-exec, in the runtime.
    let secret = std::ffi::CString::new(p("outside/canary-secret.txt").to_str().unwrap()).unwrap();
    // SAFETY: plain open(2) on a valid C string.
    let canary_fd = unsafe { libc::open(secret.as_ptr(), libc::O_RDONLY) };
    assert!(canary_fd > 2, "canary fd");

    let targets = BTreeMap::from([
        (
            "abstract".to_string(),
            Target::Abstract {
                addr: abstract_name.clone(),
            },
        ),
        (
            "tmp_socket".to_string(),
            Target::Path {
                addr: tmp_sock.to_str().unwrap().into(),
            },
        ),
        (
            "outside_socket".to_string(),
            Target::Path {
                addr: out_sock.to_str().unwrap().into(),
            },
        ),
        (
            "tcp_localhost".to_string(),
            Target::Tcp {
                host: "127.0.0.1".into(),
                port: tcp_port,
            },
        ),
    ]);

    let env = BTreeMap::from([
        ("LC_ALL".to_string(), "C".to_string()),
        ("V9R_WORLD".to_string(), "a".to_string()),
    ]);
    let mut read_paths = closure.clone();
    read_paths.push(p("shared"));

    // ---- World A: the claim.
    let manifest_a = CapabilityManifest {
        read_paths: read_paths.clone(),
        write_paths: vec![p("work")],
        descendants_allowed: false,
        env: env.clone(),
        aliases: vec![],
    };
    let plan_a = capability::plan(&manifest_a).expect("plan A");
    let obs_a = capability::observe(&plan_a, &p("root-a"), &python, &targets).expect("world A");
    report("world A", &plan_a, &obs_a);
    let decision_a = capability::check(&plan_a, &obs_a);
    eprintln!("{decision_a}");

    // Declared capabilities are visible, as the declared objects.
    assert_eq!(obs_a.walked(p("work/inside-canary.txt").to_str().unwrap()), Some(("f", true, true)));
    assert_eq!(obs_a.walked(p("shared/data.txt").to_str().unwrap()), Some(("f", true, false)));
    assert_eq!(obs_a.walked(python.to_str().unwrap()).map(|e| e.1), Some(true));
    for status in decision_of(&decision_a, "C2.declared_visible", None) {
        assert!(matches!(status, Status::Satisfied(_)), "{status:?}");
    }

    // Undeclared capabilities are not granted.
    assert_eq!(obs_a.walked(p("outside").to_str().unwrap()), None);
    assert_eq!(obs_a.walked(p("outside/canary-secret.txt").to_str().unwrap()), None);
    assert_eq!(obs_a.walked(tmp_sock.to_str().unwrap()), None);
    assert_eq!(obs_a.walked(p("smuggle").to_str().unwrap()), None);
    assert_eq!(obs_a.walked("/proc"), None);
    assert_eq!(obs_a.walked("/dev"), None);
    for key in targets.keys() {
        let answer = obs_a.target(key).unwrap();
        assert_ne!(answer, "connected", "canary {key} reachable from inside");
    }
    let names = obs_a.names();
    assert!(!names.contains(&Name::Known(DESCENDANTS.into())));
    assert!(!names.iter().any(|n| matches!(n, Name::Known(n) if n.starts_with("env/HOME"))));
    let fds = obs_a.outside().fds.as_ref().expect("fd table read from outside");
    assert!(
        fds.iter().all(|(fd, _)| *fd <= 2),
        "inherited fd canary {canary_fd} reached the world: {fds:?}"
    );

    // The world is closed.
    for status in decision_of(&decision_a, "C3.closed", None) {
        assert!(matches!(status, Status::Satisfied(_)), "{status:?}");
    }
    // The invariant.
    assert!(matches!(
        decision_of(&decision_a, "C1.observed_within_declared", None)[0],
        Status::Satisfied(_)
    ));
    assert_eq!(decision_a.verdict, Verdict::Allow, "{decision_a}");

    // ---- World W: the same manifest, observed through a launcher that
    // is a wrapper (nix `makeCWrapper` sets env, then execs). The world
    // is the same construction, but the first program changes its own
    // state before the probe reads it. C1 must catch that, and only that.
    if launcher != python {
        let mut read_paths_w = launcher_closure.clone();
        read_paths_w.push(p("shared"));
        let manifest_w = CapabilityManifest {
            read_paths: read_paths_w,
            ..manifest_a.clone()
        };
        let plan_w = capability::plan(&manifest_w).expect("plan W");
        fs::create_dir_all(p("root-w")).unwrap();
        let obs_w =
            capability::observe(&plan_w, &p("root-w"), &launcher, &targets).expect("world W");
        report("world W (wrapped launcher)", &plan_w, &obs_w);
        let decision_w = capability::check(&plan_w, &obs_w);
        let scopes = plan_w.scopes();
        let outside: Vec<String> = obs_w
            .names()
            .into_iter()
            .filter_map(|n| match n {
                Name::Known(n) if !scopes.iter().any(|s| name_within(&n, s)) => Some(n),
                _ => None,
            })
            .collect();
        eprintln!("world W outside the manifest: {outside:?}");
        eprintln!("world W verdict: {:?}", decision_w.verdict);
        assert!(!outside.is_empty() && outside.iter().all(|n| n.starts_with("env/")));
        assert!(matches!(
            decision_of(&decision_w, "C1.observed_within_declared", None)[0],
            Status::Violated(_)
        ));
        assert_eq!(decision_w.verdict, Verdict::Deny);
    }

    // ---- World B: negative control (smuggled socket) and descendants.
    let manifest_b = CapabilityManifest {
        read_paths,
        write_paths: vec![p("smuggle")],
        descendants_allowed: true,
        env,
        aliases: vec![],
    };
    let plan_b = capability::plan(&manifest_b).expect("plan B");
    let obs_b = capability::observe(&plan_b, &p("root-b"), &python, &targets).expect("world B");
    report("world B", &plan_b, &obs_b);
    let decision_b = capability::check(&plan_b, &obs_b);
    eprintln!("{decision_b}");

    assert_eq!(obs_b.walked(smuggled.to_str().unwrap()).map(|e| e.0), Some("s"));
    match &decision_of(&decision_b, "C1.observed_within_declared", None)[0] {
        Status::Violated(why) => assert!(why.contains("special"), "{why}"),
        other => panic!("smuggled socket not caught: {other:?}"),
    }
    assert!(matches!(
        decision_of(
            &decision_b,
            "C2.declared_visible",
            Some(&CapSubject::Declared(DESCENDANTS.into()))
        )[0],
        Status::Satisfied(_)
    ));
    assert!(matches!(
        decision_of(
            &decision_b,
            "C3.closed",
            Some(&CapSubject::Closing(Closing::Namespace("pid".into())))
        )[0],
        Status::Satisfied(_)
    ));
    assert_eq!(decision_b.verdict, Verdict::Deny, "{decision_b}");

    // The smuggled channel is a real one: from inside it would reach the
    // outside listener. Show the listener is still live.
    let mut c = UnixStream::connect(&smuggled).unwrap();
    let (mut s, _) = smuggled_l.accept().unwrap();
    c.write_all(b"x").unwrap();
    let mut buf = [0u8; 1];
    s.read_exact(&mut buf).unwrap();

    // SAFETY: closing the fd this test opened.
    unsafe { libc::close(canary_fd) };
    drop((abstract_l, tmp_l, out_l, tcp_l, smuggled_l));
    let _ = fs::remove_file(&tmp_sock);
    fs::remove_dir_all(&base).unwrap();
}
