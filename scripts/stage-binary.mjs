// Copy the local release build into binaries/<platform>-<arch>/, where bin/autoratchet.js looks.
import fs from 'node:fs';
import path from 'node:path';

const exe = process.platform === 'win32' ? 'autoratchet.exe' : 'autoratchet';
const dest = path.join('binaries', `${process.platform}-${process.arch}`);
fs.mkdirSync(dest, { recursive: true });
fs.copyFileSync(path.join('target', 'release', exe), path.join(dest, exe));
fs.chmodSync(path.join(dest, exe), 0o755);
console.log(`staged ${path.join(dest, exe)}`);
