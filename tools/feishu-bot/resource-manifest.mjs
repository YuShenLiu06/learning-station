// Build-time only: hash locked JS resources. Never loads app config or secrets.
import { readdirSync, readFileSync, lstatSync, readlinkSync } from 'node:fs';
import { join, relative, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
const root = dirname(fileURLToPath(import.meta.url));
const hash = path => createHash('sha256').update(readFileSync(path)).digest('hex');
const entries = [];
function walk(path) {
  const info = lstatSync(path), name = relative(root, path);
  if (info.isSymbolicLink()) {
    const target = readlinkSync(path);
    if (target.startsWith('/') || target.split('/').includes('..')) throw Error('unsafe_resource_symlink');
    entries.push({ path: name, symlink: target });
  } else if (info.isDirectory()) for (const name of readdirSync(path).sort()) walk(join(path, name));
  else if (info.isFile()) entries.push({ path: name, bytes: info.size, sha256: hash(path) });
  else throw Error('unsupported_resource');
}
for (const name of ['worker.mjs', 'adapter.mjs', 'package.json', 'package-lock.json', 'node_modules']) walk(join(root, name));
const lock = JSON.parse(readFileSync(join(root, 'package-lock.json')));
for (const [path, value] of Object.entries(lock.packages)) {
  if (path && (!value.resolved?.startsWith('https://registry.npmjs.org/') || !value.integrity)) throw Error('unlocked_dependency');
}
process.stdout.write(JSON.stringify({ schemaVersion: 1, entrypoint: 'worker.mjs', nodeVersion: process.version,
  runtimeExecutable: process.execPath, runtimeSha256: hash(process.execPath),
  runtimeCopiedByThisTask: false, sdkVersion: '1.76.0', installScriptsRun: false,
  resources: entries, totalBytes: entries.reduce((sum, entry) => sum + (entry.bytes ?? 0), 0),
  dependencyCount: Object.keys(lock.packages).length - 1 }, null, 2) + '\n');
