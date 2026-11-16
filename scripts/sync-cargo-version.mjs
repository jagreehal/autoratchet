// After `changeset version`: give the Rust crate the version npm publishes.
import fs from 'node:fs';

const { version } = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const manifest = 'crates/autoratchet/Cargo.toml';
fs.writeFileSync(
  manifest,
  fs.readFileSync(manifest, 'utf8').replace(/^version = ".*"$/m, `version = "${version}"`),
);
const lock = 'Cargo.lock';
fs.writeFileSync(
  lock,
  fs.readFileSync(lock, 'utf8').replace(/(name = "autoratchet"\nversion = )".*"/, `$1"${version}"`),
);
console.log(`Cargo version ${version}`);
