// Create a GitHub release for a given tag and upload the NSIS installer.
// Usage: node gh-release.cjs <version> <notes.md> [--prerelease]
// Example: node gh-release.cjs 0.2.3 C:\\dev\\release-v023.md
//          node gh-release.cjs 0.2.4 C:\\dev\\release-v024.md --prerelease
// Token from GH_TOKEN env only; never logged.
const fs = require('fs');
const https = require('https');

const VERSION = process.argv[2];
const NOTES = process.argv[3];
const PRERELEASE = process.argv.includes('--prerelease');
const TOKEN = process.env.GH_TOKEN;
const REPO = 'yuanzimu/maestro';
const TAG = 'v' + VERSION;
const BUNDLE_DIR = 'C:\\cargo-target\\maestro-desktop\\release\\bundle\\nsis';
const ASSET = BUNDLE_DIR + '\\Maestro_' + VERSION + '_arm64-setup.exe';
const ASSET_NAME = 'Maestro_' + VERSION + '_arm64-setup.exe';

if (!VERSION || !NOTES || !TOKEN) {
  console.error('usage: node gh-release.cjs <version> <notes.md> [--prerelease]  (GH_TOKEN env required)');
  process.exit(1);
}

function api(host, method, path, body, headers) {
  return new Promise((resolve, reject) => {
    const data = body ? (Buffer.isBuffer(body) ? body : Buffer.from(body)) : null;
    const opts = {
      host, method, path,
      headers: {
        authorization: 'Bearer ' + TOKEN,
        accept: 'application/vnd.github+json',
        'user-agent': 'maestro-release-script',
        ...(data ? { 'content-length': data.length } : {}),
        ...headers,
      },
    };
    const req = https.request(opts, res => {
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => {
        const d = Buffer.concat(chunks).toString('utf8');
        let json = null;
        try { json = d ? JSON.parse(d) : null; } catch {}
        if (res.statusCode >= 200 && res.statusCode < 300) resolve(json || { status: res.statusCode });
        else reject(new Error(res.statusCode + ' ' + d.slice(0, 400)));
      });
    });
    req.on('error', reject);
    if (data) req.write(data);
    req.end();
  });
}

(async () => {
  const notes = fs.readFileSync(NOTES, 'utf8');
  console.log('releasing', TAG, PRERELEASE ? '(prerelease)' : '(stable)');
  const body = JSON.stringify({
    tag_name: TAG,
    name: 'Maestro v' + VERSION + (PRERELEASE ? ' (pre-release)' : ' - Windows desktop client'),
    body: notes,
    draft: false,
    prerelease: PRERELEASE,
  });

  // If a release already exists for the tag, update it instead of failing.
  let release;
  try {
    release = await api('api.github.com', 'POST', `/repos/${REPO}/releases`, body);
    console.log('release created:', release.id, release.html_url);
  } catch (e) {
    if (/already_exists|422/.test(e.message)) {
      const existing = await api('api.github.com', 'GET', `/repos/${REPO}/releases/tags/${TAG}`);
      release = await api('api.github.com', 'PATCH', `/repos/${REPO}/releases/${existing.id}`, body);
      console.log('release updated:', release.id, release.html_url);
    } else throw e;
  }

  // Delete same-named asset if re-running, then upload.
  const stat = fs.statSync(ASSET);
  const dup = (release.assets || []).find(a => a.name === ASSET_NAME);
  if (dup) {
    await api('api.github.com', 'DELETE', `/repos/${REPO}/releases/assets/${dup.id}`);
    console.log('old asset deleted');
  }
  const payload = fs.readFileSync(ASSET);
  const up = await api(
    'uploads.github.com',
    'POST',
    `/repos/${REPO}/releases/${release.id}/assets?name=${encodeURIComponent(ASSET_NAME)}`,
    payload,
    { 'content-type': 'application/octet-stream' }
  );
  console.log('asset uploaded:', up.name, up.size, 'bytes (disk:', stat.size + ')');
  console.log('state:', up.state);
  console.log('RELEASE_URL:', release.html_url);
})().catch(e => { console.error('FAIL:', e.message); process.exit(1); });
