import { chmod, copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export async function syncInstallScript({
  appRoot = resolve(dirname(fileURLToPath(import.meta.url)), ".."),
  repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../.."),
} = {}) {
  const source = resolve(repoRoot, "scripts/install.sh");
  const target = resolve(appRoot, "public/install.sh");

  await mkdir(dirname(target), { recursive: true });
  await copyFile(source, target);
  await chmod(target, 0o755);

  return { source, target };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { source, target } = await syncInstallScript();
  console.log(`Synced ${source} -> ${target}`);
}
