import { promises as fs } from 'node:fs';
import path from 'node:path';

function parseArgs(argv) {
  const options = {
    version: process.env.CODEX_SENTINEL_VERSION?.trim() || null,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const current = argv[index];

    if (current === '--version') {
      options.version = argv[index + 1]?.trim() || null;
      index += 1;
      continue;
    }

    throw new Error(`Unknown argument: ${current}`);
  }

  return options;
}

function normalizeVersion(version) {
  const normalized = version?.trim().replace(/^v/i, '') || '';

  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(normalized)) {
    throw new Error(`Invalid version: ${version ?? ''}`);
  }

  return normalized;
}

async function updateJsonVersion(filePath, nextVersion, updater = null) {
  const raw = await fs.readFile(filePath, 'utf8');
  const parsed = JSON.parse(raw);
  parsed.version = nextVersion;

  if (typeof updater === 'function') {
    updater(parsed, nextVersion);
  }

  await fs.writeFile(filePath, `${JSON.stringify(parsed, null, 2)}\n`);
}

async function updateCargoTomlVersion(filePath, nextVersion) {
  const raw = await fs.readFile(filePath, 'utf8');
  const newline = raw.includes('\r\n') ? '\r\n' : '\n';
  const lines = raw.split(/\r?\n/);
  let inPackageSection = false;
  let replaced = false;

  const updatedLines = lines.map((line) => {
    const trimmed = line.trim();

    if (trimmed.startsWith('[')) {
      inPackageSection = trimmed === '[package]';
      return line;
    }

    if (inPackageSection && /^\s*version\s*=/.test(line) && !replaced) {
      replaced = true;
      return line.replace(/(\s*version\s*=\s*")([^"]+)(")/, `$1${nextVersion}$3`);
    }

    return line;
  });

  if (!replaced) {
    throw new Error(`Failed to update Cargo.toml version in ${filePath}`);
  }

  await fs.writeFile(filePath, `${updatedLines.join(newline).replace(/\s+$/u, '')}${newline}`);
}

async function main() {
  const { version } = parseArgs(process.argv.slice(2));
  const nextVersion = normalizeVersion(version);
  const root = process.cwd();

  await updateJsonVersion(path.join(root, 'package.json'), nextVersion);
  await updateJsonVersion(path.join(root, 'package-lock.json'), nextVersion, (parsed, normalizedVersion) => {
    if (parsed.packages?.['']) {
      parsed.packages[''].version = normalizedVersion;
    }
  });
  await updateJsonVersion(path.join(root, 'tauri.conf.json'), nextVersion);
  await updateCargoTomlVersion(path.join(root, 'Cargo.toml'), nextVersion);

  console.log(`[set-app-version] Synced app version to ${nextVersion}`);
}

main().catch((error) => {
  console.error(`[set-app-version] ${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
});
