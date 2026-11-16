// The TypeScript implementation behind the conformance interface: the real CLI, plus a
// `measure <dir>` command that prints every function's scores as JSON.
import path from 'node:path';
import { collectFromDirectory } from '../src/collect.js';

const [command, dir] = process.argv.slice(2);
if (command === 'measure') {
  const snapshots = await collectFromDirectory(path.resolve(dir!));
  const functions = [...snapshots.values()];
  console.log(JSON.stringify({ functions, problems: snapshots.problems }, null, 2));
} else {
  await import('../src/cli.js');
}
