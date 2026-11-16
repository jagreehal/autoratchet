---
'autoratchet': minor
---

AutoRatchet is now a native binary written in Rust, with the same CLI, JSON report, GitHub Action and agent hooks. It scores cognitive complexity itself, with the same scores as before and no Biome install, and accepts `sonarjs/cognitive-complexity` override comments. It scores methods nested inside functions on their own, gives the same result on every run, accepts TypeScript overloads and `declare` functions, and handles repositories of any size. A new `measure` command prints every function's scores. The JavaScript API is replaced by the CLI's JSON output and the `autoratchet` Rust crate.
