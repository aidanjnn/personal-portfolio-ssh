#!/usr/bin/env bash
# Provision the portfolio VM on Azure (Azure for Students) and bootstrap it.
#
#   printf 'Tailscale auth key: '; read -rs TS_AUTHKEY; echo
#   TS_AUTHKEY=$TS_AUTHKEY deploy/azure.sh --ssh-key ~/.ssh/id_ed25519.pub
#   unset TS_AUTHKEY
#   deploy/azure.sh --dry-run     # print the az commands; needs no az, login or key
#
# Never run it with `bash -x` or `az --debug`: both print the run-command
# script, which carries the auth key.
#
# Creates resource group `portfolio` with a VNet, an NSG with no inbound allow
# rules, a Standard static IPv4 and a Standard_B2ats_v2 Ubuntu 24.04 VM with
# SSH key auth only. Runs deploy/bootstrap.sh on the VM through
# `az vm run-command invoke`, never public SSH. Only after bootstrap prints
# BOOTSTRAP_OK does it allow TCP 22, 2222, 80 and 443 from the Internet.
#
# Idempotent: existing resources are kept as they are. Re-run it to push
# edited deploy/ files to the VM. Runbook: docs/DEPLOYMENT.md.

set -euo pipefail

readonly RG=portfolio
readonly VM=portfolio
readonly VNET=portfolio-vnet
readonly SUBNET=default
readonly NSG=portfolio-nsg
readonly PIP=portfolio-ip
readonly NSG_RULE=allow-portfolio-public
readonly SIZE=Standard_B2ats_v2
readonly IMAGE_URN=Canonical:ubuntu-24_04-lts:server:latest # Gen2
# 64 GiB is a P6, the free-tier Premium SSD. The image's default 30 GiB would
# be a P4, which the free tier does not cover.
readonly OS_DISK_GB=64
readonly ADMIN_USER=azureuser
readonly DEFAULT_LOCATION=canadacentral
readonly FALLBACK_LOCATION=westus2
readonly PUBLIC_TCP_PORTS="22 2222 80 443"
readonly DEPLOY_FILES="bootstrap.sh compose.prod.yaml Caddyfile fingerprint.sh ratelimit.sh"
DEPLOY_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
readonly DEPLOY_DIR

usage() {
  cat <<'EOF'
Usage: deploy/azure.sh [options]

Provision the portfolio VM on Azure and bootstrap it over run-command.

Options:
  --ssh-key PATH     Admin SSH public key for the VM (default ~/.ssh/id_ed25519.pub).
                     Azure requires one. OpenSSH is masked and the NSG never
                     admits admin SSH, so it is not usable from the Internet.
  --location REGION  Azure region. Default canadacentral, falling back to westus2
                     when the allowed-regions policy or VM size availability
                     rules it out. An existing VNet's region always wins.
  --no-start         Install everything but leave the stack stopped and the NSG
                     closed, e.g. to restore a host key backup first.
  --dry-run, -n      Print the az commands and change nothing. Needs no az or login.
  -h, --help         Show this help.

Environment:
  TS_AUTHKEY  One-off Tailscale auth key tagged tag:portfolio. Needed until the
              VM has joined the tailnet. Never printed; it reaches the VM only
              inside the run-command script, which writes it to a mode-0600
              file for `tailscale up --auth-key=file:PATH`. Read it with
              `read -rs` so it stays out of shell history, and never use
              `bash -x` or `az --debug`, which would print it.
  TAG         Image tag to start (default: the tag already running, else latest).
EOF
}

log() { printf '\n==> %s\n' "$*" >&2; }
note() { printf '%s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
show() { printf '+' >&2; printf ' %q' "$@" >&2; printf '\n' >&2; }
run() { show "$@"; [[ ${dry_run} == 1 ]] || "$@"; }
# True if the resource exists. A dry run assumes nothing exists yet.
exists() { [[ ${dry_run} == 0 ]] && "$@" --output none >/dev/null 2>&1; }

# --- Arguments -----------------------------------------------------------------

ssh_key=${HOME}/.ssh/id_ed25519.pub
location=""
start_stack=1
dry_run=0
while [[ $# -gt 0 ]]; do
  case $1 in
    --ssh-key) [[ $# -ge 2 ]] || die "--ssh-key needs a path"; ssh_key=$2; shift 2 ;;
    --ssh-key=*) ssh_key=${1#*=}; shift ;;
    --location) [[ $# -ge 2 ]] || die "--location needs a region"; location=$2; shift 2 ;;
    --location=*) location=${1#*=}; shift ;;
    --no-start) start_stack=0; shift ;;
    -n | --dry-run) dry_run=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) usage >&2; die "unknown argument: $1" ;;
  esac
done

# Keep the secret in a plain shell variable: az and its children never see it.
ts_authkey=${TS_AUTHKEY:-}
unset TS_AUTHKEY
if [[ -n ${ts_authkey} && ! ${ts_authkey} =~ ^tskey-[A-Za-z0-9-]+$ ]]; then
  die "TS_AUTHKEY does not look like a Tailscale auth key (tskey-...)"
fi
if [[ -n ${TAG:-} && ! ${TAG} =~ ^[0-9A-Za-z._-]+$ ]]; then
  die "TAG may contain only [0-9A-Za-z._-]"
fi
[[ ${ssh_key} == *.pub ]] || die "--ssh-key must be a public key file (*.pub)"

# --- Preflight -----------------------------------------------------------------

if [[ ${dry_run} == 1 ]]; then
  log "Dry run: printing az commands. Nothing is checked, created or sent."
else
  command -v az >/dev/null || die "the Azure CLI (az) is not installed"
  subscription=$(az account show --query "join(' ', [name, id])" -o tsv 2>/dev/null) \
    || die "not logged in: run az login"
  note "subscription: ${subscription}"
  [[ -n ${ts_authkey} ]] \
    || note "warning: TS_AUTHKEY is not set; bootstrap fails unless the VM is already on the tailnet"
fi

# --- Region --------------------------------------------------------------------
# Azure for Students limits deployments to a policy-defined set of regions and
# may restrict VM sizes per region. Check both before creating anything.

allowed_locations=""
policy_allows() { [[ -z ${allowed_locations} ]] || grep -qx "$1" <<<"${allowed_locations}"; }

# Prints "ok", "unavailable" or the restriction reason (e.g. NotAvailableForSubscription).
sku_status() {
  local out name reasons
  out=$(az vm list-skus --location "$1" --size "${SIZE}" --resource-type virtualMachines \
    --query "[?name=='${SIZE}'] | [0].[name, join(',', restrictions[?type=='Location'].reasonCode || \`[]\`)]" \
    -o tsv 2>/dev/null || true)
  IFS=$'\t' read -r name reasons <<<"${out}" || true
  if [[ -z ${name} ]]; then
    echo unavailable
  elif [[ -n ${reasons} ]]; then
    echo "${reasons}"
  else
    echo ok
  fi
}

choose_location() {
  local existing candidates loc status
  existing=$(az network vnet show -g "${RG}" -n "${VNET}" --query location -o tsv 2>/dev/null || true)
  if [[ -n ${existing} ]]; then
    [[ -z ${location} || ${location} == "${existing}" ]] \
      || die "${VNET} already exists in ${existing}; delete resource group ${RG} to change regions"
    location=${existing}
    note "region: ${location} (existing network)"
    return
  fi

  allowed_locations=$(az policy assignment list --disable-scope-strict-match \
    --query '[].parameters.listOfAllowedLocations.value[]' -o tsv 2>/dev/null | sort -u || true)
  [[ -z ${allowed_locations} ]] \
    || note "regions allowed by policy: $(tr '\n' ' ' <<<"${allowed_locations}")"

  candidates=${location:-${DEFAULT_LOCATION} ${FALLBACK_LOCATION}}
  for loc in ${candidates}; do
    if ! policy_allows "${loc}"; then
      note "skipping ${loc}: not in the allowed-regions policy"
      continue
    fi
    status=$(sku_status "${loc}")
    if [[ ${status} != ok ]]; then
      note "skipping ${loc}: ${SIZE} is ${status} there"
      continue
    fi
    location=${loc}
    note "region: ${location}"
    return
  done
  die "no usable region among: ${candidates} (docs/DEPLOYMENT.md, \"Pick a region\")"
}

log "Region"
if [[ ${dry_run} == 1 ]]; then
  location=${location:-${DEFAULT_LOCATION}}
  note "region: ${location} (policy and VM size checks skipped in a dry run)"
else
  choose_location
fi

# --- Resources -----------------------------------------------------------------

log "Resource group ${RG}"
if exists az group show -n "${RG}"; then
  note "exists: ${RG}"
else
  run az group create -n "${RG}" -l "${location}" --output none
fi

log "Network: VNet, NSG (no inbound allow rules), static public IPv4"
if exists az network vnet show -g "${RG}" -n "${VNET}"; then
  note "exists: ${VNET}"
else
  run az network vnet create -g "${RG}" -n "${VNET}" -l "${location}" \
    --address-prefixes 10.42.0.0/24 --subnet-name "${SUBNET}" --subnet-prefixes 10.42.0.0/27 \
    --output none
fi
if exists az network nsg show -g "${RG}" -n "${NSG}"; then
  note "exists: ${NSG}"
else
  # Only Azure's default rules: inbound from the Internet is denied.
  run az network nsg create -g "${RG}" -n "${NSG}" -l "${location}" --output none
fi
if exists az network public-ip show -g "${RG}" -n "${PIP}"; then
  note "exists: ${PIP}"
else
  run az network public-ip create -g "${RG}" -n "${PIP}" -l "${location}" \
    --sku Standard --allocation-method Static --version IPv4 --output none
fi

log "VM ${VM} (${SIZE}, Ubuntu 24.04 LTS Gen2, ${OS_DISK_GB} GiB Premium SSD)"
if exists az vm show -g "${RG}" -n "${VM}"; then
  note "exists: ${VM}"
else
  [[ ${dry_run} == 1 || -f ${ssh_key} ]] || die "SSH public key not found: ${ssh_key} (pass --ssh-key)"
  run az vm create -g "${RG}" -n "${VM}" -l "${location}" \
    --image "${IMAGE_URN}" --size "${SIZE}" \
    --os-disk-name "${VM}-osdisk" --os-disk-size-gb "${OS_DISK_GB}" --storage-sku Premium_LRS \
    --vnet-name "${VNET}" --subnet "${SUBNET}" --nsg "${NSG}" --nsg-rule NONE \
    --public-ip-address "${PIP}" \
    --admin-username "${ADMIN_USER}" --authentication-type ssh --ssh-key-values "${ssh_key}" \
    --output none
fi
# The serial console (break-glass) needs boot diagnostics; managed storage is enough.
run az vm boot-diagnostics enable -g "${RG}" -n "${VM}" --output none

if [[ ${dry_run} == 0 ]]; then
  power=$(az vm get-instance-view -g "${RG}" -n "${VM}" \
    --query "instanceView.statuses[?starts_with(code, 'PowerState/')].code | [0]" -o tsv)
  [[ ${power} == PowerState/running ]] || run az vm start -g "${RG}" -n "${VM}" --output none
fi

# --- Bootstrap -----------------------------------------------------------------
# run-command takes one script, so the payload carries the deploy/ files as
# base64 heredocs and unpacks them into a private temp dir on the VM. The
# payload lives in a mode-0600 temp file outside the repo and is deleted on exit.
# The Azure agent keeps its own root-only copy on the VM, which is one reason
# the Tailscale key must be one-off: it is spent once the VM joins.

build_payload() { # authkey (may be empty)
  local f
  cat <<'EOF'
#!/bin/sh
# Generated by deploy/azure.sh: unpack deploy/ and run bootstrap.sh as root.
set -eu
umask 077
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
EOF
  for f in ${DEPLOY_FILES}; do
    printf "base64 -d > \"\$dir/%s\" <<'B64'\n" "${f}"
    base64 <"${DEPLOY_DIR}/${f}" | tr -d '\n'
    printf '\nB64\n'
  done
  # The key goes into a file in the private dir (umask 077, so mode 0600) via a
  # heredoc, never into the environment or a command line. The EXIT trap
  # deletes it, and bootstrap deletes it once tailscale up has read it.
  if [[ -n $1 ]]; then
    printf "cat > \"\$dir/ts-authkey\" <<'KEY'\n%s\nKEY\n" "$1"
    # shellcheck disable=SC2016 # $dir expands on the VM
    printf 'export TS_AUTHKEY_FILE="$dir/ts-authkey"\n'
  fi
  [[ -z ${TAG:-} ]] || printf "export TAG='%s'\n" "${TAG}"
  printf "export START_STACK='%s'\n" "${start_stack}"
  # shellcheck disable=SC2016 # $dir expands on the VM
  printf 'bash "$dir/bootstrap.sh"\n'
}

for f in ${DEPLOY_FILES}; do
  [[ -f ${DEPLOY_DIR}/${f} ]] || die "missing ${DEPLOY_DIR}/${f}"
done

log "Bootstrap through run-command (a few minutes; output arrives at the end)"
payload=$(mktemp "${TMPDIR:-/tmp}/portfolio-bootstrap.XXXXXX")
trap 'rm -f "${payload}"' EXIT
trap 'exit 130' INT TERM
if [[ ${dry_run} == 1 ]]; then
  build_payload "${ts_authkey:+<redacted>}" >"${payload}"
else
  build_payload "${ts_authkey}" >"${payload}"
fi

out=$(run az vm run-command invoke -g "${RG}" -n "${VM}" --command-id RunShellScript \
  --scripts "@${payload}" --query 'value[0].message' -o tsv) \
  || die "run-command failed; the NSG stays closed"

open_nsg=0
if [[ ${dry_run} == 1 ]]; then
  note "payload: $(wc -c <"${payload}" | tr -d ' ') bytes: ${DEPLOY_FILES}, then bootstrap.sh"
  [[ ${start_stack} == 0 ]] || open_nsg=1
else
  # Belt and braces: the key is never printed, but scrub it from the output anyway.
  [[ -z ${ts_authkey} ]] || out=${out//"${ts_authkey}"/<redacted>}
  printf '%s\n' "${out}" >&2
  grep -q '^BOOTSTRAP_OK ' <<<"${out}" \
    || die "bootstrap did not report BOOTSTRAP_OK; the NSG stays closed (docs/DEPLOYMENT.md, \"Troubleshooting\")"
  if grep -q '^BOOTSTRAP_OK stack=started' <<<"${out}"; then open_nsg=1; fi
fi

# --- Open the public ports -----------------------------------------------------

if [[ ${open_nsg} == 1 ]]; then
  log "Allowing TCP ${PUBLIC_TCP_PORTS} from the Internet"
  if exists az network nsg rule show -g "${RG}" --nsg-name "${NSG}" -n "${NSG_RULE}"; then
    verb=update
  else
    verb=create
  fi
  # shellcheck disable=SC2086 # the port list splits into separate arguments on purpose
  run az network nsg rule "${verb}" -g "${RG}" --nsg-name "${NSG}" -n "${NSG_RULE}" \
    --priority 1000 --direction Inbound --access Allow --protocol Tcp \
    --source-address-prefixes Internet --source-port-ranges '*' \
    --destination-address-prefixes '*' --destination-port-ranges ${PUBLIC_TCP_PORTS} \
    --description "portfolio SSH (22, 2222) and Caddy redirect (80, 443)" --output none
else
  note "Stack held (--no-start): the NSG stays closed. Restore the host key, then re-run without --no-start."
fi

if [[ ${dry_run} == 1 ]]; then
  ip="<public-ip>"
else
  ip=$(az network public-ip show -g "${RG}" -n "${PIP}" --query ipAddress -o tsv)
fi

log "Done"
note "public IPv4: ${ip}"
note "admin:       ssh root@portfolio (Tailscale SSH)"
note "next:        docs/DEPLOYMENT.md, \"Check the VM\", then \"DNS cutover\""
printf '%s\n' "${ip}"
