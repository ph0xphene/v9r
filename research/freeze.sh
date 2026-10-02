#!/usr/bin/env bash
# v9r review freeze (V9R_REVIEW_FREEZE_CHECKLIST_V0). Stops at the first
# failure and changes nothing it has not reported. Run from the repo root:
#   bash research/freeze.sh 2>&1 | tee /tmp/v9r-freeze.log
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH=/nix/store/gmgfpzc0w2pgby0lmq9r88m39albiyfw-cargo-1.98.1/bin:/nix/store/hr3npzrq9l029s5hf948by203m6qz3lw-rustc-wrapper-1.98.1/bin:$PATH
CO='Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>'
die() { echo "FREEZE STOPPED: $*" >&2; exit 1; }

echo "== 0. preconditions"
[ "$(git rev-parse --abbrev-ref HEAD)" = research/effect-runtime-v0 ] || die "not on research/effect-runtime-v0"
git rev-parse -q --verify refs/tags/v9r-review-v0 >/dev/null && die "tag v9r-review-v0 already exists"
echo "HEAD $(git rev-parse HEAD)"

echo "== 1. remove the superseded experiment"
rm -f crates/v9r-core/tests/agent_transition.rs

summarize() { # sum cargo's "test result" lines
  awk '/test result:/ {p+=$4; f+=$6; i+=$8} END {printf "%d passed, %d failed, %d ignored", p, f, i}'
}

echo "== 2. full workspace verification"
set +e
cargo test --offline --workspace --no-fail-fast > /tmp/v9r-freeze-tests.log 2>&1
status=$?
set -e
grep -E '^test .*FAILED|panicked' /tmp/v9r-freeze-tests.log || true
TESTS=$(summarize < /tmp/v9r-freeze-tests.log)
echo "result: $TESTS (cargo exit $status)"
[ "$status" -eq 0 ] || die "test suite failed; see /tmp/v9r-freeze-tests.log"
grep -q 'kernel_is_unchanged_and_knows_no_delegation ... ok' /tmp/v9r-freeze-tests.log \
  || cargo test --offline -p v9r-core --lib kernel_is_unchanged_and_knows_no_delegation 2>&1 | grep -q '1 passed' \
  || die "kernel hash guard did not pass"
KHASH=$(sha256sum crates/v9r-core/src/kernel.rs | cut -c1-64)
[ "$KHASH" = 85badb669f5075458e2e934527c3aea040006276a3e3ec33310437cd6076177f ] || die "kernel.rs hash $KHASH"
KERNEL="passed (sha256 ${KHASH:0:8}…${KHASH:59:5})"
echo "kernel: $KERNEL"

echo "== 3. repository state"
git status --short
EXPECTED='Cargo.lock
Cargo.toml
crates/v9r-core/Cargo.toml
crates/v9r-core/examples/
crates/v9r-core/src/authority.rs
crates/v9r-core/src/authority/
crates/v9r-core/src/capability.rs
crates/v9r-core/src/delegation.rs
crates/v9r-core/src/delegation/
crates/v9r-core/src/lib.rs
crates/v9r-core/src/object_identity.rs
crates/v9r-core/src/snapshot.rs
crates/v9r-core/src/world_probe.py
crates/v9r-core/tests/capability_manifest.rs
crates/v9r-core/tests/capability_probe.py
crates/v9r-core/tests/content_identity.rs
crates/v9r-core/tests/controlled_domain.rs
crates/v9r-core/tests/object_identity.rs
crates/v9r-core/tests/snapshot_boundary.rs
crates/v9r-core/tests/state_vs_causality.rs
research/'
ACTUAL=$(git status --porcelain | cut -c4- | sed 's#^research/.*#research/#' | sort -u)
UNEXPECTED=$(comm -13 <(echo "$EXPECTED" | sort -u) <(echo "$ACTUAL") || true)
[ -z "$UNEXPECTED" ] || die "unclassified changes, classify before committing:
$UNEXPECTED"

echo "== 4. commits"
c() { git commit -q -m "$1" -m "$2" -m "$CO"; echo "$(git rev-parse --short HEAD) $1"; }
git add Cargo.toml Cargo.lock crates/v9r-core/Cargo.toml crates/v9r-core/src
c "feat(capability): manifest worlds, delegation ledger, signed authority, object and snapshot identity" \
  "Capability layer above the unchanged kernel: worlds built from a manifest (capability.rs, world_probe.py), delegation compiled to kernel obligations (delegation.rs), Ed25519-rooted ledger (authority.rs, ring), object identity by file handle (object_identity.rs), verified snapshot checkout and sealed snapshot views (snapshot.rs, capability.rs)."
C1=$(git rev-parse HEAD)
git add crates/v9r-core/tests/capability_manifest.rs crates/v9r-core/tests/capability_probe.py \
        crates/v9r-core/tests/content_identity.rs crates/v9r-core/tests/controlled_domain.rs \
        crates/v9r-core/tests/object_identity.rs crates/v9r-core/tests/snapshot_boundary.rs
c "test(capability): controlled domain, manifest, identity and boundary experiments" \
  "Live experiments behind CONTROLLED_DOMAIN_V0, CAPABILITY_INVENTORY_V0, CAPABILITY_MANIFEST_V0, CAPABILITY_OBJECT_IDENTITY_V0, CAPABILITY_CONTENT_IDENTITY_V0 and SNAPSHOT_CAPABILITY_BOUNDARY_V0."
C2=$(git rev-parse HEAD)
git add crates/v9r-core/tests/state_vs_causality.rs crates/v9r-core/examples/v9r_demo.rs
c "test: state vs causality; example: v9r demo" \
  "An authorized agent and an unauthorized writer of identical bytes give identical verdicts (STATE_VS_CAUSALITY_V0). Deterministic five-run demo (V9R_DEMO_IMPLEMENTATION_V0)."
FREEZE_DOCS='research/V9R_RESEARCH_MILESTONE_V0.md research/V9R_ARCHITECTURE_OVERVIEW_V0.md research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md research/V9R_REVIEW_FREEZE_CHECKLIST_V0.md research/freeze.sh'
git add research
git reset -q -- $FREEZE_DOCS
c "docs(research): containment, capability, agent transition (historical) and state-vs-causality reports" \
  "Reports as written at the time; later corrections live in later reports, not as edits to earlier ones."
for f in research/V9R_ARCHITECTURE_OVERVIEW_V0.md research/V9R_EXTERNAL_REVIEW_PACKAGE_V0.md; do
  sed -i "s|@@TESTS@@|$TESTS|g; s|@@KERNEL@@|$KERNEL|g" "$f"
done
git add $FREEZE_DOCS
c "docs(research): milestone, architecture overview, external review package, freeze checklist" \
  "Review revision. Full suite: $TESTS. kernel.rs hash guard: $KERNEL."
[ -z "$(git status --porcelain)" ] || die "working tree not clean after commits"

echo "== 5. per-commit build and test (commits 1 and 2; later commits add no code or only code tested above)"
export CARGO_TARGET_DIR=/tmp/v9r-freeze-target
for rev in $C1 $C2; do
  wt=$(mktemp -d /tmp/v9r-freeze-wt.XXXX)
  git worktree add -q --detach "$wt" "$rev"
  set +e
  (cd "$wt" && cargo test --offline --workspace --no-fail-fast > "$wt.log" 2>&1)
  s=$?
  set -e
  echo "$(git rev-parse --short "$rev"): $(summarize < "$wt.log") (exit $s)"
  git worktree remove --force "$wt"
  [ "$s" -eq 0 ] || die "commit $rev does not pass its tests; see $wt.log (commits made, no tag)"
done

echo "== 6. tag"
git tag -a v9r-review-v0 -m "v9r external review revision. Full suite: $TESTS. kernel.rs hash guard: $KERNEL."
echo "tag v9r-review-v0 -> $(git rev-parse v9r-review-v0^{commit}) (tag object $(git rev-parse v9r-review-v0))"

echo "== 7. summary"
git log --oneline 8a49c4b..HEAD
echo "tests: $TESTS"
echo "kernel: $KERNEL"
