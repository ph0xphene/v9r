# Capability Composition Design v0: can delegation be represented without weakening the invariant kernel?

Research question: **can capability delegation and multi-agent
composition be represented without weakening the invariant kernel?**

> **Status: design only.** No code was written or changed. `kernel.rs`
> sha256 `85badb66…6177f`, unchanged. Every claim below is marked as one
> of three kinds:
>
> - **established**: measured or proved in an earlier report, cited by
>   its label (F1–F17);
> - **hypothesis**: believed, not yet tested (H1–H9);
> - **proposed**: a design choice. It can be wrong, but it is not a
>   claim about the world.
>
> Each hypothesis has at least one experiment in
> [Falsification experiments](#falsification-experiments) that could
> refute it.

## Short answer

1. **Capability is two things, and v9r needs both.**
   - *Holding* is an **attribute**: what a world can do at one instant.
     It is observable, and Capability Manifest v0 already checks it
     (C1–C3).
   - *Authority* is a **relationship**: a chain of grants that ends at a
     root. It cannot be observed. It is declared, and the runtime must
     check it before it builds a world.

   The 3-tuple `Capability(agent, resource, op)` describes the
   attribute. It cannot answer "who granted this", "can it be revoked"
   or "did anyone amplify it". The 5-tuple `Delegation(...)` describes
   one edge. The model needs the edges, plus one rule: **observed
   holdings ⊆ the authority the chain derives.**
2. **The kernel's existing forms can express every edge check.**
   - *Attenuation* (the child's scope lies inside the parent's) is
     `Within`, with the child's scopes passed as names.
   - Expiry and depth bounds are `AtMost`.
   - That a grant exists and is not revoked is a `Fact` pinned to a
     ledger the runtime attests, the same idiom as `transition.ordered`
     in Temporal v0.
   - No new requirement form is needed. **Verdict B:** provenance belongs
     in a trusted layer above the kernel. It must not become kernel
     state.
3. **Names are not identities.** Attenuation checked over path names
   is sound only if the name resolves to the same object in the
   grantor's view and in the grantee's. Bind mounts are non-recursive
   (F6), and one host uid is shared by every world (F7). Both break that
   correspondence. Grants must therefore attach to:
   - **object identity resolved inside the grantor's view**, for
     mutable state;
   - **content identity**, for immutable state.

   Paths only locate objects. Abstract capability IDs name *grants*, not
   objects.
4. **Per-world invariants cannot detect the confused deputy.** In the
   deputy case, B acts entirely within B's authority, so B's C1 is
   Satisfied, and correctly so. Temporal v0 already found that a
   transition is judged by *what* changed, never *who* (F9).

   This design does not add attribution to the kernel. It **constructs
   the deputy's world per request, from the intersection of the caller's
   and the callee's authority.** That turns "B acted for A beyond A's
   authority" into the same C1 check, applied to a smaller world.

   What this cannot cover: state the deputy keeps across requests. That
   is the open edge of this design.
5. **Sharing a writable object merges domains.** Two worlds that can
   both write one object are one quiescence domain and one transition
   domain (F10, F15). That is a consequence of the established results,
   not a choice.

## 1. Established facts

From earlier reports and from reading `kernel.rs`. Nothing here is new.

| # | Fact | Source |
|---|---|---|
| F1 | The kernel has three requirement forms: `Fact` (exact value, hard or soft), `Within` (component-bounded prefix containment over `/`-separated names, with `UnknownBelow`), `AtMost` (numeric bound). Evaluation is three-valued; Unknown is never true or false | `kernel.rs` |
| F2 | **`Within` and `AtMost` consult no evidence.** They judge the names and numbers written into the obligation. Their soundness rests on whoever wrote the obligation. C1 is sound because the domain layer fills `names` from verified observation | `kernel.rs` (`check_within`, `AtMost` arm) |
| F3 | The kernel cannot compare two unknown values. Relations are expressed by reading verified values, deriving ordinary obligations from them, and **pinning** each value read as a `Fact` | Temporal Evidence v0 |
| F4 | `Verified` evidence can be created only inside v9r-core. On the graph path, registering a provider is the trust decision, and a single trusted liar is believed | Evidence Graph v0 |
| F5 | Capability names form one hierarchical namespace: `fs/read<p>`, `fs/write<p>`, exact `…/.` skeleton entries, `env/<N>`, `proc/descendants`, `special<p>` (never declarable), `net/…` (never declarable). C1 = observed ⊆ declared, as `Within` | Capability Manifest v0 |
| F6 | C2 compares **identity, not names**: `(st_dev, st_ino)` of each declared path on the host against `lstat` inside. Binds are **non-recursive**, so a submount below a declared path is not carried into the world | Capability Manifest v0 |
| F7 | An unprivileged runtime cannot drop supplementary groups or change uid. **Every world runs as the same host uid**, so OS DAC cannot tell two worlds apart. Only their mount views separate them | Capability Inventory v0, step 1; Manifest v0, residue |
| F8 | Processes of the runtime's uid on the host hold every capability in a world's user namespace (inbound authority). Sibling worlds are not such processes | Capability Manifest v0, evaluation |
| F9 | **A transition is judged by what changed, never by who changed it** | Temporal Evidence v0 |
| F10 | Freezer quiescence gives true snapshots **only if every writer is inside the frozen cgroup and cannot leave**. A cgroup namespace stops migration | Atomic Capture v0; Controlled Domain v0 |
| F11 | **The confused deputy has already been measured at the OS level.** The largest escape class was an agent *asking* an outside spawner (systemd user manager, host daemons) to act. The spawner's authority was legitimate, so no cgroup layer could see it. Only cutting the channel (mount view, netns) closed it | Controlled Domain v0 |
| F12 | Inside a manifest world there are no sockets, no network, no `/proc`, no inherited fds, and a constructed env. **The only channel between two worlds is a shared filesystem object**, plus whatever the runtime itself provides | Capability Manifest v0 |
| F13 | An observation is one instant. A socket bound into a declared write path *after* the probe ran is not seen. World B shows the checker would catch it if it observed again | Capability Manifest v0 (Time); P8 |
| F14 | Content-addressed objects can be verified by anyone, from bytes supplied by anyone. A snapshot is a git tree | Content-Addressed State v0 |
| F15 | Two domains that share a substrate cannot have independent transition invariants | Temporal Evidence v0 |
| F16 | Path-based check-then-use is racy: 3 in 200,000 listings escaped the root. fd-relative resolution closed it | Atomic Capture v0 |
| F17 | Unknown is never absence. An observer gap yields `UnknownBelow`, never an `Absent` | `facts.rs`; Capability Manifest v0 |

Also established, and outside this design: the harness tool plane
(email, publishing, remote agents) is authority no OS construction
touches (Inventory v0, finding 4). Delegation of harness tools is
**not** covered here.

## 2. Analysis

### Q1. Attribute or relationship?

**`Capability(agent, resource, operation)`** is what Manifest v0 checks
today, with `agent` = the world:

- `holds(W, name)` is observed;
- `declared(W)` is the manifest;
- C1 checks `holds ⊆ declared`.

There is exactly one grantor, the runtime, so the relationship is
implicit. **Manifest v0 is already delegation, with one edge:
runtime → W.**

With a second agent, the attribute form loses information it cannot
recover:

| Question | Attribute form | Relationship form |
|---|---|---|
| Is B's holding legitimate? | only "B's manifest says so" | B's grant chain reaches the root, and every edge attenuates |
| Did A have it to give? | not expressible | the parent edge's scope contains the child's |
| Who can revoke it? | not expressible | anyone on the chain above it (proposed rule) |
| What breaks if A's grant is revoked? | not expressible | every descendant edge |
| Did B amplify? | not expressible | an edge whose scope is not within its parent's |

**Proposed:** keep both, as two layers.

- `holds(W, name)`: **attribute, observed**, per world, per instant.
  Unchanged from Manifest v0.
- `Grant(id, parent, grantor, grantee, scope, objects, constraints)`:
  **relationship, declared**, issued by the runtime.
- The binding rule: `holds(W) ⊆ ⋃ { scope(g) | g a live grant to W }`,
  checked as C1 against the derived scope set.

The 5-tuple in the question has an **`owner`** field. The proposed
record replaces it with `parent`:

- Ownership is *derived*: walk `parent` up to the root.
- A stored owner could disagree with the chain, and then the record
  would contradict itself.

### Q2. Minimum provenance model

Each question in the brief maps to one source:

| Question | Answered by | Kind |
|---|---|---|
| Who owns the authority? | the root of the `parent` chain | derived |
| Who granted it? | `grantor`, which must equal `parent.grantee` | field + check |
| To whom? | `grantee`: a **principal** (below) | field |
| For what object? | `objects`: name root → object identity | field, verified at issue |
| For what operation? | `scope`: capability names (F5 vocabulary) | field |
| Under what constraints? | `constraints`: only **monotone** ones (below) | field |
| Can it be revoked? | `revocation`: the **mechanism**, not a flag | field + check |

**Proposed record:**

```text
Grant {
  id:          sha256 of the canonical record (content-addressed, F14)
  parent:      GrantId | Root(root manifest digest)
  grantor:     Principal
  grantee:     Principal
  scope:       [CapName]                 // F5 names, Known only
  objects:     { name-root → ObjectRef } // see Q4
  constraints: { not_after: epoch, depth_left: u8 }
  revocation:  Teardown | Unmount        // see below
}

Principal = (plan digest, userns id, mntns id)  // bound when the world is built
```

**Principal** (proposed). An agent is identified by its world, not by
a name it gives itself:

- before construction, by the plan digest;
- after construction, by the namespace ids the runtime reads from
  outside (as in C3).

A uid cannot serve as the identity: every world has the same one (F7).

**Constraints must be monotone under attenuation** (proposed). Each
child must be at least as narrow as its parent, and that must be
checkable with an existing kernel form:

| Constraint | Edge check |
|---|---|
| scope | `Within(names = child.scope, scopes = parent.scope)` |
| expiry | `AtMost(child.not_after ≤ parent.not_after)` |
| redelegation | `AtMost(child.depth_left + 1 ≤ parent.depth_left)`; `depth_left = 0` means "not redelegable" |

Some conditions are not monotone, for example "only if tests passed" or
"only for commit X". Those are **invariants on use**, not grant
constraints. They stay ordinary obligations at decision time. Keeping
them out of grants keeps the edge check closed and decidable.

**Revocation is a property of the mechanism, not of the record**
(hypothesis H6). Two mechanisms are available to the runtime:

- `Teardown`: kill the grantee's world (`cgroup.kill`, F10). Sound by
  construction: nothing in the world survives.
- `Unmount`: from outside, enter the grantee's mount namespace and
  detach the bind. The runtime owns the userns (F8), so it can do this.

  Expected weakness: a file descriptor opened before the unmount
  survives a lazy detach. If that holds, `Unmount` revokes access by
  *name* only, and `Teardown` is the only sound revocation.

Read authority over content the grantee has already copied cannot be
revoked by any mechanism. **Proposed:** a read grant declares that its
content is disclosed, and revocation never claims to undo a disclosure.

### Q3. Confused deputy

Three scenarios, judged against the existing model (Manifest v0 C1–C3
per world, Temporal v0 transitions):

| Scenario | What per-world C1 says | Why |
|---|---|---|
| **S1.** A asks B to do something B may do and A may not (A names `B_private/x` in a request) | B: **Satisfied**. A: Satisfied | B acted within B's authority. Nothing B holds is undeclared. The deputy problem lies in the *designation* ("A chose the target"), and per-world holdings do not carry designation |
| **S2.** A gives B access to a shared object | both Satisfied | correctly: A had it. But (a) A and B are now co-writers, so neither world can be quiesced or judged alone (F10, F15); (b) anything A later moves or hard-links into the shared subtree is delegated **implicitly**, with no grant record |
| **S3.** B becomes an amplifier | depends on how | three routes, below |

The S3 routes:

- **Widening redelegation.** B passes on more than it got. The edge
  check (`Within` on scopes) catches it, provided redelegation goes
  through the runtime.
- **Name/object mismatch.** The name B passes on resolves to a
  *different object* in the new grantee's view (Q4): a submount, a
  swapped symlink, a moved directory. A name-level `Within` says Allow.
  Only an object-identity check catches it (H3).
- **Combination.** B holds grants from A and from D, and serves C. C's
  request makes B combine A's and D's authority. This is S1 with two
  sources.

**Can the existing facts and invariants express the confused deputy?**
Not as an attributed property. C1 is about one world. F9 says the
transition layer does not know who. **Adding "who" to the kernel is
the wrong fix:** attribution is not observable after the fact under
concurrency, which is the lesson of Atomic Capture v0.

**Proposed: make the deputy's authority a construction property.** A
request from A to B runs in a **fresh world for B**, built from:

```text
M(B for A, request r) = (scope(A) ∩ scope(B)) ∪ B_private_ro ∪ R_r
```

- `scope(A) ∩ scope(B)`: what both hold. For prefix scope sets, the
  intersection is computable: for each pair of scopes where one contains
  the other, keep the narrower one.
- `B_private_ro`: B's own code and configuration, **read-only**.
- `R_r`: a fresh write directory for this request's results.

Then "B acted beyond A's authority" is **C1 on B's per-request world**,
with no new kernel form. The per-request world also brackets the
effect, so the request's transition is attributed by construction, not
by observation.

The model does **not** cover the case where B needs private *write*
state that persists across requests (a cache, a queue, its own log).
That state is a channel. A request from A can leave an instruction in
it, and a later request from C can act on it ("deferred laundering",
H8). Proposed rule for v0: **persistent deputy write state is forbidden,
or is treated as held by every principal that ever wrote to it.**
Expressing the second option precisely would need information-flow
labels. They are not in this design. If they turn out to be necessary,
see [§4](#4-kernel-boundary).

**The request channel must not carry authority** (proposed). In a
manifest world, authority moves between worlds only through objects
(F12). An fd can move only over a unix socket (`SCM_RIGHTS`), and
sockets are never declarable (F5). So the runtime carries requests over
**pipes it owns**: a request can *designate* (bytes) and cannot
*transfer authority* (fds). The one gap is time: a socket bound into a
shared write path after observation (F13). Experiment X7 tests it.

### Q4. Object identity

| Identity | Stable under | Breaks under | Fit |
|---|---|---|---|
| **path** | nothing it does not control | rename, symlink swap (F16), different mount views, non-recursive binds (F6) | **locator only**. Never the subject of a grant |
| **(dev, ino)** | rename, multiple names, views that bind the same object | inode reuse after deletion; it says nothing about what lies *below* a directory (submounts, moved-in children); hard links give one inode many names | **mutable objects**, resolved inside the grantor's view and checked in the grantee's (extends C2) |
| **content (sha256 tree/blob)** | everything: anyone can verify it, it can be stored anywhere (F14) | it is not a thing you can write to | **immutable objects and read grants** (snapshots, inputs, env values) |
| **abstract capability ID** | whatever the issuer guarantees | needs an issuer and a table: that *is* the grant ledger | **names grants, not objects** |

**Proposed:**

- A grant's `objects` field maps each **name root** in its scope to an
  `ObjectRef`:
  - `Inode(dev, ino, mnt_id)` for mutable subtrees. The runtime
    resolves it **inside the grantor's world**, through
    `/proc/<grantor-init>/root/…`, fd-relative (F16). It does not
    resolve it on the host.
  - `Content(sha256)` for immutable inputs. The grantee receives a
    read-only materialization, and C2 checks it by content id.
  - `EnvValue(sha256)` for `env/<N>`. A name-level grant of `env/TOKEN`
    says nothing about its value, so the value digest is the identity.
- The grant ID is the content hash of the record. It is the "abstract
  capability ID", and its only job is to name an edge in the chain.

**Why the grantor's view, not the host path** (hypothesis H3):

- A's world binds host path `Q` non-recursively (F6).
- If the host has a mount at `Q/sub`, A sees the underlying directory
  there.
- A grant of `fs/write Q/sub`, bound into B *from the host path*, gives
  B the submount: objects A never held.
- `Within(fs/write Q/sub, fs/write Q)` is Satisfied anyway. **The name
  check passes and authority is amplified.**

Resolving the path in A's view and binding from that resolution (or
comparing `(dev, ino, mnt_id)` of both resolutions) closes this. X1
tests it.

**The `(dev, ino)` of a directory does not pin its subtree.** A co-writer
can move children in and out. That is not amplification, because the
co-writer held them already. It *is* implicit delegation (S2b), and the
ledger does not see it. Proposed v0 stance: a write grant on a subtree
is a grant over *whatever that subtree contains over its lifetime*, and
the record says so. X6 measures what that admits.

### Q5. Kernel boundary

**A: Is the Fact/Within/evidence model sufficient on its own?** No.
Kernel-only evaluation cannot know:

- whether a grant exists;
- whether a grant was revoked;
- which world a principal is;
- that a name resolves to the same object in two views.

Those are trusted state and trusted resolution, like Temporal v0's
snapshot identity, which also lives outside the kernel.

**B: Provenance as a trusted layer above the kernel.** Proposed and
expected. The layer (call it the **grant ledger**) does four things:

1. **Owns grant identity.** Only the ledger issues grants, as Temporal
   v0's clock is the only thing that issues snapshots. Agents submit
   *requests to delegate*. Those requests enter as `Proposed` evidence
   and are never trusted (F4).
2. **Attests ledger facts as `Verified`**, at a temporal snapshot:
   `grant(id) = record digest`, `live(id) = true @ s`. This is the same
   status as `transition.ordered`.
3. **Writes the edge obligations** from verified records, and pins every
   value it reads (F3):
   - `Fact(grant(parent) = digest)`, `Fact(live(parent) = true)`;
   - `Within(child.scope ⊆ parent.scope)`;
   - `AtMost` for expiry and depth;
   - `Fact(object(child, root) = parent-view resolution)` for each name
     root, attested by the runtime's fd-relative resolver.
4. **Builds the grantee's world** from the derived manifest. Manifest
   v0's C1–C3 then check that world, unchanged.

Why the kernel stays unchanged:

- `Within` is transitive on component-bounded prefixes. So chain
  attenuation is the conjunction of per-edge `Within` checks. **The
  kernel never needs to walk a chain.**
- Every edge check is one of the three existing forms (table in Q2).
- Revocation is "`live` is not true at the snapshot under check".
  F17 makes a missing `live` fact **Blocked**, never Allow. That is the
  right failure direction.

**C: Is the kernel abstraction insufficient?** Not on this analysis.
[§4](#4-kernel-boundary) lists the specific findings that would make it
C.

One property of the kernel needs naming, because the model leans on it
(F2): **`Within` trusts its names.** An edge `Within` is only as sound
as the ledger that copied `child.scope` out of a verified record. The
pins in step 3 are what tie the evidence-free check to verified state.
A ledger that forgets a pin makes an attenuation check about
fabricated names. X2 includes a mutation for this.

## 3. Proposed model: Capability Composition v0

The smallest model that answers Q1–Q5. All of it sits above the kernel.

```text
              root manifest  (trusted configuration; its digest is the chain's root)
                    │
     ┌──────────────┴───────────────┐   GrantLedger (trusted, above the kernel)
     │ issue(request: Proposed)     │    - verifies parent live, grantor = parent.grantee
     │   → Grant (content-addressed)│    - resolves objects in the GRANTOR's view, fd-relative
     │ revoke(id) → teardown        │    - attests grant(id), live(id)@s, object(id, root)
     └──────────────┬───────────────┘
                    │ edge obligations (existing forms only)
                    │   Fact pins · Within(child ⊆ parent) · AtMost(expiry, depth)
                    ▼
               kernel::evaluate  (unchanged)
                    │ Allow
                    ▼
     derive manifest: scopes of live grants to W  ──plan()──▶  world W
                    │
                    ▼
     Manifest v0 C1–C3 on W (unchanged): holds(W) ⊆ derived scopes; objects = granted identities

     Deputy call A → B (request r):  fresh world from (A ∩ B) ∪ B_private_ro ∪ R_r,
                                     request bytes over a runtime pipe; C1 on that world
```

**Rules:**

| # | Rule | Enforced by |
|---|---|---|
| R1 | Only the ledger issues grants. Agent delegation requests are `Proposed` | F4: `Verified` is crate-private |
| R2 | `grantor = parent.grantee`, and the parent is live at issue | `Fact` pins |
| R3 | Attenuation: scope, expiry, depth | `Within`, `AtMost` per edge |
| R4 | Objects are resolved in the grantor's view. Mutable → `Inode`; immutable → `Content`; env → `EnvValue` | ledger resolver + C2 in the grantee's world |
| R5 | A world's manifest is derived from its live grants and nothing else | `plan()` input |
| R6 | Revocation = teardown of every world holding a descendant grant (v0). `Unmount` only if X3 supports it | ledger + `cgroup.kill` |
| R7 | Deputy calls run in a per-request world from the intersection. No persistent deputy write state | construction |
| R8 | Inter-world requests travel over runtime pipes. No world may hold a socket | F5 (`special` never declarable) + X7 |
| R9 | Worlds that share a writable object are one quiescence and transition domain: frozen together, snapshotted together | F10, F15 |
| R10 | Accepted residue (groups) is the same for every principal and is not attenuable. Like Manifest v0, it is accepted only on the `access(2)`-over-a-complete-walk argument | Manifest v0 residue |

**Explicitly not in v0:**

- harness tool delegation;
- network grants;
- delegation between hosts;
- information-flow labels;
- partial revocation of a disclosed read;
- non-monotone grant constraints;
- persistent deputies.

### Hypotheses

| # | Hypothesis | Tested by |
|---|---|---|
| H1 | Every edge check fits `Fact`/`Within`/`AtMost`; no new kernel form is needed | X2 |
| H2 | Per-edge `Within` composes into chain attenuation (transitivity), including the exact `…/.` skeleton entries: an exact parent never admits a prefix child, and a grantee's skeleton ancestors are within the grantor's | X2 |
| H3 | Name-level attenuation is **unsound**, and object identity resolved in the grantor's view is **sound**, for mutable subtrees | X1 |
| H4 | Per-world C1 cannot detect a confused deputy (S1); C1 on a per-request intersection world can | X4 |
| H5 | Two worlds sharing a writable object cannot be quiesced or judged separately | X5 |
| H6 | `Unmount` revocation leaves pre-opened fds usable; only `Teardown` revokes soundly | X3 |
| H7 | Shared write subtrees carry implicit delegation (rename, hard link) that the ledger does not record, though it never amplifies | X6 |
| H8 | A deputy with persistent write state launders requests across principals, and per-request attribution misses it | X4c |
| H9 | Pipes as the only request channel prevent authority transfer; sockets are the only fd-transfer route, and a shared write path re-opens it over time | X7 |

## 4. Kernel boundary

**Decision (design-level): B.** The provenance model is a trusted layer
above the kernel, structurally the same as the temporal layer:

- it owns identities (grants, as Temporal v0 owns snapshots);
- it attests a few facts about its own state;
- it emits ordinary obligations with pinned values.

`kernel.rs` stays untouched.

**This becomes C** if any of the following is observed:

1. **An edge check that no existing form expresses.** For example, a
   needed attenuation over non-prefix structure ("only `*.log` files",
   "append only"), where the honest encoding is neither a name hierarchy
   nor a bound. That would need a new requirement form.
2. **Deputy safety that needs information flow** (H8 holds, and no
   construction rule removes it). Labels propagated through state are
   not expressible as `Fact`/`Within`/`AtMost` over finitely many
   pinned values. They need either quantification over histories or a
   label lattice in the kernel.
3. **Revocation that must be judged across time inside one decision**
   (for example, "no use after revocation" over a window, not at a
   snapshot). The kernel has no time. The temporal layer gives it pairs
   of snapshots, not intervals.
4. **The intersection world is impossible** for real deputies. If every
   useful deputy needs authority beyond `A ∩ B`, then only attribution
   can separate legitimate from confused use. Attribution under
   concurrency was shown to need quiescence (F10), so it would be a
   runtime question first. It becomes a kernel question only if "who"
   must be compared across principals.

**This would weaken the kernel, and is rejected in advance:**

- making `Within` consult evidence for its names (a kernel change for a
  ledger bug);
- accepting agent-asserted grants as anything above `Proposed`;
- adding "principal" as a kernel concept. It lives in subject keys,
  like everything else.

## Falsification experiments

Ordered by how much of the design each one can refute. All are
unprivileged, on this host, and reuse Manifest v0 worlds.

### X1. Name vs object attenuation (H3), the critical one

**Setup:**

- A's world write-binds `<base>/q`.
- On the host, a tmpfs is mounted at `<base>/q/sub`. Mounting it needs
  an outer mount namespace that the runtime owns, so the test runs the
  whole experiment inside one.
- A therefore sees the underlying (empty) `sub`.
- A requests a grant of `fs/write<base>/q/sub` to B.

**Arms:**

- (a) bind B from the host path;
- (b) bind B from A's view (`/proc/<A-init>/root/<base>/q/sub`, opened
  fd-relative);
- (c) as (a), plus a ledger `object` check comparing `(dev, ino,
  mnt_id)` across the two resolutions.

A second variant swaps `q/sub` for a symlink between issue and binding
(F16-style race, 10⁵ attempts).

**Predictions:**

- (a) edge `Within` Allow, B's C1 Allow, and B holds the tmpfs's
  canary;
- (b) B holds only what A saw;
- (c) Deny.

**Falsified if** (a) does not amplify, which would mean name-level
attenuation is sound here and R4 is unnecessary cost. Also falsified if
(b) or (c) still amplify, which would mean object identity is not
enough.

### X2. Chain attenuation in existing forms (H1, H2)

**Setup:** synthetic ledgers, no worlds (unit level). Chains of depth
1–4 with:

- an honest chain;
- widening at depth 2;
- an exact parent `fs/read/nix/.` with a prefix child `fs/read/nix`;
- a child whose skeleton ancestor lies outside the grantor's scope;
- expiry extension;
- redelegation with `depth_left = 0`;
- a revoked middle edge;
- a missing `live` fact.

**Predictions:**

- honest → Allow;
- each violation → Deny;
- missing `live` → Blocked;
- every case evaluated by unmodified `kernel::evaluate`.

**Mutations:**

- drop the `Fact` pins: a fabricated `child.scope` is then accepted,
  which demonstrates F2's dependency;
- drop the per-edge `Within`: widening is then accepted.

**Falsified if** any case needs a requirement form the kernel lacks,
or if transitivity fails for some scope pair the vocabulary can
produce.

### X3. Revocation (H6)

**Setup:**

- B holds a write grant on `<base>/shared`.
- B's agent opens `shared/f` and holds the fd, then signals ready.
- The runtime revokes the grant:
  - arm (a): `Unmount` (setns into B's mount ns, `umount2(MNT_DETACH)`);
  - arm (b): `Teardown`.
- After revocation, a writer in A's world checks whether `f` still
  changes.

**Prediction:** (a) B keeps writing through the held fd; (b) it stops.

**Falsified if** (a) revokes completely, which would make `Unmount` a
sound and cheaper R6. Also falsified if (b) leaves a writer, which would
mean F10 containment fails under a shared object.

### X4. Confused deputy (H4, H8)

**Setup:**

- B holds `fs/write<base>/release`; A does not.
- A sends B, over a runtime pipe, a request naming
  `<base>/release/x`.

**Arms:**

- (a) B is a persistent world holding its full grant;
- (b) B runs in a per-request world from `A ∩ B`;
- (c) B keeps a persistent queue in a private write path:
  - request 1 from A enqueues "write `release/x`";
  - request 2 from C (who holds `release`) runs the queue;
  - both run per-request.

**Predictions:**

- (a) `release/x` written; every C1 Allow, which shows the gap;
- (b) `ENOENT` inside, nothing written, C1 Allow;
- (c) `release/x` written during C's request; both per-request C1 Allow.
  This would be a false negative, demonstrating H8.

**Falsified if** (a) is detected by an existing per-world or
transition invariant (the gap does not exist), or if (c) is caught
(persistent deputy state is safe after all). If (c) holds, R7's ban is
required, and §4 item 2 decides whether v9r stays at B.

### X5. Shared-object quiescence (H5)

**Setup:**

- A and B each in their own cgroup subtree (Controlled Domain L8).
- Both hold write on `<base>/shared`. B runs Atomic Capture v0's
  continuous writer there.
- 200 captures of A's domain each for:
  - (a) freezing A only;
  - (b) freezing the write-sharing component {A, B}.

**Prediction:** (a) false snapshots at about the unfrozen rate; (b)
200/200 true.

**Falsified if** (a) is clean. That would mean per-world quiescence
survives sharing, and R9 is unnecessary.

### X6. Implicit delegation through a shared subtree (H7)

`link(2)` and `rename(2)` refuse to cross mount points (`EXDEV`), even
within one filesystem. So the experiment has two layouts.

**Setup:**

- **(a) Separate binds.** A holds `fs/write<base>/a_private` and
  `fs/write<base>/shared` as two binds. B holds only `shared`.
- **(b) Subtree grant**, the normal delegation case. A holds
  `fs/write<base>/work` as one bind and grants B
  `fs/write<base>/work/shared`.
- In each layout, A hard-links a private file `f` into `shared/f`. As a
  separate case, A renames a private directory into `shared`.
- B writes through `shared/f`.

**Measure:**

- whether the link or rename succeeds;
- whether A's private `f` changed;
- whether B's C1 and A's C1 are Allow;
- whether an inode-set comparison of `shared` (at grant time vs now)
  detects the new object.

**Prediction:**

- (a) `EXDEV` for both link and rename. Separate binds make the
  implicit channel copy-only: data moves, inodes do not.
- (b) both succeed. A's private `f` changes, and both C1 checks are
  Allow. Nothing is amplified (A held `f`), and the ledger has no
  record. The inode-set comparison detects the new object.

**Falsified if** (a) succeeds, which would mean the mount boundary is
not a delegation boundary. Also falsified if (b) fails, for example
through `protected_hardlinks`, which would make S2b narrower than
claimed. If the predictions hold, R4 gains a construction rule: **bind a
granted subtree separately in the grantor's world too**, so that
implicit delegation through it becomes copy-only.

### X7. Authority transfer over time (H9)

**Setup:**

- A and B share a write path. B's initial observation is clean.
- A then binds a unix socket in the shared path and offers an fd over
  it. The fd leads to a file only A's view contains.
- B connects and receives the fd with `SCM_RIGHTS`.

**Measure:**

- whether B gains access outside its manifest;
- whether a re-observation (a second probe entering B's namespaces)
  flags `special/…` and the out-of-manifest object.

**Prediction:** the transfer works, the initial check misses it (F13),
and re-observation denies it.

**Falsified if** the transfer fails. That would make pipes-only
transport (R8) sufficient without continuous observation. It would also
be falsified if re-observation cannot see the received fd: without
`/proc` inside, only the outside fd-table reader (Manifest v0) could,
and that reader has to run again.

## Order of work

1. **X2** (unit level, no OS). It decides H1/H2, and with them whether B
   is viable at all.
2. **X1.** It decides what a grant attaches to. Everything else builds
   on R4.
3. **X4.** It decides whether the deputy problem stays at B or pushes to
   C.
4. **X3, X5, X6, X7.** These measure the cost and the edges of the
   model.

No implementation code should be written until X2's synthetic cases are
written as tests against the unmodified kernel. That is the first point
where the design can be wrong cheaply.

## Trusted computing base of a composition claim

The claim: "world W holds only authority that a live, attenuating chain
from the root granted it". It trusts:

- everything Manifest v0's claim trusts (kernel, `prepare`/`construct`,
  probe, interpreter, the `access(2)` argument);
- the ledger: issue, revoke, its attestations, and that it pins every
  value it writes into an obligation;
- the fd-relative resolver that reads object identity in the grantor's
  view;
- the root manifest, as trusted configuration;
- for deputy calls: the intersection computation, and that the deputy
  holds no persistent write state.

It does not trust any agent's statement about what it holds, what it
was granted, or on whose behalf it acts.
