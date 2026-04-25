const https = require('https');
const fs = require('fs');
const os = require('os');
const path = require('path');

const version = require('./package.json').version;
const platform = os.platform();
const arch = os.arch();

const targets = {
  'darwin,x64': 'x86_64-apple-darwin',
  'darwin,arm64': 'aarch64-apple-darwin',
  'linux,x64': 'x86_64-unknown-linux-gnu',
  'linux,arm64': 'aarch64-unknown-linux-gnu',
};

const key = `${platform},${arch}`;
const target = targets[key];

if (!target) {
  console.error(`Unsupported platform: ${platform} ${arch}`);
  process.exit(1);
}

const url = `https://github.com/hgwk/cduo/releases/download/v${version}/cduo-${target}`;
const binDir = path.join(__dirname, 'bin');
const binPath = path.join(binDir, 'cduo');

if (!fs.existsSync(binDir)) {
  fs.mkdirSync(binDir, { recursive: true });
}

console.log(`Downloading cduo v${version} for ${target}...`);

const file = fs.createWriteStream(binPath);
https.get(url, (response) => {
  if (response.statusCode === 302 || response.statusCode === 301) {
    https.get(response.headers.location, (redirect) => {
      redirect.pipe(file);
      file.on('finish', () => {
        file.close();
        fs.chmodSync(binPath, 0o755);
        console.log('cduo installed successfully.');
      });
    });
  } else {
    response.pipe(file);
    file.on('finish', () => {
      file.close();
      fs.chmodSync(binPath, 0o755);
      console.log('cduo installed successfully.');
    });
  }
}).on('error', (err) => {
  fs.unlinkSync(binPath);
  console.error(`Download failed: ${err.message}`);
  process.exit(1);
});
