# v9r research

v9r is a state transition verifier. An actor proposes a change; v9r
accepts the resulting state only if every declared rule is satisfied by
attestations its registered observers gave to v9r's own requests.
v9r verifies states, not histories: it does not establish who produced
a state, that its observers are independent of what they judge, or that
a program is correct.

## Reading order

1. [thesis](thesis.md): why v9r exists, the hypothesis tested, and
   what was established.
2. [verification-boundary](verification-boundary.md): exactly what v9r
   claims, its conditions and its trusted computing base.
3. [observer-boundary](observer-boundary.md): where the claim breaks.
   The X1/X4 measurements and observer independence.
4. [causality](causality.md): why v9r verifies states, not histories.
5. [archive-boundary](archive-boundary.md): what happened to the old
   runtime and product architecture.
6. [release](release.md): how to reproduce and review
   `v9r-review-v1.1`.

## Archive

[archive/](archive/README.md) holds the historical process records:
earlier experiments, design notes, audits, checklists and release
records. They are kept as written and are not part of the current
claim.
