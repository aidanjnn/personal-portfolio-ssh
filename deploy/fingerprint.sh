#!/usr/bin/env bash
# Print the SHA256 fingerprint of the running portfolio's SSH host key, for
# publishing on the website and in the HOST_KEY_FINGERPRINT repo variable.
#
# Run on the VM:  /opt/portfolio/fingerprint.sh
#
# Reads only the public half: the private key never leaves the container.
# Cross-checks it against what the listener actually serves on :2222.

set -euo pipefail

DIR=${PORTFOLIO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)}
KEY=/data/ssh_host_ed25519_key
# Names visitors use; port 2222 entries need the [host]:port form.
NAMES=${PUBLIC_NAMES:-aidanjeon.com,ssh.aidanjeon.com,[aidanjeon.com]:2222,[ssh.aidanjeon.com]:2222}

compose() { docker compose -f "${DIR}/compose.prod.yaml" "$@"; }

# "ssh-ed25519 AAAA..." without any comment.
pub=$(compose exec -T portfolio ssh-keygen -y -f "${KEY}" | awk '{print $1" "$2}')
[[ ${pub} == ssh-ed25519\ * ]] || { echo "error: no ed25519 key at ${KEY}" >&2; exit 1; }

fp=$(printf '%s\n' "${pub}" | compose exec -T portfolio ssh-keygen -lf - | awk '{print $2}')

if command -v ssh-keyscan >/dev/null; then
  served=$(ssh-keyscan -T 5 -t ed25519 -p 2222 127.0.0.1 2>/dev/null | awk '{print $2" "$3}')
  if [[ ${served} != "${pub}" ]]; then
    echo "error: the key served on 127.0.0.1:2222 does not match ${KEY}" >&2
    exit 1
  fi
fi

cat <<EOF
Fingerprint (publish this):
  ${fp} (ED25519)

known_hosts line (optional, lets visitors pre-trust the key):
  ${NAMES} ${pub}
EOF
