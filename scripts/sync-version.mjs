// After `changeset version`: give the Rust crate and the README's workflow example the version npm publishes.
import fs from 'node:fs';

const { version } = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const update = (file, pattern, replacement) =>
  fs.writeFileSync(file, fs.readFileSync(file, 'utf8').replace(pattern, replacement));

update('crates/autoratchet/Cargo.toml', /^version = ".*"$/m, `version = "${version}"`);
update('Cargo.lock', /(name = "autoratchet"\nversion = )".*"/, `$1"${version}"`);
update('README.md', /jagreehal\/autoratchet@v[0-9][^\s]*/g, `jagreehal/autoratchet@v${version}`);
console.log(`Synced version ${version}`);
