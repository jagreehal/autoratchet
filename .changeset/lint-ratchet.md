---
'autoratchet': minor
---

Ratchet lint findings alongside complexity. New functions get no findings, and existing functions may not gain any.

- JavaScript and TypeScript: 17 anti-slop rules and 87 of Oxlint's built-in rules. These need Node.js and Oxlint's npm package, which the GitHub Action installs.
- Rust, Python and Go: `anti-slop-rust`, `anti-slop-python` and `anti-slop-go`, built into the binary.
- Approve a finding with `oxlint-disable-next-line <rule> -- reason`, or `autoratchet-disable-next-line` in Rust, Python and Go.
- AutoRatchet matches functions that share a name by their code, so you can add, remove or reorder callbacks and each keeps its history.
