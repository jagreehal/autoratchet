#!/usr/bin/env node
// Runs the AutoRatchet binary for this platform, with the Oxlint version this package depends on.
// Set AUTORATCHET_OXLINT to use another.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const platform = `${process.platform}-${process.arch}`;
const binary = path.join(
  root,
  'binaries',
  platform,
  process.platform === 'win32' ? 'autoratchet.exe' : 'autoratchet',
);

if (!fs.existsSync(binary)) {
  console.error(
    `autoratchet: no binary for ${platform}. Supported: macOS, Linux and Windows on x64 and arm64.`,
  );
  process.exit(4);
}
// npm does not keep the executable bit on files outside "bin".
try {
  fs.accessSync(binary, fs.constants.X_OK);
} catch {
  fs.chmodSync(binary, 0o755);
}

const env = { ...process.env };
if (!env.AUTORATCHET_OXLINT) {
  // npm ships Oxlint as a Node.js module, so the binary runs its script under this Node.js.
  env.AUTORATCHET_OXLINT = path.join(
    path.dirname(require.resolve('oxlint/package.json')),
    'bin/oxlint',
  );
  env.AUTORATCHET_NODE = process.execPath;
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit', env });
if (result.error) {
  console.error(`autoratchet: ${result.error.message}`);
  process.exit(4);
}
process.exit(result.status ?? 4);
