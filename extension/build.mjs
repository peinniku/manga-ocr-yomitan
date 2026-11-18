// Assemble the extension for Chrome and Firefox/Zen: dist/chrome, dist/firefox.
//   node extension/build.mjs
import { execFileSync } from 'node:child_process';
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const version = readFileSync(join(here, '../Cargo.toml'), 'utf8').match(/^version = "(.+)"/m)[1];

const base = {
  manifest_version: 3,
  name: 'pic_yomi',
  version,
  description: '按住 Shift 识别网页图片中的日语，查词、中文解释、一键加入生词本（配合 pic_yomi 桌面程序）。',
  permissions: ['storage', 'activeTab'],
  host_permissions: ['<all_urls>'],
  action: { default_title: 'pic_yomi', default_popup: 'popup.html' },
  content_scripts: [{ matches: ['<all_urls>'], js: ['content.js'], run_at: 'document_idle', all_frames: false }],
};

const targets = {
  chrome: { ...base, background: { service_worker: 'background.js' } },
  firefox: {
    ...base,
    background: { scripts: ['background.js'] },
    browser_specific_settings: { gecko: { id: 'picyomi@local', strict_min_version: '121.0' } },
  },
};

for (const [name, manifest] of Object.entries(targets)) {
  const out = join(here, 'dist', name);
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  cpSync(join(here, 'src'), out, { recursive: true });
  writeFileSync(join(out, 'manifest.json'), JSON.stringify(manifest, null, 2));
  console.log(`built ${out}`);
}

// Installable package for Zen / Firefox (about:addons → Install Add-on From File).
const xpi = join(here, 'dist', 'picyomi-firefox.xpi');
rmSync(xpi, { force: true });
execFileSync('zip', ['-q', '-r', '-X', xpi, '.'], { cwd: join(here, 'dist', 'firefox') });
console.log(`built ${xpi}`);
