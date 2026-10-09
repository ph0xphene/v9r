# Ten-minute verification boundary demo

This deterministic example needs Rust stable and `cp` on Linux/macOS. No model, credentials, or network service is required (Cargo may download dependencies).

```bash
cargo run --manifest-path examples/task-runtime/Cargo.toml
```

The program creates a disposable directory under `TMPDIR` and demonstrates:

1. Snapshot `state.txt` containing `clean`.
2. Run the allowed `cp dirty.txt state.txt` command; state becomes `dirty`.
3. Submit a command with a declared write to `../escape.txt`; the runtime rejects parent traversal before spawning it.
4. Restore the checkpoint; `state.txt` is `clean` and `escape.txt` does not exist.

Observed output (UUIDs and absolute trace paths vary):

```text
after allowed write: Success
state: dirty
bad write result: Err(Violation("declared write path parent-dir traversal: ../escape.txt"))
status after violation: Violation
after rollback: clean
escape exists: false
```

The final XML snapshot includes manifest, filesystem state, and trace events. This proves a specific structural rejection and rollback path, **not OS-level containment of arbitrary subprocesses**. Declared read/write paths are not independent observations of all effects a process can cause.

The example overwrites only its dedicated `v9r-task-runtime-demo` directory under `TMPDIR`. Do not store other files there. Run it serially.
