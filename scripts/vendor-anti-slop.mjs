// Vendor the anti-slop Oxlint rules AutoRatchet ratchets, as plain JavaScript the binary embeds.
//
//   node scripts/vendor-anti-slop.mjs <path to an anti-slop checkout>
//
// Types are stripped and `defineRule` (an identity wrapper for Oxlint) is inlined, so the
// plugin needs no dependencies and runs under any Node.js that Oxlint's npm package supports.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { stripTypeScriptTypes } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

// Every generic rule but require-readable-spacing: a whitespace rule needs a fixer, and
// AutoRatchet has none. The opt-in Effect rules stay out too.
const RULES = [
  'no-array-filter-map',
  'no-chained-type-assertions',
  'no-conditional-empty-object-spread',
  'no-known-value-widening',
  'no-module-mocking',
  'no-object-parameters',
  'no-reduce-accumulator-copy',
  'no-reflect-apply',
  'no-reflect-get',
  'no-runtime-typeof',
  'no-shape-in-symbol-names',
  'no-unknown-parameters',
  'no-unknown-returns',
  'no-unknown-type-aliases',
  'no-unsafe-dictionary-type',
  'no-widen-then-assert',
  'require-safety-comment-for-type-assertion',
];

const upstream = process.argv[2];
if (!upstream) throw new Error('usage: vendor-anti-slop.mjs <anti-slop checkout>');
const src = path.join(upstream, 'src');
const out = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
  'crates/autoratchet/plugin',
);
const commit = execFileSync('git', ['-C', upstream, 'rev-parse', 'HEAD'], {
  encoding: 'utf8',
}).trim();

// Local changes, applied to the stripped JavaScript. Each must apply.
const PATCHES = {
  // A promise rejection handler must take `unknown`
  // (typescript/use-unknown-in-catch-callback-variable): `.catch(fn)` or `.then(_, fn)`.
  'rules/no-unknown-parameters.ts': [
    [
      'export const noUnknownParametersRule',
      `function isPromiseRejectionHandler(node) {
  const call = node.parent;
  if (call?.type !== "CallExpression" || call.callee.type !== "MemberExpression") return false;
  const { computed, property } = call.callee;
  if (computed || property.type !== "Identifier") return false;
  const index = call.arguments.indexOf(node);
  return (property.name === "catch" && index === 0) || (property.name === "then" && index === 1);
}

export const noUnknownParametersRule`,
    ],
    [
      'isTypePredicateSubject(node, name)) continue;',
      'isTypePredicateSubject(node, name) || isPromiseRejectionHandler(node)) continue;',
    ],
  ],
};

fs.rmSync(out, { recursive: true, force: true });
const written = new Set();

/** Copy one module and, depth first, every relative module it imports. */
function vendor(rel) {
  if (written.has(rel)) return;
  written.add(rel);
  const ts = fs.readFileSync(path.join(src, rel), 'utf8');
  let js = stripTypeScriptTypes(ts, { mode: 'strip' })
    .replace(
      /^import \{ defineRule \} from "@oxlint\/plugins";$/m,
      'const defineRule = (rule) => rule;',
    )
    .replace(/(from\s+(["'])\.{1,2}\/[^"']+)\.ts\2/g, '$1.js$2');
  for (const [find, replace] of PATCHES[rel] ?? []) {
    if (js.split(find).length !== 2)
      throw new Error(`${rel}: patch target not found once: ${find}`);
    js = js.replace(find, replace);
  }
  if (/@oxlint\/plugins/.test(js))
    throw new Error(`${rel}: runtime import of @oxlint/plugins left`);
  const target = path.join(out, rel.replace(/\.ts$/, '.js'));
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(
    target,
    `// Vendored from anti-slop@${commit} (MIT). Regenerate with scripts/vendor-anti-slop.mjs.\n${js}`,
  );
  for (const [, , dep] of js.matchAll(/from\s+(["'])(\.{1,2}\/[^"']+)\.js\1/g)) {
    vendor(path.normalize(path.join(path.dirname(rel), `${dep}.ts`)));
  }
}

const camel = (name) => name.replace(/-(\w)/g, (_, c) => c.toUpperCase());
const exportName = (rule) => {
  const source = fs.readFileSync(path.join(src, 'rules', `${rule}.ts`), 'utf8');
  return /export const (\w+Rule)\b/.exec(source)?.[1] ?? `${camel(rule)}Rule`;
};
for (const rule of RULES) vendor(`rules/${rule}.ts`);

const imports = RULES.map((r) => `import { ${exportName(r)} } from './rules/${r}.js';`).join('\n');
const entries = RULES.map((r) => `    '${r}': ${exportName(r)},`).join('\n');
fs.writeFileSync(
  path.join(out, 'index.js'),
  `// Vendored from anti-slop@${commit} (MIT). Regenerate with scripts/vendor-anti-slop.mjs.\n${imports}\n\nexport default {\n  meta: { name: 'anti-slop' },\n  rules: {\n${entries}\n  },\n};\n`,
);
fs.copyFileSync(path.join(upstream, 'LICENSE'), path.join(out, 'LICENSE'));
fs.writeFileSync(
  path.join(out, 'UPSTREAM.md'),
  `# Upstream

- Source: anti-slop, by Dillon Mulroy
- Commit: \`${commit}\`
- Rules: ${RULES.map((r) => `\`${r}\``).join(', ')}

## Deviations

- TypeScript types are stripped to plain ES modules, so the plugin runs on any Node.js that
  Oxlint's npm package supports, and formatted with oxfmt.
- \`defineRule\` from \`@oxlint/plugins\` is inlined (it returns the rule unchanged), so the plugin
  needs no dependencies.
- \`no-unknown-parameters\` allows \`unknown\` on a function passed directly as a promise
  rejection handler (\`.catch(fn)\`, or the second argument to \`.then\`), where
  \`typescript/use-unknown-in-catch-callback-variable\` requires it.
- \`index.js\` registers only the rules above. Regenerate everything with
  \`node scripts/vendor-anti-slop.mjs <anti-slop checkout>\`.
`,
);
// ES modules whatever Node.js decides about a bare .js file.
fs.writeFileSync(path.join(out, 'package.json'), '{ "type": "module" }\n');

// The binary embeds every file listed here.
const files = [...written].map((f) => f.replace(/\.ts$/, '.js')).sort();
fs.writeFileSync(
  path.join(out, 'files.rs'),
  `// Generated by scripts/vendor-anti-slop.mjs from anti-slop@${commit}.\n/// Rule names, as Oxlint reports them under the \`anti-slop\` plugin.\npub const RULES: [&str; ${RULES.length}] = [\n${RULES.map((r) => `    "${r}",`).join('\n')}\n];\n/// Plugin files, relative to the plugin directory.\npub const FILES: [(&str, &str); ${files.length + 2}] = [\n    ("index.js", include_str!("index.js")),\n    ("package.json", include_str!("package.json")),\n${files.map((f) => `    ("${f}", include_str!("${f}")),`).join('\n')}\n];\n`,
);
// Stripping leaves blanks where the types were: format, so the vendored code reads like code.
execFileSync(path.join(out, '../../../node_modules/.bin/oxfmt'), [out], { stdio: 'inherit' });
console.log(`vendored ${RULES.length} rules, ${files.length} modules from ${commit}`);
