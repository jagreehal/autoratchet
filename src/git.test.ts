import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { verdict } from './compare.js';
import { check } from './git.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
let repo: string;

function sh(...args: string[]) {
  execFileSync('git', args, { cwd: repo, stdio: 'pipe' });
}

function commitBase() {
  repo = fs.mkdtempSync(path.join(os.tmpdir(), 'autoratchet-git-'));
  fs.mkdirSync(path.join(repo, 'src'));
  fs.copyFileSync(
    path.join(root, 'test/fixtures/base/payments.ts'),
    path.join(repo, 'src/payments.ts'),
  );
  fs.writeFileSync(path.join(repo, '.gitignore'), 'generated/\n');
  sh('init', '-q', '-b', 'main');
  sh('add', '-A');
  sh('-c', 'user.email=t@t', '-c', 'user.name=t', 'commit', '-qm', 'base');
}

afterEach(() => fs.rmSync(repo, { recursive: true, force: true }));

describe('check', () => {
  it('compares the working tree, uncommitted changes included, with the base', async () => {
    commitBase();
    fs.copyFileSync(
      path.join(root, 'test/fixtures/head-fail/payments.ts'),
      path.join(repo, 'src/payments.ts'),
    );
    fs.copyFileSync(
      path.join(root, 'test/fixtures/head-fail/refunds.ts'),
      path.join(repo, 'src/refunds.ts'),
    );
    const result = await check({ cwd: repo, base: 'main' });
    expect(verdict(result)).toBe('fail');
    // Oxlint lints files in parallel, so sort before comparing.
    expect(
      result.regressions.map((r) => `${r.filePath}::${r.functionName}:${r.metric}`).sort(),
    ).toEqual([
      'src/payments.ts::processPayment:cognitive',
      'src/payments.ts::processPayment:cyclomatic',
      'src/payments.ts::validateCustomer:cyclomatic',
      'src/refunds.ts::createRefund:cognitive',
      'src/refunds.ts::createRefund:cyclomatic',
    ]);
  });

  it('skips git-ignored files and honours pathspecs', async () => {
    commitBase();
    fs.mkdirSync(path.join(repo, 'generated'));
    fs.copyFileSync(
      path.join(root, 'test/fixtures/head-fail/refunds.ts'),
      path.join(repo, 'generated/refunds.ts'),
    );
    fs.mkdirSync(path.join(repo, 'other'));
    fs.copyFileSync(
      path.join(root, 'test/fixtures/head-fail/refunds.ts'),
      path.join(repo, 'other/refunds.ts'),
    );
    expect(verdict(await check({ cwd: repo, base: 'main', paths: ['src'] }))).toBe('pass');
    expect(verdict(await check({ cwd: repo, base: 'main' }))).toBe('fail'); // other/ is not ignored
    expect(verdict(await check({ cwd: repo, base: 'main', paths: ['.', ':!other'] }))).toBe('pass');
  });
});
