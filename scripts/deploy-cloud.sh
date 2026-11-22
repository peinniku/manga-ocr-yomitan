#!/usr/bin/env bash
# Deploy the pic_yomi cloud sync hub to your Cloudflare account and point this
# Mac at it. Run `npx wrangler login` (in cloud/) once before the first deploy.
#
#   scripts/deploy-cloud.sh
#
# Free plan only: one Worker + one D1 database. Safe to re-run (updates the
# Worker and web app; keeps the data and the pairing token).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
DATA="${PICYOMI_DATA:-$HOME/Library/Application Support/pic_yomi}"
CLI="$ROOT/target/release/picyomi-cli"

echo "== build web app and CLI"
(cd web && npm run build >/dev/null)
cargo build -q --release -p picyomi-core --bin picyomi-cli
rm -rf cloud/public && mkdir -p cloud/public/app && cp -R web/dist/* cloud/public/app/

cd cloud
[ -d node_modules ] || npm install --silent
W="npx wrangler"

echo "== D1 database"
ID=$($W d1 list --json | node -e 'const l=JSON.parse(require("fs").readFileSync(0,"utf8"));const d=l.find(x=>x.name==="picyomi");process.stdout.write(d?d.uuid:"")')
if [ -z "$ID" ]; then
  $W d1 create picyomi >/dev/null
  ID=$($W d1 list --json | node -e 'const l=JSON.parse(require("fs").readFileSync(0,"utf8"));process.stdout.write(l.find(x=>x.name==="picyomi").uuid)')
fi
# The account's database id goes into a git-ignored copy of the config.
sed -E "s/\"database_id\": \"[^\"]*\"/\"database_id\": \"$ID\"/" wrangler.jsonc > wrangler.local.jsonc
W="$W -c wrangler.local.jsonc"
$W d1 execute picyomi --remote --file schema.sql >/dev/null

echo "== deploy worker"
OUT=$($W deploy 2>&1) || { echo "$OUT"; exit 1; }
URL=$(printf '%s\n' "$OUT" | grep -Eo 'https://[a-z0-9.-]+\.workers\.dev' | head -1)
[ -n "$URL" ] || { echo "$OUT"; echo "could not find the workers.dev URL"; exit 1; }
echo "   $URL"

echo "== pairing token"
"$CLI" cloud-setup "$URL" >/dev/null
# Straight from the local database into the Worker secret; never printed.
sqlite3 "$DATA/picyomi.db" "SELECT value FROM settings WHERE key='cloud.token'" | tr -d '\n' | $W secret put TOKEN >/dev/null
echo "   set"

for i in $(seq 1 20); do
  curl -sf "$URL/health" >/dev/null && break
  sleep 1
done
echo "== done: the Mac app syncs with $URL in the background"
echo "   pair the phone from pic_yomi → 设置 → 手机同步 (QR code)"
