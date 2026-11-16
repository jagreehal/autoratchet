// Before publishing: every supported platform has its binary, so no user gets a package without one.
import fs from 'node:fs';
import path from 'node:path';

const platforms = [
  'darwin-arm64',
  'darwin-x64',
  'linux-arm64',
  'linux-x64',
  'win32-arm64',
  'win32-x64',
];
const missing = platforms.filter((p) => {
  const exe = p.startsWith('win32') ? 'autoratchet.exe' : 'autoratchet';
  return !fs.existsSync(path.join('binaries', p, exe));
});
if (missing.length > 0) {
  console.error(`Missing binaries: ${missing.join(', ')}`);
  process.exit(1);
}
console.log(`All ${platforms.length} binaries present.`);
