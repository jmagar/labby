import { mkdir, mkdtemp, readFile, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import assert from "node:assert/strict";

import { syncInstallScript } from "./sync-install-script.mjs";

test("syncInstallScript copies the repo installer into public assets", async () => {
  const root = await mkdtemp(join(tmpdir(), "labby-install-sync-"));
  const repoRoot = join(root, "repo");
  const appRoot = join(repoRoot, "apps/gateway-admin");
  const installer = "#!/usr/bin/env sh\nset -eu\necho labby\n";

  await mkdir(join(repoRoot, "scripts"), { recursive: true });
  await writeFile(join(repoRoot, "scripts/install.sh"), installer, {
    mode: 0o755,
  });

  const { target } = await syncInstallScript({ appRoot, repoRoot });

  assert.equal(await readFile(target, "utf8"), installer);
  assert.equal((await stat(target)).mode & 0o111, 0o111);
});

test("checked-in public installer matches the repo installer", async () => {
  const appRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
  const repoRoot = resolve(appRoot, "../..");
  const source = await readFile(join(repoRoot, "scripts/install.sh"), "utf8");
  const publicCopy = await readFile(join(appRoot, "public/install.sh"), "utf8");

  assert.equal(publicCopy, source);
});
