// Build the shared Rust FSRS scheduler (crates/srs) to WebAssembly and copy it into public/.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '../..');
const cargo = existsSync(join(homedir(), '.cargo/bin/cargo')) ? join(homedir(), '.cargo/bin/cargo') : 'cargo';
execFileSync(cargo, ['build', '--release', '-p', 'picyomi-srs', '--target', 'wasm32-unknown-unknown'], { cwd: root, stdio: 'inherit' });
copyFileSync(join(root, 'target/wasm32-unknown-unknown/release/picyomi_srs.wasm'), join(root, 'web/public/srs.wasm'));
console.log('public/srs.wasm updated');
