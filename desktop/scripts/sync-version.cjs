// Version single-source-of-truth sync (Sprint C C0-3).
// Source: desktop/src-tauri/tauri.conf.json -> version
// Targets: desktop/package.json, desktop/package-lock.json (root + packages.""),
//          desktop/src-tauri/Cargo.toml ([package] version)
// Usage:
//   node sync-version.cjs            # sync all files to tauri.conf.json version
//   node sync-version.cjs 0.2.5      # bump tauri.conf.json first, then sync all
// Idempotent; exits 1 when the source version cannot be parsed.
// NOTE: ASCII-only (PowerShell 5.1 GBK pitfall on Windows dev machines).
const fs = require('fs');
const path = require('path');

const desktop = path.resolve(__dirname, '..');
const files = {
  tauriConf: path.join(desktop, 'src-tauri', 'tauri.conf.json'),
  pkg: path.join(desktop, 'package.json'),
  lock: path.join(desktop, 'package-lock.json'),
  cargo: path.join(desktop, 'src-tauri', 'Cargo.toml'),
};

function readJson(p) { return JSON.parse(fs.readFileSync(p, 'utf8')); }
function writeJson(p, o) { fs.writeFileSync(p, JSON.stringify(o, null, 2) + '\n'); }

// 1. Bump the source first when a new version is given
const next = process.argv[2];
if (next) {
  const conf = readJson(files.tauriConf);
  conf.version = next;
  writeJson(files.tauriConf, conf);
}
const version = readJson(files.tauriConf).version;
if (!/^\d+\.\d+\.\d+(-[\w.]+)?$/.test(version)) {
  console.error('bad version in tauri.conf.json: ' + version);
  process.exit(1);
}

// 2. package.json
const pkg = readJson(files.pkg);
if (pkg.version !== version) {
  console.log('package.json: ' + pkg.version + ' -> ' + version);
  pkg.version = version;
  writeJson(files.pkg, pkg);
} else {
  console.log('package.json: ok (' + version + ')');
}

// 3. package-lock.json (2 slots: root + packages[""])
const lock = readJson(files.lock);
let lockTouched = false;
if (lock.version !== version) { lock.version = version; lockTouched = true; }
if (lock.packages && lock.packages[''] && lock.packages[''].version !== version) {
  lock.packages[''].version = version;
  lockTouched = true;
}
if (lockTouched) {
  writeJson(files.lock, lock);
  console.log('package-lock.json: synced (2 slots) -> ' + version);
} else {
  console.log('package-lock.json: ok (' + version + ')');
}

// 4. src-tauri/Cargo.toml (first line-start version key = [package] version;
//    dependency versions live inside inline tables, never at line start)
let cargo = fs.readFileSync(files.cargo, 'utf8');
const re = /^version\s*=\s*"[^"]*"/m;
const m = cargo.match(re);
if (!m) {
  console.error('Cargo.toml: no [package] version line found');
  process.exit(1);
}
if (m[0] !== 'version = "' + version + '"') {
  cargo = cargo.replace(re, 'version = "' + version + '"');
  fs.writeFileSync(files.cargo, cargo);
  console.log('Cargo.toml: -> ' + version);
} else {
  console.log('Cargo.toml: ok (' + version + ')');
}
console.log('VERSION_SYNCED=' + version);
