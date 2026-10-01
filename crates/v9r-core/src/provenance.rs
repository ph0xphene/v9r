//! Explaining a decision by the lineage of its evidence.
//!
//! The kernel records, per finding, a human-readable reason; for a
//! verified fact that reason names the observer and the provenance basis,
//! which on the evidence-graph path is a `lineage:L<n>` token. This
//! module resolves those tokens in the registry's lineage store and
//! follows each fact's dependencies, so a verdict can be explained as
//! "ALLOW because evidence L12 (git, round 41, method …) …".
//!
//! It also audits the decision for provenance that does not fit:
//!
//! * verified provider evidence whose lineage cannot be resolved;
//! * evidence for a key at snapshot `n` whose lineage names another
//!   snapshot (evidence moved between moments);
//! * one invariant requiring the same value of facts established under
//!   different definitions (values compared across normal forms).
//!
//! Lineage fields are of two kinds and are shown apart: what the
//! registry *established* (provider, request, round, snapshot, that
//! dependencies came from the same response) and what the provider
//! *claimed* (method, definition, observed state).

use std::collections::BTreeMap;
use std::fmt;

use crate::graph::{lineage_ids, Lineage, Registry};
use crate::kernel::{Decision, Phase, Requirement, Status, Verdict};

/// Who established a finding's evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A provider fact, its lineage, and (depth-first) what it depends on.
    Lineage(Vec<(usize, Lineage)>),
    /// Established by a layer of v9r itself (runtime, temporal clock).
    Layer { observer: String, basis: String },
    /// A local check of the obligation itself (`Within`, `AtMost`).
    LocalCheck,
    /// No single verified fact behind it (unknown, contradictory, claims).
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explained {
    pub invariant: String,
    pub statement: String,
    pub status: Status,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Explanation {
    pub phase: Phase,
    pub verdict: Verdict,
    pub findings: Vec<Explained>,
    /// Provenance that does not fit; empty for a sound explanation.
    pub problems: Vec<String>,
}

/// (invariant, required value) → (fact statement, claimed definition).
type Compared = BTreeMap<(String, String), Vec<(String, Option<String>)>>;

/// Explain `decision`. `snapshot_of` names the snapshot a subject refers
/// to, if any (for the snapshot audit).
pub fn explain<S: fmt::Debug, V: fmt::Debug>(
    decision: &Decision<S, V>,
    registry: &Registry,
    snapshot_of: impl Fn(&S) -> Option<u64>,
) -> Explanation {
    let mut problems = Vec::new();
    let mut findings = Vec::new();
    let mut compared: Compared = BTreeMap::new();
    for finding in &decision.findings {
        let invariant = finding.obligation.invariant.clone();
        let reason = match &finding.status {
            Status::Satisfied(r) | Status::Violated(r) | Status::Undetermined(r) => r,
        };
        let (statement, source) = match &finding.obligation.requirement {
            Requirement::Fact { subject, value, .. } => {
                let statement = format!("{subject:?} = {value:?}");
                let source = fact_source(reason, registry, &mut problems);
                if let Source::Lineage(chain) = &source {
                    let direct = &chain[0].1;
                    if let Some(n) = snapshot_of(subject) {
                        if direct.snapshot != Some(n) {
                            problems.push(format!(
                                "{statement}: evidence for snapshot {n} has lineage L{} from {}",
                                direct.id,
                                direct
                                    .snapshot
                                    .map_or(format!("round {} (no snapshot)", direct.round), |s| {
                                        format!("snapshot {s}")
                                    })
                            ));
                        }
                    }
                    compared
                        .entry((invariant.clone(), format!("{value:?}")))
                        .or_default()
                        .push((statement.clone(), direct.method.definition.clone()));
                }
                (statement, source)
            }
            Requirement::Within { names, scopes } => (
                format!("{} name(s) within {scopes:?}", names.len()),
                Source::LocalCheck,
            ),
            Requirement::AtMost {
                quantity,
                value,
                limit,
            } => (
                format!("{quantity} = {value} <= {limit}"),
                Source::LocalCheck,
            ),
        };
        findings.push(Explained {
            invariant,
            statement,
            status: finding.status.clone(),
            source,
        });
    }
    for ((invariant, value), facts) in compared {
        let mut definitions: Vec<&Option<String>> = facts.iter().map(|(_, d)| d).collect();
        definitions.sort();
        definitions.dedup();
        if facts.len() > 1 && definitions.len() > 1 {
            problems.push(format!(
                "{invariant} compares {value} under different definitions: {}",
                facts
                    .iter()
                    .map(|(s, d)| format!("{s} [{}]", d.as_deref().unwrap_or("undeclared")))
                    .collect::<Vec<_>>()
                    .join(" vs ")
            ));
        }
    }
    Explanation {
        phase: decision.phase,
        verdict: decision.verdict,
        findings,
        problems,
    }
}

/// The source named in a kernel reason ("verified by X (basis)").
fn fact_source(reason: &str, registry: &Registry, problems: &mut Vec<String>) -> Source {
    let Some(start) = reason.find("verified by ") else {
        return Source::None;
    };
    let rest = &reason[start + "verified by ".len()..];
    let (observer, basis) = match rest.split_once(" (") {
        Some((observer, tail)) => (observer, tail.split(')').next().unwrap_or_default()),
        None => (rest, ""),
    };
    let ids = lineage_ids(basis);
    if !observer.starts_with("provider:") {
        return Source::Layer {
            observer: observer.to_string(),
            basis: basis.to_string(),
        };
    }
    let Some(&id) = ids.first() else {
        problems.push(format!(
            "{observer}: verified evidence without lineage ({reason})"
        ));
        return Source::None;
    };
    let mut chain = Vec::new();
    follow(id, 0, registry, &mut chain, problems);
    if chain.is_empty() {
        Source::None
    } else {
        Source::Lineage(chain)
    }
}

fn follow(
    id: u64,
    depth: usize,
    registry: &Registry,
    chain: &mut Vec<(usize, Lineage)>,
    problems: &mut Vec<String>,
) {
    if depth > 16 || chain.iter().any(|(_, l)| l.id == id) {
        return;
    }
    let Some(lineage) = registry.lineage(id) else {
        problems.push(format!("lineage L{id} is not in the registry"));
        return;
    };
    let deps = lineage.depends_on.clone();
    chain.push((depth, lineage));
    for dep in deps {
        follow(dep, depth + 1, registry, chain, problems);
    }
}

impl fmt::Display for Explanation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verdict = match self.verdict {
            Verdict::Allow => "ALLOW",
            Verdict::Deny => "DENY",
            Verdict::Blocked => "BLOCKED",
        };
        writeln!(f, "{verdict} ({:?}) because:", self.phase)?;
        for e in &self.findings {
            let (tag, why) = match &e.status {
                Status::Satisfied(_) => ("ok", None),
                Status::Violated(r) => ("VIOLATED", Some(r)),
                Status::Undetermined(r) => ("UNDETERMINED", Some(r)),
            };
            writeln!(f, "  [{tag}] {}: {}", e.invariant, e.statement)?;
            match &e.source {
                Source::Lineage(chain) => {
                    for (depth, l) in chain {
                        let pad = "    ".repeat(depth + 1);
                        let arrow = if *depth == 0 {
                            "evidence"
                        } else {
                            "└ depends on"
                        };
                        writeln!(f, "{pad}{arrow} L{}: {:?} = {:?}", l.id, l.key, l.value)?;
                        writeln!(
                            f,
                            "{pad}  established: provider {} · request {} · round {} · {}{}",
                            l.provider,
                            l.request,
                            l.round,
                            l.snapshot
                                .map_or("no snapshot".to_string(), |s| format!("snapshot s{s}")),
                            if l.supporting {
                                " · supporting fact"
                            } else {
                                ""
                            }
                        )?;
                        writeln!(
                            f,
                            "{pad}  claimed:     method `{}`{} · observed {}",
                            l.method.name,
                            l.method
                                .definition
                                .as_ref()
                                .map_or(String::new(), |d| format!(" · definition {d}")),
                            if l.observed.is_empty() {
                                "-".to_string()
                            } else {
                                l.observed.join(", ")
                            }
                        )?;
                    }
                }
                Source::Layer { observer, basis } => {
                    writeln!(f, "    established by {observer} ({basis})")?
                }
                Source::LocalCheck => writeln!(f, "    local check of the obligation")?,
                Source::None => {}
            }
            if let Some(why) = why {
                writeln!(f, "    because: {why}")?;
            }
        }
        if self.problems.is_empty() {
            write!(f, "provenance: consistent")
        } else {
            write!(f, "provenance PROBLEMS:")?;
            for p in &self.problems {
                write!(f, "\n  - {p}")?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn provenance_depends_on_no_domain_module() {
        let source = include_str!("provenance.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for line in source
            .lines()
            .filter(|l| l.trim_start().starts_with("use "))
        {
            assert!(
                ["std::", "crate::kernel::", "crate::graph::"]
                    .iter()
                    .any(|allowed| line.contains(allowed)),
                "{line}"
            );
        }
        let code = source
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        for word in [
            "path",
            "file",
            "commit",
            "git",
            "workspace",
            "ci",
            "artifact",
        ] {
            assert!(
                !code
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|t| t == word),
                "provenance code mentions `{word}`"
            );
        }
    }
}
