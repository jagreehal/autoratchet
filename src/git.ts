import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { compareSources, type CompareResult } from './compare.js';
import type { GateOptions } from './gate.js';
import { isSourceFile } from './overrides.js';

export type CheckOptions = GateOptions & {
  /** Branch, tag or commit to compare against. The comparison uses its merge base with HEAD. */
  base?: string;
  /** Git pathspecs to measure, such as `src` or `':!src/generated'`. Default: the whole repo. */
  paths?: string[];
  /** Any directory inside the repository. Default: the current directory. */
  cwd?: string;
};

export type CheckResult = CompareResult & {
  /** The commit the working tree was compared with. */
  baseCommit: string;
};

function git(cwd: string, args: string[]): string {
  return execFileSync('git', args, { cwd, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
}

function nulList(output: string): string[] {
  return output.split('\0').filter(Boolean);
}

/**
 * Compare the working tree with `merge-base(base, HEAD)`. Uncommitted changes count,
 * files git ignores do not, and only JavaScript and TypeScript sources are measured.
 */
export async function check(options: CheckOptions = {}): Promise<CheckResult> {
  const cwd = path.resolve(options.cwd ?? process.cwd());
  const root = git(cwd, ['rev-parse', '--show-toplevel']).trim();
  const base = options.base ?? 'main';
  const baseCommit = git(root, ['merge-base', base, 'HEAD']).trim();
  const pathspec = ['--', ...(options.paths?.length ? options.paths : ['.'])];

  const headFiles = nulList(
    git(root, ['ls-files', '-z', '--cached', '--others', '--exclude-standard', ...pathspec]),
  ).filter((f) => isSourceFile(f) && fs.existsSync(path.join(root, f)));
  const baseFiles = nulList(
    git(root, ['ls-tree', '-r', '-z', '--name-only', baseCommit, ...pathspec]),
  ).filter(isSourceFile);

  const baseRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'autoratchet-base-'));
  try {
    if (baseFiles.length > 0) {
      // `git archive` writes the base commit's copies without touching the working tree.
      const archive = spawnSync(
        'git',
        ['archive', '--format=tar', baseCommit, '--', ...baseFiles],
        {
          cwd: root,
          maxBuffer: 1024 * 1024 * 1024,
        },
      );
      if (archive.status !== 0) throw new Error(`git archive failed: ${archive.stderr.toString()}`);
      const untar = spawnSync('tar', ['-x', '-C', baseRoot], { input: archive.stdout });
      if (untar.status !== 0) throw new Error(`tar failed: ${untar.stderr.toString()}`);
    }
    const result = await compareSources(
      { root: baseRoot, files: baseFiles.map((f) => path.join(baseRoot, f)) },
      { root, files: headFiles.map((f) => path.join(root, f)) },
      options,
    );
    return { ...result, baseCommit };
  } finally {
    fs.rmSync(baseRoot, { recursive: true, force: true });
  }
}
