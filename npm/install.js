const https = require('https');
const fs = require('fs');
const os = require('os');
const path = require('path');

const version = require('./package.json').version;
const platform = os.platform();
const arch = os.arch();
const isTty = process.stdout.isTTY && !process.env.CI;
const useColor = isTty && !process.env.NO_COLOR;

const color = {
  dim: (text) => useColor ? `\x1b[2m${text}\x1b[0m` : text,
  cyan: (text) => useColor ? `\x1b[36m${text}\x1b[0m` : text,
  green: (text) => useColor ? `\x1b[32m${text}\x1b[0m` : text,
  red: (text) => useColor ? `\x1b[31m${text}\x1b[0m` : text,
};

const targets = {
  'darwin,x64': 'x86_64-apple-darwin',
  'darwin,arm64': 'aarch64-apple-darwin',
  'linux,x64': 'x86_64-unknown-linux-gnu',
  'linux,arm64': 'aarch64-unknown-linux-gnu',
};

const key = `${platform},${arch}`;
const target = targets[key];
const url = target
  ? `https://github.com/hgwk/cduo/releases/download/v${version}/cduo-${target}`
  : null;
const binDir = path.join(__dirname, 'bin');
const binPath = path.join(binDir, 'cduo');
let file = null;

function log(line = '') {
  process.stdout.write(`${line}\n`);
}

function banner() {
  log('');
  log(`        ${color.cyan('cduo')} ${color.dim(`v${version}`)}`);
  log(color.dim('  Claude Code <-> OpenAI Codex'));
  log(color.dim('  native pair runtime'));
  log('');
}

function step(index, label) {
  log(`${color.cyan(`[${index}/3]`)} ${label}`);
}

function updateProgress(downloaded, total) {
  if (!isTty || !total) {
    return;
  }

  const width = 28;
  const ratio = Math.min(downloaded / total, 1);
  const filled = Math.round(ratio * width);
  const bar = `${'='.repeat(filled)}${'-'.repeat(width - filled)}`;
  const percent = String(Math.round(ratio * 100)).padStart(3, ' ');
  const mb = (downloaded / 1024 / 1024).toFixed(1);
  process.stdout.write(`\r    ${color.dim('fetch')} [${bar}] ${percent}% ${mb} MB`);
}

function finishProgress() {
  if (isTty) {
    process.stdout.write('\n');
  }
}

function cleanupPartial() {
  if (file) {
    file.destroy();
  }
  if (fs.existsSync(binPath)) {
    fs.unlinkSync(binPath);
  }
}

function fail(err) {
  cleanupPartial();
  finishProgress();
  console.error(color.red(`Download failed: ${err.message}`));
  process.exit(1);
}

function completeInstall() {
  file.close(() => {
    finishProgress();
    fs.chmodSync(binPath, 0o755);
    step(3, `ready ${color.green('cduo')}`);
    log('');
    log(`${color.green('Installed.')} Try: ${color.cyan('cduo claude codex')}`);
    log(color.dim('Switch: Ctrl-W    Scroll: PageUp/PageDown    Quit: Ctrl-Q'));
    log(color.dim('Drag inside one pane to copy text.'));
    log('');
  });
}

function download(downloadUrl, redirects = 0) {
  if (redirects > 5) {
    fail(new Error('Too many redirects'));
    return;
  }

  https.get(downloadUrl, (response) => {
    if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
      const location = response.headers.location;
      if (!location) {
        fail(new Error('Redirect response did not include a location header'));
        return;
      }
      download(location, redirects + 1);
      return;
    }

    if (response.statusCode !== 200) {
      fail(new Error(`GitHub release returned HTTP ${response.statusCode}`));
      return;
    }

    const total = Number(response.headers['content-length'] || 0);
    let downloaded = 0;
    file = fs.createWriteStream(binPath);

    response.on('data', (chunk) => {
      downloaded += chunk.length;
      updateProgress(downloaded, total);
    });
    response.on('error', fail);
    file.on('error', fail);
    file.on('finish', completeInstall);
    response.pipe(file);
  }).on('error', fail);
}

banner();
step(1, `platform ${color.green(platform)} ${color.green(arch)}`);

if (!target) {
  console.error(color.red(`Unsupported platform: ${platform} ${arch}`));
  process.exit(1);
}

if (!fs.existsSync(binDir)) {
  fs.mkdirSync(binDir, { recursive: true });
}

step(2, `download ${color.green(target)}`);
download(url);
