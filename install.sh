#!/usr/bin/env bash
set -euo pipefail

repository="${RYN_REPOSITORY:-rynlanguage/Ryn}"
api_url="https://api.github.com/repos/${repository}/releases/latest"
install_dir="${RYN_INSTALL_DIR:-${HOME}/.local/bin}"

fail() {
    printf 'Ryn installer: %s\n' "$*" >&2
    exit 1
}

for command_name in curl python3 tar sha256sum install; do
    command -v "$command_name" >/dev/null 2>&1 || fail "required command not found: $command_name"
done

[[ "$(uname -s)" == Linux ]] || fail 'this release currently provides a Bash installer for Linux only.'
case "$(uname -m)" in
    x86_64|amd64) ;;
    *) fail "Linux $(uname -m) does not have a published Ryn binary yet." ;;
esac

token="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
if [[ -z "$token" ]] && command -v gh >/dev/null 2>&1; then
    token="$(gh auth token --hostname github.com 2>/dev/null || true)"
fi

api_headers=(-H 'Accept: application/vnd.github+json')
asset_headers=(-H 'Accept: application/octet-stream')
if [[ -n "$token" ]]; then
    api_headers+=(-H "Authorization: Bearer $token")
    asset_headers+=(-H "Authorization: Bearer $token")
fi

release_json="$(curl -fsSL "${api_headers[@]}" "$api_url")" || fail 'could not read the latest GitHub release; authenticate with `gh auth login` if the repository is private.'
asset_info="$(REPOSITORY="$repository" RELEASE_JSON="$release_json" python3 - <<'PY'
import json
import os
import re
import sys

repository = os.environ["REPOSITORY"]
release = json.loads(os.environ["RELEASE_JSON"])
tag = release.get("tag_name", "")
match = re.fullmatch(r"v(\d+\.\d+\.\d+)", tag)
if not match:
    sys.exit(f"latest release tag is not a stable Ryn version: {tag!r}")

version = match.group(1)
archive_name = f"ryn-{version}-linux-x86_64.tar.gz"
assets = {asset.get("name"): asset.get("url", "") for asset in release.get("assets", [])}
archive_url = assets.get(archive_name, "")
checksum_url = assets.get(archive_name + ".sha256", "")
prefix = f"https://api.github.com/repos/{repository}/releases/assets/"
if not archive_url.startswith(prefix) or not checksum_url.startswith(prefix):
    sys.exit(f"release {tag} is missing its Linux x86_64 archive or checksum")

print(tag)
print(archive_name)
print(archive_url)
print(checksum_url)
PY
)" || fail 'could not find the Linux x86_64 archive in the latest release.'
mapfile -t release_fields <<< "$asset_info"
tag="${release_fields[0]}"
archive_name="${release_fields[1]}"
archive_url="${release_fields[2]}"
checksum_url="${release_fields[3]}"

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT
archive_path="$work_dir/$archive_name"
checksum_path="$work_dir/$archive_name.sha256"
curl -fsSL "${asset_headers[@]}" "$archive_url" -o "$archive_path" || fail 'could not download the Linux archive.'
curl -fsSL "${asset_headers[@]}" "$checksum_url" -o "$checksum_path" || fail 'could not download the release checksum.'

expected_hash="$(awk 'NR == 1 { print $1 }' "$checksum_path")"
[[ "$expected_hash" =~ ^[[:xdigit:]]{64}$ ]] || fail 'the release checksum file is malformed.'
printf '%s  %s\n' "$expected_hash" "$archive_path" | sha256sum --check --status || fail 'the downloaded archive failed its SHA-256 check.'

version="${tag#v}"
member="ryn-$version-linux-x86_64/ryn"
tar -tzf "$archive_path" | grep -Fxq "$member" || fail 'the release archive does not contain the Ryn executable.'
tar -xOzf "$archive_path" "$member" > "$work_dir/ryn"
chmod 0755 "$work_dir/ryn"
[[ "$("$work_dir/ryn" --version)" == "ryn $version" ]] || fail 'the downloaded executable reported an unexpected version.'

mkdir -p "$install_dir"
install_dir="$(cd "$install_dir" && pwd)"
install -m 0755 "$work_dir/ryn" "$install_dir/ryn.new"
mv -f "$install_dir/ryn.new" "$install_dir/ryn"

if [[ "$install_dir" == "$HOME/.local/bin" ]]; then
    for startup_file in "$HOME/.profile" "$HOME/.bashrc"; do
        if [[ ! -f "$startup_file" ]] || ! grep -Fq '# Ryn CLI PATH' "$startup_file"; then
            cat >> "$startup_file" <<'EOF'

# Ryn CLI PATH
case ":$PATH:" in
    *:"$HOME/.local/bin":*) ;;
    *) export PATH="$HOME/.local/bin:$PATH" ;;
esac
EOF
        fi
    done
fi

printf 'Installed Ryn %s to %s/ryn\n' "$version" "$install_dir"
if [[ ":$PATH:" != *":$install_dir:"* ]]; then
    printf 'Open a new terminal to use ryn from PATH, or run: %s/ryn --version\n' "$install_dir"
else
    printf 'Run: ryn --version\n'
fi
