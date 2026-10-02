# v9r Observer Capability Boundary v0

*A model document. It asks whether the observer independence of
V9R_OBSERVER_INDEPENDENCE_MODEL_V0 reduces to capability and state
checks. Nothing was built or run for it. It proposes no code change and
does not assume execution receipts.*

**Revision:** the code of `v9r-review-v1` (code commit `6a9c717`). The
capability experiments cited here are **archived**. They were measured
on the pre-debloat tree and are reproducible at `v9r-archive-v0`. They
are not part of the v1 claim.

**Evidence labels:**

- **measured:** a result in an existing report, named;
- **code:** a fact about frozen code, read, not run;
- **semantics:** a property of the language or OS the argument relies
  on, not measured here;
- **definition:** introduced by this document.

## 0. Short answer

**No. Independence splits in two, and only one half reduces to
capability and state checks.**

**I-eff, "the observer does not change what is judged",** is an
authority property.

- It has a set form,
  `watched_objects ∩ mutable(observer) = ∅`.
- That form needs three qualifications:
  - it must be evaluated over **object** identities, not names;
  - `mutable` must be closed over **delegated** authority;
  - it must hold for the whole **decision**, not one instant.
- It is establishable only by **constructing** the observer's world
  (isolation), with observation as the check. Declaring it in a manifest
  is not enough.
- The archived experiments measured every piece of that construction,
  and the residue it leaves.

**I-out, "the judged state does not control the observer's output",**
is **not** an authority property.

- X1 violated it with an observer holding exactly the authority any
  test runner must hold: read the checkout, run the interpreter. No
  write, no escape.
- No intersection of authority sets expresses it. It is a property of
  **what the observer computes from S**: whether S is *data* to it, or
  *code*.

**The missing abstraction is the difference between *authority* (what
an observer can touch) and *influence* (what controls its output).**
Capability checks bound authority. Influence is bounded only by what
the observer's code does with S.

**X4 is neither.** Its writer was the actor's leftover process, so it
is actor containment, not observer independence (§1.4).

## 1. Is an observer just another actor with read, write and execution authority?

### 1.1 Mapping

*Definition.* Give an observer O the same authority description as an
actor:

| Authority | For the X1 runner (`O_exec`) | Needed for its job? |
|---|---|---|
| **read** S1 | yes, via a checkout materialized from the object store | yes |
| **write** | its scratch checkout; and, by ambient authority, everything its uid can write, including the live workspace | only the scratch checkout |
| **execute** | `python3`, which imports `src/greet.py` from the checkout | yes: running the test *is* executing S1's code |

### 1.2 Where the mapping holds: I-eff

For the question "can O change what is judged?", O **is** just another
actor. Everything measured about containing actors applies:

- **Process-tree writers** are contained by a delegated cgroup subtree
  with freeze and `cgroup.kill`. **Self-migration** out of it is closed
  by a cgroup namespace (measured: CONTROLLED_DOMAIN_V0, L1–L2).
- **Delegated writers** are closed only by mount and network namespaces
  that hide every spawning service's socket. This covers writers O
  *asks* someone else to start: the service manager, `/tmp` daemons,
  localhost. Cgroups do nothing against it: 5 of 11 behaviours
  (measured: CONTROLLED_DOMAIN_V0, L5–L8).
- **What namespaces alone do not remove** (measured where marked;
  CAPABILITY_INVENTORY_V0 by kernel rule otherwise):
  - supplementary groups;
  - inherited fds;
  - the environment;
  - harness tools.

So an observer's I-eff is exactly an actor-containment problem, with
the observer as the actor.

### 1.3 Where it fails: I-out

For "can S control O's output?", the authority description says
nothing.

- **X1 needed no write authority and no escape** (measured:
  V9R_SELF_REFERENTIAL_OBSERVERS_V0). The runner read its checkout, ran
  `python3`, and reported the exit status. The false `passed` was
  produced by the code path every test runner must take.
- **Read and execute are not separable as authority for interpreted
  code** (semantics): Python's `import` *reads* `greet.py`. The file's
  execute bit is irrelevant. An OS permission that allows reading S's
  bytes allows interpreting them.
- **The same failure, measured in the capability work:** CAPABILITY_MANIFEST_V0,
  world W. A `python3` launcher wrapper set `PYTHONNOUSERSITE` before
  the probe read its environment, and the probe reported it. The report
  concludes: *"every program between `exec` and the observer is part of
  the trusted base"*. The probe's own trust rested on being *"the
  world's only process"*, and: *"If the world ran an agent before the
  probe, none of this would hold."* In X1, S1's code runs **inside** the
  observer's process, before it reports.

**Answer to 1:** for I-eff, yes. An observer is an actor, and its
authority must be contained like an actor's. For I-out, no. An observer
is also a **function** from S to a report, and that function's
dependence on S is not described by any authority it holds.

### 1.4 X4 is the actor's authority, not the observer's

In X4 the writer was a process the actor left behind (measured, 40/40).
The observer's authority played no part: the runner's checkout was a
timing signal. In this model X4 is
`watched_objects ∩ mutable(actor's residual processes) ≠ ∅` during the
decision. That is containment of the **actor** over the decision
window. CONTROLLED_DOMAIN_V0 measured that problem: process-tree
writers, including a "delayed writer after parent exits", were frozen
and killed from L1.

## 2. Is independence `observed_objects ∩ mutable_objects(observer) = ∅`?

### 2.1 What the formula covers

It is a statement of **I-eff only**, and of the most direct part of it:
O cannot write what it observes. G′, where the runner's execution of S1
rewrites the live workspace, is a violation of it, and is **unmeasured**.

### 2.2 What the formula does not cover

- **I-out (X1).** In X1 the observed objects were the runner's
  **materialized checkout**. The judged tree id was fixed before the
  checkout existed. Even with `observed ∩ mutable = ∅` true, the X1
  report would still have been `passed`: the exit status does not
  depend on any write.
- **Writes by others during the decision (X4).** The formula is about
  O's authority. X4's writer was a different principal. The general
  form is `watched ∩ mutable(P) = ∅` for every principal P live during
  the decision.
- **Inbound authority over O's view.** The **owner** of the world's
  user namespace, a same-uid process, changed a sealed read-only
  snapshot view, by remount (A5) and by overmount (A6). This was
  **detected, not prevented** (measured: SNAPSHOT_CAPABILITY_BOUNDARY_V0).
  `mutable(O)` was empty for that view: O could not change it (`EROFS`,
  `EPERM`). Someone else could.

### 2.3 What its terms must mean for it to be checkable

**`observed_objects` must be objects, not names, or content that was
materialized.**

- Names were wrong in 3 of 6 cases; `(st_dev, st_ino)` in 1 of 6
  (inode reuse 500/500 on ext4); `st_dev` + file handle in 0 of 6
  (measured: CAPABILITY_OBJECT_IDENTITY_V0).
- A directory is an object only for itself. A hard link placed inside
  a granted directory passes every identity check (measured: same).
- A live directory verified against a snapshot missed 2 of 6 changes.
  A view materialized from the snapshot was exactly S in 9 of 9
  (measured: CAPABILITY_CONTENT_IDENTITY_V0).

The X1 runner's checkout is of the second kind. For I-eff the
intersection is then over the runner's own copy plus the live
workspace it can reach by ambient authority.

**`mutable(O)` must be enumerated completely, and that is only possible
by construction.**

- *"Ambient authority cannot be fully inferred from observation"*.
  Absence is provable only for complete kernel enumerations, closing
  bits, or *"a view the runtime built from empty"*
  (CAPABILITY_INVENTORY_V0).
- Constructed from a manifest and checked by an inside probe, outside
  kernel tables and canaries, the world matched: 9/9 predictions; a
  smuggled socket was Denied (measured: CAPABILITY_MANIFEST_V0).

**`mutable(O)` must be closed over delegation.** Authority O can obtain
by asking a spawner outside is part of it (measured: CONTROLLED_DOMAIN_V0,
5 of 11 behaviours).

**The formula must hold for the decision's duration.** A manifest check
is *"one instant"* (CAPABILITY_MANIFEST_V0, "Time"). Atomic capture
against a same-uid writer was not achieved by any detection strategy.
Only the cgroup freezer gave true snapshots, 200/200, and only while
writers could not leave (measured: ATOMIC_CAPTURE_V0).

**Answer to 2:** the formula expresses I-eff, if:

- its objects are file handles or materialized content;
- `mutable` is constructed, not inferred, and closed over delegation;
- it is quantified over the decision window and over every live
  principal.

It does not express I-out at all.

## 3. What evidence would prove it?

### 3.1 For I-eff: all measured as components, never assembled for an observer

| Evidence | Establishes | Measured where | Residue that stays trusted |
|---|---|---|---|
| a world constructed from a manifest; inside walk with `access(2)`; outside kernel tables (namespaces, fd table); canaries | `mutable(O)` ⊆ declared | CAPABILITY_MANIFEST_V0 (9/9) | the kernel; `prepare`/`construct`; the probe and its interpreter; supplementary groups (unprivileged runtimes cannot drop them; argued away via a complete `access(2)` walk) |
| observed objects as file handles, or as a read-only view materialized from the snapshot | `observed_objects` well defined, disjoint from the live tree | CAPABILITY_OBJECT_IDENTITY_V0 (6/6), CAPABILITY_CONTENT_IDENTITY_V0 (9/9), SNAPSHOT_CAPABILITY_BOUNDARY_V0 (B cannot change its view) | the namespace owner (A5, A6: detected, not prevented) |
| cgroup subtree + cgroupns + mountns + netns around O | no process-tree or delegated writer outside O's world | CONTROLLED_DOMAIN_V0 (11/11 at L7–L8) | sockets of spawning services not hidden by the allowlist view; harness tools (CAPABILITY_INVENTORY_V0, not OS-visible) |
| the same around the actor, frozen across the decision | no actor process writes during the decision (X4) | CONTROLLED_DOMAIN_V0 (freeze), ATOMIC_CAPTURE_V0 (200/200 under the freezer) | writers that left the cgroup before the freeze (closed only by cgroupns) |
| re-verification from outside after the decision | detection of anything the above missed | SNAPSHOT_CAPABILITY_BOUNDARY_V0 (root ≠ S after A5/A6) | detection only; the change has already happened |

Every row is an ordinary `Fact` pin or `Within` over names in the
archived work. The kernel was unchanged throughout: capability C1–C3,
identity as a `Fact` pin, containment as `Within` (measured: kernel
sha256 unchanged in each report).

### 3.2 For I-out: no evidence of the capability kind exists

What would have to be shown is that **O's output is a fixed function of
S's bytes, and S is not interpreted as code**. The only measured
observers with that property are:

- **`FsSnapshot`** recomputes the git tree id from bytes
  (`a_claimed_snapshot_id_is_recomputed_not_believed`). S's content is
  the hash input, never the hash function.
- **The capability probe** ran as the world's only process, with its
  source on argv, so the world held no observer files
  (CAPABILITY_MANIFEST_V0).

In both, the evidence of independence is **the observer's own code**,
part of the trusted crate. It is not anything observed at run time.

Consider a run-time record of what O interpreted. The model's §5.3
showed `Within` could judge one. It would itself be produced by an
observer of O's execution, and that observer's influence-independence
from O is the same question again. World W measured the failure shape:
code that runs before the reporter controls the report.

## 4. A) better manifests, B) execution isolation, or C) a new kernel concept?

| | I-eff (G′) | I-out (X1) | decision currency (X4) |
|---|---|---|---|
| **A) better observer manifests** | **insufficient.** A manifest is a declaration. Authority is not inferable by observation (CAPABILITY_INVENTORY_V0); a manifest becomes evidence only when the world is **built** from it | **cannot express it.** The runner's manifest is "read S1, execute `python3`". That is legitimate, and it is exactly X1 | not about the observer |
| **B) an execution isolation layer** | **required**, with A as its input. Construction + containment + IPC closure, each measured; residue: namespace owner, supplementary groups, harness tools | **does not help.** A perfectly isolated runner still reports S1's chosen exit status. X1 needs no authority beyond what isolation grants | **required, for the actor**, not the observer: freeze across the decision |
| **C) a new kernel concept** | **not required.** Every component was expressed as `Fact` and `Within` with `kernel.rs` unchanged | **does not help.** The kernel judges values. Whether a value is influence-independent is decided before the value exists, by what O computes | not required |

**Answer to 4:**

- **I-eff needs B**, of which A is a part. Not C.
- **X4 needs B applied to the actor.**
- **I-out is solved by none of A, B or C.** It is a property of the
  observer's function and of the rule that chose it (adequacy, in the
  independence model). Today it is decided at registration, an
  assumption. It is established only where the observer is v9r's own
  fixed function over S-as-data.

## 5. Three observer models compared

*Definitions:*

- **Trusted observer:** today's `Trust::Attesting`. Registered, believed,
  ambient authority.
- **Capability-scoped observer:** O's read, write and exec authority is
  declared in a manifest. The world is not constructed from it, or is
  checked by observation alone.
- **Isolated observer:** O runs in a world **constructed** from its
  manifest (CAPABILITY_MANIFEST_V0), on a view materialized from the
  snapshot (CAPABILITY_CONTENT_IDENTITY_V0,
  SNAPSHOT_CAPABILITY_BOUNDARY_V0), inside cgroup + cgroupns + mountns
  + netns (CONTROLLED_DOMAIN_V0).

| | Trusted | Capability-scoped | Isolated |
|---|---|---|---|
| **what establishes O's authority** | nothing; registration | a declaration | construction, checked by an inside probe, outside kernel tables and canaries |
| **I-eff** | assumed | declared, not established: ambient authority is not inferable by observation | established up to measured residue |
| **I-out** | assumed | assumed: a manifest cannot say "data, not code" | assumed: isolation bounds what O touches, not what controls O's output |
| **X1** (S1 controls report) | **Allow, measured** | Allow: the runner's declared authority suffices | Allow: same, X1 uses no excluded authority |
| **G′** (O's execution writes live state) | Allow, predicted (unmeasured) | Allow if the declaration is wrong, and nothing checks it | prevented for O's own processes and delegated spawns, as measured for actors. The namespace owner can still change O's view: **detected, not prevented** |
| **X4** (actor's process writes during decision) | **Allow, measured** | unchanged: not O's authority | unchanged unless the **actor** is also isolated and frozen across the decision |
| **what is still trusted** | O entirely | O's function; the declaration | O's function; kernel, construction code, probe and interpreter; the namespace owner; harness tools |
| **cost (measured)** | none | none at run time | per world: a 40-step plan, 23 of them read-only binds for the interpreter closure (CAPABILITY_MANIFEST_V0); unprivileged on this host except dropping supplementary groups and the tool plane |

**Reading the table:**

- From trusted to capability-scoped moves **nothing** from assumption to
  evidence. A declaration of authority is a claim until the world is
  built from it.
- From capability-scoped to isolated moves **I-eff** from assumption to
  evidence, with measured residue.
- **No column moves I-out.** The X1 row is Allow in all three.

## 6. What this document establishes

1. An observer is an actor **for authority** and a function **for
   influence**. Capability and state checks cover only the first.
2. `observed ∩ mutable(O) = ∅` is a correct statement of I-eff, if:
   - the objects are file handles or materialized content;
   - `mutable` is constructed and closed over delegation;
   - it holds for the whole decision, for every live principal.

   It says nothing about X1.
3. The evidence for I-eff exists as measured components (archived),
   all on the unchanged kernel. Its residue is measured: the namespace
   owner, supplementary groups, harness tools, and an instant rather
   than an interval unless frozen.
4. I-eff needs isolation (B). X4 needs isolation of the actor (B).
   Neither needs a new kernel concept (C). Better manifests (A) are an
   input to B, not a substitute.
5. I-out is not reducible to authority. It holds only where S is data
   to the observer's fixed code, which today means only v9r's own
   verifiers. For every other observer it remains a registration-time
   assumption, and X1 shows that it fails for the demo's.

**Not measured, and not assumed here:** G′ itself; any isolated observer
in v9r's evidence path (the archived isolation was built for agent
worlds, never for an observer of a transition); whether an observer
can be classified as data-only by anything other than reading its code.
