# Upstream

- Source: anti-slop, by Dillon Mulroy
- Commit: `13909c9e07590f5a85d93c62712259fe14a883f7`
- Rules: `no-array-filter-map`, `no-chained-type-assertions`, `no-conditional-empty-object-spread`, `no-known-value-widening`, `no-module-mocking`, `no-object-parameters`, `no-reduce-accumulator-copy`, `no-reflect-apply`, `no-reflect-get`, `no-runtime-typeof`, `no-shape-in-symbol-names`, `no-unknown-parameters`, `no-unknown-returns`, `no-unknown-type-aliases`, `no-unsafe-dictionary-type`, `no-widen-then-assert`, `require-safety-comment-for-type-assertion`

## Deviations

- TypeScript types are stripped to plain ES modules, so the plugin runs on any Node.js that
  Oxlint's npm package supports, and formatted with oxfmt.
- `defineRule` from `@oxlint/plugins` is inlined (it returns the rule unchanged), so the plugin
  needs no dependencies.
- `no-unknown-parameters` allows `unknown` on a function passed directly as a promise
  rejection handler (`.catch(fn)`, or the second argument to `.then`), where
  `typescript/use-unknown-in-catch-callback-variable` requires it.
- `index.js` registers only the rules above. Regenerate everything with
  `node scripts/vendor-anti-slop.mjs <anti-slop checkout>`.
