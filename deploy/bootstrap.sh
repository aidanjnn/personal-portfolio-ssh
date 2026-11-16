#!/usr/bin/env bash
# Unattended, re-runnable setup for the portfolio VM (Ubuntu 24.04, amd64).
#
# deploy/azure.sh runs it as root through `az vm run-command invoke`, with the
# other deploy/ files next to it. There is no public admin SSH at any point.
# By hand, as root on the VM, with the whole deploy/ directory:
#
#   read -rs TS_AUTHKEY && TS_AUTHKEY=$TS_AUTHKEY bash deploy/bootstrap.sh
#
# Never run it with `bash -x`: the trace would print the auth key.
#
# Environment:
#   TS_AUTHKEY_FILE  File holding a one-off Tailscale auth key (azure.sh's
#                    payload writes one, mode 0600). Deleted once read.
#   TS_AUTHKEY   Alternative for runs by hand: the key itself. It is moved to
#                a mode-0600 temp file and unset before anything else runs.
#                Either is needed only until the VM has joined the tailnet.
#   TS_HOSTNAME  Tailnet name (default portfolio).
#   TS_TAGS      Tags to advertise (default tag:portfolio).
#   TAG          Image tag (default: the release already installed, kept by
#                digest when CI deployed it; else latest).
#   START_STACK  1 (default) starts the stack. 0 installs everything but
#                leaves it stopped, e.g. to restore a host key backup first.
#
# It installs Docker Engine (with a pinned daemon.json) and Tailscale, adds a
# per-address connection-rate limit, joins the tailnet with Tailscale SSH,
# disables OpenSSH so the portfolio can own port 22, creates the `deploy` user
# for CI, installs the stack into /opt/portfolio and starts it.
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
# Write stdin to a file only if its content differs. Sets changed=1 if it did.
put() { # mode dst
  local tmp
  tmp=$(mktemp)
  cat >"${tmp}"
  if [[ -f $2 ]] && cmp -s "${tmp}" "$2"; then
    changed=0
    echo "unchanged: $2"
  else
    install -D -m "$1" "${tmp}" "$2"
    changed=1
    echo "installed: $2"
  fi
  rm -f "${tmp}"
}
install_file() { put "$1" "$3" <"$2"; } # mode src dst

# --- Preflight ----------------------------------------------------------------

[[ ${EUID} -eq 0 ]] || die "run as root"

# Take the Tailscale auth key out of the environment before anything else runs:
# apt, dpkg and their maintainer scripts inherit it. tailscale up reads it from
# a mode-0600 file (--auth-key=file:PATH), so it never appears in a process's
# arguments either. The file is deleted on exit.
ts_key_file=${TS_AUTHKEY_FILE:-}
if [[ -z ${ts_key_file} && -n ${TS_AUTHKEY:-} ]]; then
  ts_key_file=$(mktemp) # mode 0600
  printf '%s\n' "${TS_AUTHKEY}" >"${ts_key_file}" # printf is a builtin: no argv
fi
unset TS_AUTHKEY TS_AUTHKEY_FILE
cleanup() { [[ -z ${ts_key_file} ]] || rm -f -- "${ts_key_file}"; }
trap cleanup EXIT
[[ ${START_STACK} == 0 || ${START_STACK} == 1 ]] || die "START_STACK must be 0 or 1"

# shellcheck source=/dev/null
. /etc/os-release
[[ ${ID} == ubuntu ]] || die "expected Ubuntu, found ${ID}"
CODENAME=${UBUNTU_CODENAME:-${VERSION_CODENAME}}
[[ ${CODENAME} == noble ]] || printf 'warning: tested on Ubuntu 24.04 (noble), found %s\n' "${CODENAME}" >&2

# The image is built for linux/amd64 only.
[[ $(dpkg --print-architecture) == amd64 ]] || die "expected an amd64 host"

for f in compose.prod.yaml Caddyfile fingerprint.sh ratelimit.sh; do
  [[ -f ${SCRIPT_DIR}/${f} ]] || die "missing ${SCRIPT_DIR}/${f}; copy the whole deploy/ directory"
done

# On first boot cloud-init may still be configuring the system.
if command -v cloud-init >/dev/null; then
  log "Waiting for cloud-init"
  cloud-init status --wait >/dev/null || true
fi

log "Installing base packages"
apt_get update
apt_get install ca-certificates curl unattended-upgrades

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
# On a re-run this also upgrades them to the latest release. A docker-ce
# upgrade restarts dockerd; live-restore (below) keeps containers running.
apt_get update
apt_get install \
  docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin \
  tailscale

# --- Docker daemon settings ----------------------------------------------------
# Pinned instead of left to Docker's defaults, which can change:
#   iptables        Published ports are DNATed by iptables, so a visitor's real
#                   source address survives into the container, and
#                   --max-per-ip counts by that address. With iptables off,
#                   docker-proxy would accept every connection itself and the
#                   portfolio would see one address (the bridge gateway) for
#                   everyone: all visitors would share 3 slots. The DOCKER-USER
#                   rate limit below needs it too. Never turn it off.
#   userland-proxy  docker-proxy then carries only connections made on the host
#                   itself (fingerprint.sh's keyscan of 127.0.0.1:2222), and the
#                   port 22 check below expects it to hold the port.
#   live-restore    Containers keep running while dockerd restarts, e.g. when
#                   apt upgrades docker-ce, so visitors are not dropped.
#   log-*           Defaults for containers that set no logging of their own.
log "Configuring the Docker daemon"
put 0644 /etc/docker/daemon.json <<'EOF'
{
  "iptables": true,
  "userland-proxy": true,
  "live-restore": true,
  "log-driver": "json-file",
  "log-opts": {
    "max-size": "10m",
    "max-file": "3"
  }
}
EOF
daemon_changed=${changed}
dockerd --validate --config-file /etc/docker/daemon.json
systemctl enable --now docker tailscaled
# The package starts dockerd before the file exists on a first run. With
# live-restore already on, a restart leaves running containers alone.
if [[ ${daemon_changed} == 1 ]]; then
  systemctl restart docker
fi

# --- Connection-rate limit -----------------------------------------------------
# iptables hashlimit in DOCKER-USER, re-applied after every docker start.
# The numbers are explained in deploy/ratelimit.sh.
log "Rate-limiting new SSH connections per source address"
install_file 0755 "${SCRIPT_DIR}/ratelimit.sh" /usr/local/sbin/portfolio-ratelimit
put 0644 /etc/systemd/system/portfolio-ratelimit.service <<'EOF'
[Unit]
Description=Per-address rate limit for new connections to the portfolio's SSH ports
After=docker.service
Requires=docker.service
PartOf=docker.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/local/sbin/portfolio-ratelimit

[Install]
WantedBy=docker.service
EOF
systemctl daemon-reload
systemctl enable portfolio-ratelimit.service
systemctl restart portfolio-ratelimit.service

# --- Tailnet -------------------------------------------------------------------
# Admin access goes only over the tailnet, through Tailscale SSH. tailscaled
# answers port 22 on the tailnet address itself, so it never reaches the
# portfolio container that owns port 22 on the public address.

# --auth-key=file:PATH needs Tailscale 1.22 or later and `set --auto-update`
# a recent release. The stable repository always serves a current one, so this
# only catches a held old package.
readonly MIN_TAILSCALE=1.56.0
ts_version=$(tailscale version)
ts_version=${ts_version%%$'\n'*}
dpkg --compare-versions "${ts_version}" ge "${MIN_TAILSCALE}" \
  || die "Tailscale ${ts_version} is older than ${MIN_TAILSCALE}"

# Captured, not piped into grep -q, which can SIGPIPE the writer under pipefail.
ts_status=$(tailscale status --json --peers=false 2>/dev/null || true)
if [[ ${ts_status} =~ \"BackendState\":\ *\"Running\" ]]; then
  log "Already on the tailnet; keeping Tailscale SSH on"
  tailscale set --ssh --hostname="${TS_HOSTNAME}"
else
  log "Joining the tailnet as ${TS_HOSTNAME}"
  [[ -n ${ts_key_file} && -s ${ts_key_file} ]] || die "TS_AUTHKEY is required to join the tailnet"
  # file: needs Tailscale 1.22 or later (checked above).
  tailscale up --auth-key="file:${ts_key_file}" --ssh --hostname="${TS_HOSTNAME}" \
    --advertise-tags="${TS_TAGS}" --timeout=120s
fi
cleanup
TS_IP=$(tailscale ip -4 | head -n 1)
[[ ${TS_IP} == 100.* ]] || die "no tailnet IPv4 address"
ts_prefs=$(tailscale debug prefs 2>/dev/null || true)
[[ ${ts_prefs} =~ \"RunSSH\":\ *true ]] || printf 'warning: could not confirm Tailscale SSH is on\n' >&2
# Tailscale upgrades its own package through apt, so fixes arrive without a
# bootstrap re-run.
tailscale set --auto-update

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
# so ufw stays off. The one host rule is the rate limit above, which admits
# everything the NSG admits, only slower for a flooding address.

if command -v ufw >/dev/null && ufw status | grep -q '^Status: active'; then
  log "Disabling ufw (the NSG is the firewall)"
  ufw --force disable
fi

# --- Automatic updates and reboots ---------------------------------------------
# unattended-upgrades installs Ubuntu's security updates daily. Kernel, libc
# and OpenSSL fixes only take effect after a reboot, so it reboots by itself
# when one is required, at 04:00 Pacific, the quietest hour for this site.
# Visitors connected at that moment are dropped; Docker (restart:
# unless-stopped), tailscaled and the rate limit all come back on boot.
# Docker Engine comes from Docker's repository, which unattended-upgrades
# leaves alone. Re-running this script upgrades it (apt-get install upgrades
# installed packages) and may restart dockerd; live-restore keeps the
# containers up meanwhile.
log "Configuring automatic security updates (reboot at 04:00 Pacific if needed)"
# Automatic-Reboot-Time is local time; the zone keeps it at 04:00 across DST.
timedatectl set-timezone America/Los_Angeles
put 0644 /etc/apt/apt.conf.d/52portfolio-unattended-upgrades <<'EOF'
// Written by deploy/bootstrap.sh.
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-WithUsers "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:00";
EOF
systemctl enable --now unattended-upgrades.service apt-daily.timer apt-daily-upgrade.timer

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
install_file 0644 "${SCRIPT_DIR}/compose.prod.yaml" "${APP_DIR}/compose.prod.yaml"
install_file 0644 "${SCRIPT_DIR}/Caddyfile" "${APP_DIR}/Caddyfile"
caddyfile_changed=${changed}
install_file 0755 "${SCRIPT_DIR}/fingerprint.sh" "${APP_DIR}/fingerprint.sh"
compose config --quiet

# --- Start ---------------------------------------------------------------------
# A re-run keeps the release that is already there unless TAG says otherwise.
# CI deploys by digest, so the current release is a full image reference
# (...@sha256:...), passed on to compose as PORTFOLIO_IMAGE.

current_image() {
  local id
  # -a: a stopped container still records the release to keep.
  id=$(compose ps -a -q portfolio 2>/dev/null || true)
  [[ -n ${id} ]] || return 0
  docker inspect -f '{{.Config.Image}}' "${id}"
}
if [[ -n ${TAG:-} ]]; then
  [[ ${TAG} =~ ^[0-9A-Za-z._-]+$ ]] || die "invalid TAG"
  PORTFOLIO_IMAGE=${IMAGE}:${TAG}
else
  PORTFOLIO_IMAGE=$(current_image)
  PORTFOLIO_IMAGE=${PORTFOLIO_IMAGE:-${IMAGE}:latest}
fi
ref=${PORTFOLIO_IMAGE#"${IMAGE}"}
[[ ${ref} =~ ^(:[0-9A-Za-z._-]+|@sha256:[0-9a-f]{64})$ ]] || die "unexpected image ${PORTFOLIO_IMAGE}"
export PORTFOLIO_IMAGE
unset TAG

fingerprint="(stack not started)"
if [[ ${START_STACK} == 1 ]]; then
  log "Starting ${PORTFOLIO_IMAGE}"
  compose pull --quiet
  # --wait returns once portfolio is healthy (its SSH health check) and caddy runs.
  # Caddy does not watch its file, and `up` does not recreate a container for a
  # changed bind-mounted file. install also replaces the file (a new inode), so
  # a running container would keep reading the old one even after a reload.
  # Recreate caddy when its file changed and it was already there.
  caddy_existed=$(compose ps -a -q caddy 2>/dev/null || true)
  compose up -d --remove-orphans --wait --wait-timeout 180
  if [[ ${caddyfile_changed} == 1 && -n ${caddy_existed} ]]; then
    log "Caddyfile changed: recreating caddy"
    compose up -d --no-deps --force-recreate --wait --wait-timeout 60 caddy
  fi
  fingerprint=$("${APP_DIR}/fingerprint.sh" | awk '/\(ED25519\)/ {print $1}')
  stack=started
elif [[ -n $(compose ps -q 2>/dev/null || true) ]]; then
  log "START_STACK=0: not starting anything; the stack was already running and stays up"
  if [[ ${caddyfile_changed} == 1 ]]; then
    log "The new Caddyfile takes effect when caddy is recreated (re-run with START_STACK=1)"
  fi
  stack=held
else
  log "START_STACK=0: leaving the stack stopped"
  stack=held
fi

# --- Summary -------------------------------------------------------------------
# run-command returns only the last 4 KB of output, so keep this short and last.

log "Summary"
echo "tailnet:     ${TS_HOSTNAME} ${TS_IP} (Tailscale SSH)"
echo "openssh:     $(systemctl is-enabled ssh.socket 2>/dev/null || true)"
echo "image:       ${PORTFOLIO_IMAGE}"
echo "fingerprint: ${fingerprint}"
echo "versions:    $(docker --version | cut -d, -f1); tailscale ${ts_version}"
echo "BOOTSTRAP_OK stack=${stack}"
