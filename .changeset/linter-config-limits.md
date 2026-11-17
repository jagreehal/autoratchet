---
'autoratchet': minor
---

AutoRatchet takes complexity limits from your linter config: `.oxlintrc.json`, `biome.json`, Ruff, `clippy.toml` and golangci-lint. Each language gets its own limit, and the report names the file that set it. The action's `threshold` and `cognitive-threshold` inputs now default to your config.
