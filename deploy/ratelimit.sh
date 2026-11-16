#!/usr/bin/env bash
# Per-address rate limit for new connections to the portfolio's public SSH
# ports. deploy/bootstrap.sh installs it as /usr/local/sbin/portfolio-ratelimit,
# and portfolio-ratelimit.service runs it after every docker start. Idempotent.
#
# The rules live in DOCKER-USER, the chain Docker leaves to the admin and
# checks before its own forwarding rules. By then DNAT has already rewritten
# the destination to the container's port 2222, so the rules match the port
# the visitor dialled (conntrack's original destination): 22 or 2222. Both
# ports draw on one bucket per source address.
#
# The numbers: a person connects a few times a minute at most (a reconnect, a
# second window, the fallback port). Each address gets 20 back-to-back new
# connections (the burst), refilled at 10 a minute. Beyond that its new SYNs
# are dropped until the bucket refills. That stops floods and scanners that
# hammer the key exchange, which costs CPU, and no human notices it. An empty
# bucket refills in 2 minutes (20 at 10 a minute), so entries expire after 2
# minutes: a shorter expiry would hand a flooder a fresh burst after a pause.
# Established sessions are never touched. --max-per-ip in compose.prod.yaml
# caps concurrent sessions per address; this caps the connection rate.
#
# Needs Docker's iptables backend (daemon.json pins "iptables": true). Docker's
# optional nftables backend has no DOCKER-USER chain; this script then fails.

set -euo pipefail

readonly CHAIN=PORTFOLIO-RATE
readonly PORTS="22 2222"

# dockerd creates DOCKER-USER while it starts.
for _ in $(seq 1 30); do
  iptables -w -n -L DOCKER-USER >/dev/null 2>&1 && break
  sleep 1
done
iptables -w -n -L DOCKER-USER >/dev/null \
  || { echo "error: no DOCKER-USER chain; is dockerd running with iptables on?" >&2; exit 1; }

iptables -w -N "${CHAIN}" 2>/dev/null || true
iptables -w -F "${CHAIN}"
iptables -w -A "${CHAIN}" -m hashlimit --hashlimit-name portfolio-ssh --hashlimit-mode srcip \
  --hashlimit-above 10/minute --hashlimit-burst 20 --hashlimit-htable-expire 120000 -j DROP
iptables -w -A "${CHAIN}" -j RETURN

for port in ${PORTS}; do
  rule=(-p tcp --syn -m conntrack --ctstate NEW --ctorigdstport "${port}" -j "${CHAIN}")
  iptables -w -C DOCKER-USER "${rule[@]}" 2>/dev/null || iptables -w -I DOCKER-USER "${rule[@]}"
done

echo "portfolio-ratelimit: new connections to TCP ${PORTS// /, } limited to 10/min per address (burst 20)"
