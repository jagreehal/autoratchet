---
'autoratchet': minor
---

AutoRatchet measures Python and Go. Python scores follow radon for cyclomatic complexity and complexipy for cognitive complexity; Go scores follow gocyclo and gocognit. Each language reads the override comments its developers write today (`# noqa: C901`, `# complexipy: ignore`, `//nolint:gocyclo,gocognit`, `//gocognit:ignore`), and both run from the AutoRatchet binary alone.
