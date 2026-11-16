#!/usr/bin/env bash
# Unattended, re-runnable setup for the portfolio VM (Ubuntu 24.04, amd64).
#
# deploy/azure.sh runs it as root through `az vm run-command invoke`, with the
# other deploy/ files next to it. There is no public admin SSH at any point.
# By hand, as root on the VM, with the whole deploy/ directory:
#
#   TS_AUTHKEY=tskey-auth-... bash deploy/bootstrap.sh
#
# Environment:
#   TS_AUTHKEY   One-off Tailscale auth key. Needed only until the VM has
#                joined the tailnet. Never printed or written to disk here.
#   TS_HOSTNAME  Tailnet name (default portfolio).
#   TS_TAGS      Tags to advertise (default tag:portfolio).
#   TAG          Image tag (default: the tag already running, else latest).
#   START_STACK  1 (default) starts the stack. 0 installs everything but
#                leaves it stopped, e.g. to restore a host key backup first.
#
# It installs Docker Engine and Tailscale, joins the tailnet with Tailscale
# SSH, disables OpenSSH so the portfolio can own port 22, creates the `deploy`
# user for CI, installs the stack into /opt/portfolio and starts it.
# On success the last line is `BOOTSTRAP_OK stack=started|held`.

set -euo pipefail

readonly APP_DIR=/opt/portfolio
readonly DEPLOY_USER=deploy
readonly IMAGE=ghcr.io/aidanjnn/personal-portfolio-ssh
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly SCRIPT_DIR

TS_HOSTNAME=${TS_HOSTNAME:-portfolio}
TS_TAGS=${TS_TAGS:-tag:portfolio}
START_STACK=${START_STACK:-1}

# run-command starts scripts with a minimal environment, and the caller's
# umask must not make apt sources or keyrings unreadable.
umask 022
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
export HOME=${HOME:-/root}
export DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=a

log() { printf '\n==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
apt_get() { apt-get -o DPkg::Lock::Timeout=300 -y -q "$@"; }
compose() { docker compose -f "${APP_DIR}/compose.prod.yaml" "$@"; }

# --- Preflight ----------------------------------------------------------------

[[ ${EUID} -eq 0 ]] || die "run as root"
[[ ${START_STACK} == 0 || ${START_STACK} == 1 ]] || die "START_STACK must be 0 or 1"

# shellcheck source=/dev/null
. /etc/os-release
[[ ${ID} == ubuntu ]] || die "expected Ubuntu, found ${ID}"
CODENAME=${UBUNTU_CODENAME:-${VERSION_CODENAME}}
[[ ${CODENAME} == noble ]] || printf 'warning: tested on Ubuntu 24.04 (noble), found %s\n' "${CODENAME}" >&2

# The image is built for linux/amd64 only.
[[ $(dpkg --print-architecture) == amd64 ]] || die "expected an amd64 host"

for f in compose.prod.yaml Caddyfile fingerprint.sh; do
  [[ -f ${SCRIPT_DIR}/${f} ]] || die "missing ${SCRIPT_DIR}/${f}; copy the whole deploy/ directory"
done

# On first boot cloud-init may still be configuring the system.
if command -v cloud-init >/dev/null; then
  log "Waiting for cloud-init"
  cloud-init status --wait >/dev/null || true
fi

log "Installing base packages"
apt_get update
apt_get install ca-certificates curl

install -m 0755 -d /etc/apt/keyrings

# --- Docker Engine from Docker's apt repository --------------------------------
# Ubuntu's own docker.io/compose packages lag behind Docker's releases.

log "Adding the Docker repository"
for pkg in docker.io docker-doc docker-compose docker-compose-v2 podman-docker containerd runc; do
  # Distro packages conflict with docker-ce; a fresh cloud image has none.
  # shellcheck disable=SC2016 # ${Status} is a dpkg-query field, not a shell variable
  if [[ $(dpkg-query -W -f='${Status}' "${pkg}" 2>/dev/null || true) == "install ok installed" ]]; then
    apt_get remove "${pkg}"
  fi
done

if [[ ! -s /etc/apt/keyrings/docker.asc ]]; then
  curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
  chmod a+r /etc/apt/keyrings/docker.asc
fi
cat > /etc/apt/sources.list.d/docker.sources <<EOF
Types: deb
URIs: https://download.docker.com/linux/ubuntu
Suites: ${CODENAME}
Components: stable
Signed-By: /etc/apt/keyrings/docker.asc
EOF

# --- Tailscale from its apt repository -----------------------------------------

log "Adding the Tailscale repository"
if [[ ! -s /usr/share/keyrings/tailscale-archive-keyring.gpg ]]; then
  curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODENAME}.noarmor.gpg" \
    -o /usr/share/keyrings/tailscale-archive-keyring.gpg
fi
curl -fsSL "https://pkgs.tailscale.com/stable/ubuntu/${CODENAME}.tailscale-keyring.list" \
  -o /etc/apt/sources.list.d/tailscale.list

log "Installing Docker and Tailscale packages"
apt_get update
apt_get install \
  docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin \
  tailscale
systemctl enable --now docker tailscaled

# --- Tailnet -------------------------------------------------------------------
# Admin access goes only over the tailnet, through Tailscale SSH. tailscaled
# answers port 22 on the tailnet address itself, so it never reaches the
# portfolio container that owns port 22 on the public address.

# Captured, not piped into grep -q, which can SIGPIPE the writer under pipefail.
ts_status=$(tailscale status --json --peers=false 2>/dev/null || true)
if [[ ${ts_status} =~ \"BackendState\":\ *\"Running\" ]]; then
  log "Already on the tailnet; keeping Tailscale SSH on"
  tailscale set --ssh --hostname="${TS_HOSTNAME}"
else
  log "Joining the tailnet as ${TS_HOSTNAME}"
  [[ -n ${TS_AUTHKEY:-} ]] || die "TS_AUTHKEY is required to join the tailnet"
  tailscale up --auth-key="${TS_AUTHKEY}" --ssh --hostname="${TS_HOSTNAME}" \
    --advertise-tags="${TS_TAGS}" --timeout=120s
fi
unset TS_AUTHKEY
TS_IP=$(tailscale ip -4 | head -n 1)
[[ ${TS_IP} == 100.* ]] || die "no tailnet IPv4 address"
ts_prefs=$(tailscale debug prefs 2>/dev/null || true)
[[ ${ts_prefs} =~ \"RunSSH\":\ *true ]] || printf 'warning: could not confirm Tailscale SSH is on\n' >&2

# --- OpenSSH off ---------------------------------------------------------------
# The portfolio needs host port 22. Ubuntu 24.04 socket-activates sshd, so both
# units go. Break-glass is `az vm run-command` and the serial console, neither
# of which uses sshd. Undo, with the stack stopped: systemctl unmask
# ssh.socket ssh.service && systemctl enable --now ssh.socket. The NSG still
# keeps it off the Internet, and the next bootstrap run masks it again.

log "Disabling OpenSSH"
for unit in ssh.socket ssh.service; do
  systemctl disable --now "${unit}" >/dev/null 2>&1 || true
  systemctl mask "${unit}" >/dev/null
done
# On a re-run the portfolio's docker-proxy already listens on :22; nothing else may.
if ss -Hltnp 'sport = :22' | grep -qv docker-proxy; then
  die "something other than the portfolio still listens on port 22"
fi

# --- No host firewall ----------------------------------------------------------
# The Azure NSG is the perimeter: it admits only TCP 22, 2222, 80 and 443.
# ufw would add nothing. Docker's published ports bypass it, and with OpenSSH
# off nothing else listens publicly (tailscaled's UDP port is meant to be
# reachable). A second rule set to keep in step is only a way to get it wrong,
# so ufw stays off.

if command -v ufw >/dev/null && ufw status | grep -q '^Status: active'; then
  log "Disabling ufw (the NSG is the firewall)"
  ufw --force disable
fi

# --- Swap ----------------------------------------------------------------------
# Standard_B2ats_v2 has 1 GiB of RAM and no temporary disk. A small swap file
# keeps apt and image pulls from waking the OOM killer.

if ! swapon --show=NAME --noheadings | grep -qx /swapfile; then
  log "Adding a 1 GiB swap file"
  if [[ ! -f /swapfile ]]; then
    fallocate -l 1G /swapfile
    chmod 0600 /swapfile
    mkswap /swapfile >/dev/null
  fi
  swapon /swapfile
fi
grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab

# --- Deploy user ---------------------------------------------------------------
# CI reaches the VM over Tailscale SSH as this user. Docker group membership
# is root-equivalent; the account is only reachable over the tailnet, and the
# tailnet policy limits it to tag:ci.

log "Creating the ${DEPLOY_USER} user"
if ! id -u "${DEPLOY_USER}" >/dev/null 2>&1; then
  useradd --system --create-home --shell /bin/bash "${DEPLOY_USER}"
fi
usermod -aG docker "${DEPLOY_USER}"

# --- Application directory -----------------------------------------------------

log "Installing the stack into ${APP_DIR}"
install -d -m 0755 "${APP_DIR}"
install_file() { # mode src dst
  if [[ -f $3 ]] && cmp -s "$2" "$3"; then
    echo "unchanged: $3"
  else
    install -m "$1" "$2" "$3"
    echo "installed: $3"
  fi
}
install_file 0644 "${SCRIPT_DIR}/compose.prod.yaml" "${APP_DIR}/compose.prod.yaml"
install_file 0644 "${SCRIPT_DIR}/Caddyfile" "${APP_DIR}/Caddyfile"
install_file 0755 "${SCRIPT_DIR}/fingerprint.sh" "${APP_DIR}/fingerprint.sh"
compose config --quiet

# --- Start ---------------------------------------------------------------------
# A re-run keeps the release that is already running unless TAG says otherwise.

running_tag() {
  local id
  id=$(compose ps -q portfolio 2>/dev/null || true)
  [[ -n ${id} ]] || return 0
  docker inspect -f '{{.Config.Image}}' "${id}" | sed -n 's|^.*:\([^:/]*\)$|\1|p'
}
TAG=${TAG:-$(running_tag)}
TAG=${TAG:-latest}
[[ ${TAG} =~ ^[0-9A-Za-z._-]+$ ]] || die "invalid TAG"
export TAG

fingerprint="(stack not started)"
if [[ ${START_STACK} == 1 ]]; then
  log "Starting ${IMAGE}:${TAG}"
  compose pull --quiet
  # --wait returns once portfolio is healthy (its SSH health check) and caddy runs.
  compose up -d --remove-orphans --wait --wait-timeout 180
  fingerprint=$("${APP_DIR}/fingerprint.sh" | awk '/\(ED25519\)/ {print $1}')
  stack=started
else
  log "START_STACK=0: leaving the stack stopped"
  stack=held
fi

# --- Summary -------------------------------------------------------------------
# run-command returns only the last 4 KB of output, so keep this short and last.

log "Summary"
echo "tailnet:     ${TS_HOSTNAME} ${TS_IP} (Tailscale SSH)"
echo "openssh:     $(systemctl is-enabled ssh.socket 2>/dev/null || true)"
echo "image:       ${IMAGE}:${TAG}"
echo "fingerprint: ${fingerprint}"
echo "versions:    $(docker --version | cut -d, -f1); tailscale $(tailscale version | head -n 1)"
echo "BOOTSTRAP_OK stack=${stack}"
