# Deploy the SSH portfolio

The portfolio is an application served over SSH. A visitor's SSH session opens the portfolio and never a system shell. This runbook takes it from a local container to `ssh aidanjeon.com` on one small Azure VM, paid for by an Azure for Students subscription.

**Status:** prepared, not executed. No Azure resource, DNS record, tailnet or registry package exists yet. The files live in [`deploy/`](../deploy) and [`.github/workflows/deploy.yml`](../.github/workflows/deploy.yml). They have only been checked locally (`bash -n`, `docker compose config`, `deploy/azure.sh --dry-run`).

> **App flags.** `deploy/compose.prod.yaml` passes `--max-per-ip`, `--max-session-secs` and `--content-url`. The first release you deploy must support them, or the container exits on start. Check with `docker run --rm ghcr.io/aidanjnn/personal-portfolio-ssh:<tag> --help`. `--content-url` also needs outbound HTTPS and trusted CA roots inside the image.

## Architecture

`deploy/azure.sh` creates everything in one resource group:

| Resource | Name | Setting |
| --- | --- | --- |
| Resource group | `portfolio` | Region `canadacentral`; `westus2` if the student policy refuses it |
| VM | `portfolio` | `Standard_B2ats_v2` (2 vCPU burstable, 1 GiB RAM, x64), Ubuntu 24.04 LTS Gen2, SSH key auth only |
| OS disk | `portfolio-osdisk` | Premium SSD P6, 64 GiB |
| Public IP | `portfolio-ip` | Standard SKU, static, IPv4 only. Also the VM's outbound path |
| Network | `portfolio-vnet`, `portfolio-nsg` | NSG on the NIC. One allow rule, added last: TCP 22, 2222, 80, 443 from `Internet` |

x64, not Arm: the image is built for linux/amd64 only.

| Name | DNS record (Namecheap) | Port | Served by | Result |
| --- | --- | --- | --- | --- |
| `aidanjeon.com` | A → VM | 22/tcp | `portfolio` container :2222 | SSH portfolio |
| `aidanjeon.com` | A → VM | 2222/tcp | `portfolio` container :2222 | SSH fallback for networks that block 22 |
| `aidanjeon.com` | A → VM | 80, 443/tcp | `caddy` | `308` → `https://www.aidanjeon.com{uri}` |
| `ssh.aidanjeon.com` | A → VM | 22, 2222, 80, 443 | same as apex | SSH portfolio; browsers get the same `308` |
| `www.aidanjeon.com` | CNAME → Vercel (unchanged) | 443 | Vercel | Website (primary domain) and `/portfolio.json` |
| `portfolio` (tailnet name) | MagicDNS only | 22 on the tailnet IP | Tailscale SSH | Admin access |

| File | On the VM | Purpose |
| --- | --- | --- |
| `deploy/azure.sh` | runs on your laptop | Creates the Azure resources, runs bootstrap over run-command, then opens the NSG |
| `deploy/bootstrap.sh` | sent by `azure.sh` | Docker (pinned `/etc/docker/daemon.json`), rate limit, Tailscale (joins with `--ssh`), OpenSSH masked, `deploy` user, `/opt/portfolio`, starts the stack |
| `deploy/compose.prod.yaml` | `/opt/portfolio/compose.prod.yaml` | `portfolio` and `caddy` services, hardened |
| `deploy/Caddyfile` | `/opt/portfolio/Caddyfile` | Apex and `ssh.` redirects with Vercel's HSTS header, automatic Let's Encrypt, HTTP/1.1 and HTTP/2 only |
| `deploy/fingerprint.sh` | `/opt/portfolio/fingerprint.sh` | Prints the host-key fingerprint to publish |
| `deploy/ratelimit.sh` | `/usr/local/sbin/portfolio-ratelimit` | Per-address connection-rate limit in `DOCKER-USER`, run by `portfolio-ratelimit.service` after every docker start |
| `.github/workflows/deploy.yml` | GitHub Actions | Builds/pushes the image on `v*` tags; optional rollout over Tailscale SSH |

**Volumes.** The compose project is named `portfolio`, so volumes are `portfolio_host-key` (the SSH identity), `portfolio_caddy-data` (certificates) and `portfolio_caddy-config`. **Never run `docker compose down -v`.**

**Hardening, both services:** read-only root filesystem, `cap_drop: ALL` (Caddy gets back only `NET_BIND_SERVICE`), `no-new-privileges`, `init`, CPU/memory/PID limits, `json-file` logs capped at 3 × 1 MB, `restart: unless-stopped`. The portfolio runs as UID/GID 10001, set both by the Dockerfile's `USER` and by `user: "10001:10001"` in compose.

**Pinned images.** Caddy is pinned to an exact version and index digest in `compose.prod.yaml`, and the Dockerfile's `rust` and `debian` base images are pinned by digest. Each file says how to bump its pin (`docker buildx imagetools inspect <image>:<tag>` prints the digest). After bumping Caddy, re-run `azure.sh`; CI never touches files on the VM.

**Perimeter.** The NSG is the firewall. It starts with Azure's default rules only, so nothing from the Internet gets in while bootstrap runs. The allow rule is added only after bootstrap reports success. There is no host firewall: Docker's published ports bypass ufw, and with OpenSSH off nothing else listens publicly, so ufw would only be a second list to keep in step. The NSG admits no UDP, so Caddy does not offer HTTP/3.

**Rate limit.** The NSG cannot rate-limit, so `portfolio-ratelimit.service` adds an iptables `hashlimit` rule to Docker's `DOCKER-USER` chain: new TCP connections to ports 22 and 2222 get a shared bucket per source address of 20 back-to-back, refilled at 10 a minute. A flood's extra SYNs are dropped; people never get near it. The unit runs after every docker start, so the rule survives reboots and daemon restarts. Check it with `iptables -L PORTFOLIO-RATE -nv` on the VM. `--max-per-ip 3` separately caps concurrent sessions per address.

**Real source addresses.** Both limits count by the visitor's IP, which only works while it survives Docker's DNAT into the container. `bootstrap.sh` pins this in `/etc/docker/daemon.json`: `"iptables": true` (DNAT keeps the source; with it off, docker-proxy would accept every connection and the container would see the bridge gateway for everyone, so all visitors would share 3 slots), `"userland-proxy": true` (only host-local connections go through docker-proxy), `"live-restore": true` (dockerd restarts don't stop containers), and log defaults. Docker restarts only when the file changes. Visitors behind one shared NAT, such as a campus, office or carrier-grade NAT, really do share one address, so they share 3 sessions and one rate bucket.

**Admin access.** Only Tailscale SSH, on the tailnet address. OpenSSH is disabled and masked, and no NSG rule ever admits admin SSH. The VM's admin account (`azureuser`) holds your public key only because Azure requires one. Break-glass is `az vm run-command`, the serial console, and `az vm repair` when the VM agent itself is down ([below](#break-glass)).

## Cost

Azure for Students gives $100 of credit for 12 months, with no card, plus 12 months of free services. The setup is sized to the free services.

| Item | Free for 12 months | Otherwise, roughly |
| --- | --- | --- |
| `Standard_B2ats_v2` Linux VM | 750 h/month. One VM running all month is at most 744 h | $7/month |
| P6 64 GiB Premium SSD | 2 disks | $10/month |
| Standard static IPv4 | Listed in the free services (1,500 h/month) at the time of writing; check your offer | $3.65/month |
| Outbound data | First 100 GB/month | SSH sessions and redirects are tiny |
| Managed boot diagnostics | Not free | Cents |

Check the pricing calculator for your region; `canadacentral` costs a little more than US regions. Even with nothing free, the credit would last about 5 months.

What to watch:

- **The credit balance:** [microsoftazuresponsorships.com/Balance](https://www.microsoftazuresponsorships.com/Balance). Look again a few days after provisioning: anything above cents means something is not on the free meters.
- **Cost Management → Cost analysis**, grouped by resource, weekly. Set a budget alert (Cost Management → Budgets) at $5/month.
- **Free-tier limits.** Exactly one B-series VM of this size. Keep the disk at 64 GiB: 30 GiB is a P4 and 128 GiB a P10, neither free. Snapshots and extra IPs cost money. A deallocated VM stops VM charges but not the static IP's.
- **The end date.** When the credit runs out or the 12 months end, the subscription is disabled and the VM stops. Before month 11, renew Azure for Students (if still eligible) or upgrade to pay-as-you-go.

## Prerequisites

| Item | Notes |
| --- | --- |
| Azure for Students | Sign up with your school email at [azure.microsoft.com/free/students](https://azure.microsoft.com/free/students). No card |
| Azure CLI on your laptop | `az login`, then `az account set --subscription "Azure for Students"` |
| SSH public key | Default `~/.ssh/id_ed25519.pub`, or pass `--ssh-key`. Required by Azure, never reachable |
| Tailscale account | MagicDNS on. Policy below |
| Tailscale auth key | One per provisioning run. See [Provision](#provision) |
| Tailscale OAuth client | Auth-keys write scope, limited to `tag:ci`. Only for CI rollouts |
| Namecheap | Advanced DNS access for `aidanjeon.com` |
| GitHub | Repo `aidanjnn/personal-portfolio-ssh` (public), Actions enabled; package write via `GITHUB_TOKEN` |
| Offline secret storage | Password manager or encrypted drive for the host key |

Tailnet policy (merge into yours; replace a default allow-all rule):

```jsonc
{
  "tagOwners": {
    "tag:portfolio": ["autogroup:admin"],
    "tag:ci": ["autogroup:admin"]
  },
  "acls": [
    { "action": "accept", "src": ["autogroup:admin"], "dst": ["tag:portfolio:22"] },
    { "action": "accept", "src": ["tag:ci"], "dst": ["tag:portfolio:22"] }
  ],
  "ssh": [
    { "action": "check", "src": ["autogroup:admin"], "dst": ["tag:portfolio"], "users": ["root", "deploy"] },
    { "action": "accept", "src": ["tag:ci"], "dst": ["tag:portfolio"], "users": ["deploy"] }
  ]
}
```

GitHub settings, used only by the `deploy` job:

| Kind | Name | Value |
| --- | --- | --- |
| Variable | `DEPLOY_ENABLED` | `true` to roll out after each build. Leave unset until the VM and DNS are live |
| Variable | `DEPLOY_HOST` | Tailnet hostname: `portfolio`, or the full `portfolio.<tailnet>.ts.net` if the short name doesn't resolve |
| Variable | `DEPLOY_USER` | Optional, default `deploy` |
| Variable | `PUBLIC_HOST` | Optional, default `ssh.aidanjeon.com`. Smoke-test target |
| Variable | `HOST_KEY_FINGERPRINT` | Optional `SHA256:...`. The smoke test fails if the served key differs |
| Secret | `TS_OAUTH_CLIENT_ID`, `TS_OAUTH_SECRET` | Tailscale OAuth client |

## Pick a region

Azure for Students only deploys to a policy-defined set of regions, and some VM sizes are restricted per subscription. `azure.sh` checks both and falls back from `canadacentral` to `westus2` on its own. To look yourself:

```sh
# Regions the subscription's policy allows (Portal: Policy → Assignments →
# "Allowed resource deployment regions" → Parameters)
az policy assignment list --disable-scope-strict-match \
  --query '[].parameters.listOfAllowedLocations.value[]' -o tsv | sort -u

# Is the size offered, and unrestricted, there? Restrictions must be empty.
az vm list-skus --location canadacentral --size Standard_B2ats_v2 --resource-type virtualMachines -o table

# vCPU quota: the VM needs 2 in "Total Regional vCPUs" and "Standard BASv2 Family vCPUs"
az vm list-usage --location canadacentral -o table
```

Pass `--location <region>` to force one.

## Provision

1. **Image.** Push a tag (see [Release](#release-and-rollback)) so `ghcr.io/aidanjnn/personal-portfolio-ssh:<tag>` exists. Then make the package public: GitHub → your profile → Packages → `personal-portfolio-ssh` → Package settings → Change visibility → Public. It holds no secrets, and the VM pulls it without `docker login`. Check from a logged-out Docker: `docker pull ghcr.io/aidanjnn/personal-portfolio-ssh:<tag>`.
2. **Auth key.** Tailscale admin console → Settings → Keys → Generate auth key: not reusable, not ephemeral, pre-approved, tag `tag:portfolio`, expiry 1 day. It is spent when the VM joins.
3. **Dry run.** Prints every `az` command and changes nothing:
   ```sh
   deploy/azure.sh --dry-run
   ```
4. **Provision.** Read the key without echoing it, so it never lands in your shell history, then run (bash or zsh):
   ```sh
   printf 'Tailscale auth key: '; read -rs TS_AUTHKEY; echo
   TS_AUTHKEY=$TS_AUTHKEY TAG=<tag> deploy/azure.sh --ssh-key ~/.ssh/id_ed25519.pub
   unset TS_AUTHKEY
   ```
   It takes about 10 minutes and prints the public IPv4 last. Without `TAG` it starts `:latest`.

   > **Never** run `azure.sh` with `bash -x`, or add `az --debug` to its commands. Both print the run-command script, auth key included, to your terminal.

What `azure.sh` does, in order:

1. Picks the region and checks the VM size there.
2. Creates the resource group, VNet, NSG (no inbound allow rules), static IP and VM (`--nsg-rule NONE`). Existing resources are left alone. Only a `NotFound` answer counts as missing; any other `az` error stops the script.
3. Enables boot diagnostics, for the serial console.
4. Sends `deploy/` to the VM inside one `az vm run-command invoke` script and runs `bootstrap.sh` as root. Bootstrap installs Docker (pinning `daemon.json`) and Tailscale, adds the rate limit, joins the tailnet with `tailscale up --ssh`, masks `ssh.socket` and `ssh.service`, adds a 1 GiB swap file, creates the `deploy` user, installs `/opt/portfolio`, pulls the image and starts the stack. It waits for the portfolio's health check and prints the fingerprint, then `BOOTSTRAP_OK`.
5. Only on `BOOTSTRAP_OK`, adds the NSG rule for TCP 22, 2222, 80 and 443.

The auth key never appears in output, in the repo or on a command line. It travels in the run-command script, from a mode-0600 temp file on your laptop that is deleted on exit. On the VM the script writes it to a mode-0600 file in a private temp dir, and `bootstrap.sh` unsets every key variable before apt or dpkg runs, passes the file with `tailscale up --auth-key=file:PATH` (Tailscale 1.22 or later; bootstrap checks) and deletes it. The Azure agent keeps a root-only copy of the script on the VM, which is why the key must be one-off.

**Re-running.** `azure.sh` is idempotent. Re-run it after changing anything in `deploy/`; CI does not sync those files. A re-run keeps the current release, even if its container is stopped, unless you set `TAG`, and needs no auth key once the VM is on the tailnet. Once the NSG rule exists, a failed re-run leaves it in place and says so; `--no-start` on a running VM starts nothing new but doesn't stop the stack or close the NSG either (the script prints the `az network nsg rule delete` command if you want the ports closed).

### Check the VM

From your laptop:

```sh
ssh root@portfolio 'hostname; systemctl is-enabled ssh.socket ssh.service; docker compose -f /opt/portfolio/compose.prod.yaml ps'
ssh -p 2222 <public-ip>      # portfolio; the fingerprint matches azure.sh's output
ssh <public-ip>              # portfolio on 22
```

The first command must reach the host shell over Tailscale (not the portfolio), print `masked` twice, and show `portfolio` healthy and `caddy` running. Then record the fingerprint and [back up the host key](#host-key-backup-and-restore). Caddy logs ACME errors until DNS points here. That is expected.

### Rebuild with an existing host key

To keep the published fingerprint on a new VM, restore the key before the stack ever starts:

0. Delete the old VM (or `az group delete -n portfolio` for a clean slate), then **remove the old `portfolio` machine from the tailnet** (Tailscale admin console → Machines → `portfolio` → Remove). Otherwise the new VM joins as `portfolio-1`, and `ssh root@portfolio`, `DEPLOY_HOST` and the tailnet policy's expectations all point at the dead node.
1. Read a fresh auth key as in [Provision](#provision) step 4, then `TS_AUTHKEY=$TS_AUTHKEY deploy/azure.sh --no-start`. Bootstrap installs everything but leaves the stack stopped and the NSG closed.
2. Restore the key ([below](#host-key-backup-and-restore)).
3. `deploy/azure.sh` again. It starts the stack and opens the NSG. Check that `fingerprint.sh` prints the published value.

### Troubleshooting

| Symptom | Fix |
| --- | --- |
| `RequestDisallowedByPolicy` or `RequestDisallowedByAzure` | The region is refused. Re-run with `--location westus2` or another [allowed region](#pick-a-region). If `portfolio-vnet` already exists in the refused region, `az group delete -n portfolio` first |
| `SkuNotAvailable` or `NotAvailableForSubscription` | The size is restricted there. Use another region |
| `QuotaExceeded` or `OperationNotAllowed` naming vCPUs | Check `az vm list-usage`. Use another region or request quota in the portal |
| `az vm create` rejects the SSH key | An older Azure CLI may not take Ed25519. Upgrade `az`, or pass an RSA public key. It is never used to log in |
| No `BOOTSTRAP_OK` | run-command returns only the last 4 KB of output. The full output is in `/var/lib/waagent/run-command/download/<n>/stdout` and `stderr` on the VM; read it with run-command ([below](#break-glass)). Fix the cause and re-run. The NSG stays closed until bootstrap succeeds |
| `TS_AUTHKEY is required` or `tailscale up` fails | The key is missing, used or expired. Generate a new one and re-run |
| run-command reports a conflict | Only one runs at a time. Wait and retry |
| `could not tell whether the resource exists` | An `az ... show` failed with something other than `NotFound` (login expired, throttling, network, permissions). `azure.sh` stops rather than re-create the resource, since re-creating the NSG would wipe its allow rule. Fix the cause and re-run |

## Break-glass

Three ways in without Tailscale, in this order. None needs OpenSSH, so it stays masked.

**1. Run Command** (first resort). It runs a script as root through the Azure VM agent (`walinuxagent`), takes about 20 seconds at minimum and returns the last 4 KB of output:

```sh
az vm run-command invoke -g portfolio -n portfolio --command-id RunShellScript \
  --scripts 'tailscale status; docker compose -f /opt/portfolio/compose.prod.yaml ps' \
  --query 'value[0].message' -o tsv
```

If the VM drops off the tailnet (node removed, state lost), delete any leftover `portfolio` node in the Tailscale admin console, generate a new auth key and re-run `azure.sh`. Bootstrap rejoins when Tailscale is not running.

**2. Serial console**, for an interactive shell while the agent still works but networking or Tailscale doesn't. It needs boot diagnostics (`azure.sh` enables it) and a password, which the VM does not have. `az vm user update` sets one through the VMAccess extension, which needs the same agent as Run Command, so this is no help once the agent is down:

1. Set a temporary one: `az vm user update -g portfolio -n portfolio -u azureuser -p '<temporary>'`.
2. Portal → `portfolio` VM → Help → Serial console. Log in as `azureuser`, then `sudo -i`.
3. Afterwards, lock it again: run `passwd -l azureuser` through Run Command.

**3. Repair VM**, when the agent itself is broken (Run Command times out or the VM agent shows "Not Ready"). `az vm repair` doesn't use the VM's agent at all: it copies the OS disk, attaches the copy to a new rescue VM, lets you fix it there, then swaps the fixed copy in as the OS disk. It needs the `vm-repair` CLI extension:

```sh
az extension add --name vm-repair        # once; later: az extension update --name vm-repair

# Rescue VM in its own resource group, with a copy of the OS disk attached as a data disk.
# No public IP. The password is for the rescue VM only, which restore deletes.
printf 'Rescue VM password: '; read -rs REPAIR_PW; echo
az vm repair create -g portfolio -n portfolio \
  --repair-username repairadmin --repair-password "$REPAIR_PW" --verbose
unset REPAIR_PW

# Fix the copy: runs fix.sh (below) as root on the rescue VM through its own, healthy agent.
az vm repair run -g portfolio -n portfolio --run-on-repair \
  --custom-script-file fix.sh --verbose

# Swap the fixed copy in as the OS disk, restart the VM and delete the rescue resources.
az vm repair restore -g portfolio -n portfolio --verbose
```

`fix.sh`, adapted to what is broken:

```bash
#!/bin/bash
set -euxo pipefail
# The copied disk's root filesystem: labelled cloudimg-rootfs and not mounted.
root=$(lsblk -nrpo NAME,LABEL,MOUNTPOINT | awk '$2 == "cloudimg-rootfs" && NF == 2 {print $1}')
mkdir -p /rescue && mount "$root" /rescue
chroot /rescue systemctl enable walinuxagent.service   # bring the agent (and Run Command) back
chroot /rescue systemctl enable tailscaled.service docker.service
# Optional, for the serial console: a temporary password. Lock it again afterwards.
# echo 'azureuser:<temporary>' | chroot /rescue chpasswd
umount /rescue
```

For boot failures (bad `fstab`, initrd, kernel), the built-in Linux auto-repair scripts do the work instead: `az vm repair run -g portfolio -n portfolio --run-on-repair --run-id linux-alar2 --parameters fstab` (or `initrd`, `kernel`). After `restore`, the VM boots from the fixed copy with the same tailnet identity and host-key volume. The original OS disk stays behind unattached: once the VM is healthy, delete it with `az disk delete -g portfolio -n portfolio-osdisk` (check the name with `az disk list -g portfolio -o table`), since a third P6 disk is not free. The rescue VM costs a few cents per hour while it exists.

## DNS cutover

DNS is at Namecheap (Domain List → `aidanjeon.com` → Advanced DNS). Today the apex A record points at Vercel (`216.198.79.1`), which already redirects it to `www`. Change **only** the apex A record and add `ssh`. Leave the `www` CNAME, every MX and TXT record, and Mail Settings (email forwarding) exactly as they are.

1. **A day before:** screenshot the host records and Mail Settings. Lower the `@` A record's TTL to 5 min and wait out the old TTL. If `dig +short CAA aidanjeon.com` returns anything, it must allow `letsencrypt.org`.
2. **Add `ssh` first:** A record, host `ssh`, value `<vm-ip>`, TTL 5 min. Nothing depends on this name yet, so it tests the VM end to end before the apex moves. Caddy has been failing ACME for this name since it started and backs off longer after each failure, so as soon as `dig +short ssh.aidanjeon.com @1.1.1.1` returns the VM, restart it to retry now:
   ```sh
   ssh root@portfolio 'docker compose -f /opt/portfolio/compose.prod.yaml restart caddy'
   curl -sSI https://ssh.aidanjeon.com       # 308 to https://www.aidanjeon.com/ (may take a minute for the certificate)
   ssh ssh.aidanjeon.com                     # portfolio; fingerprint matches
   ```
3. **Confirm** `curl -sSI https://aidanjeon.com` still shows Vercel's redirect (`307` to `https://www.aidanjeon.com/`, with `strict-transport-security: max-age=63072000`), and `docker compose -f /opt/portfolio/compose.prod.yaml ps` shows `portfolio` healthy.
4. **Move the apex at a quiet hour** (e.g. late evening Pacific): edit the A record with host `@` from `216.198.79.1` to `<vm-ip>`. Change nothing else. No AAAA record: the VM is IPv4 only.

   Why the quiet hour: Vercel sends `Strict-Transport-Security` for the apex, so every browser that has visited `aidanjeon.com` in the last two years insists on HTTPS there and won't let anyone click past a certificate error. From the moment a resolver hands out the VM's address until Caddy has a certificate for the apex, those browsers get a hard error instead of the redirect. Keep that window short and at a time few people visit. Caddy sends the same HSTS header as Vercel (`deploy/Caddyfile`), so the policy neither lapses nor changes afterwards.
5. **Certificates:** once `dig +short aidanjeon.com @1.1.1.1` returns the VM, restart Caddy so ACME retries right away instead of waiting out its backoff: `ssh root@portfolio 'docker compose -f /opt/portfolio/compose.prod.yaml restart caddy'`. Watch `logs -f caddy` for "certificate obtained". If a resolver Let's Encrypt uses still had the old address, the attempt fails; wait for the old TTL and restart once more.
6. **Verify:**
   ```sh
   curl -sSI https://aidanjeon.com          # 308, location: https://www.aidanjeon.com/, strict-transport-security: max-age=63072000
   curl -sSI https://aidanjeon.com/a?b=c    # location keeps /a?b=c
   curl -sSI http://ssh.aidanjeon.com       # 308 to https, then 308 to www
   dig +short MX aidanjeon.com; dig +short TXT aidanjeon.com   # same as the screenshot
   ```
   From another network (e.g. a phone hotspot): `ssh aidanjeon.com` and `ssh -p 2222 ssh.aidanjeon.com`. The fingerprint must match the published one. Send a test email to a forwarded address.
7. **Rollback:** set the `@` A record back to `216.198.79.1`. Keep `aidanjeon.com` attached to the Vercel project for a week so this works; Vercel will flag its DNS as misconfigured meanwhile.
8. **After a quiet week:** raise the TTLs back (e.g. 30 min), and optionally remove `aidanjeon.com` from the Vercel project.

## Host key backup and restore

The key is `/data/ssh_host_ed25519_key` in volume `portfolio_host-key`, owned by UID 10001, mode 0600. It is generated on the first start and is the server's permanent identity. Never regenerate it, put it in Git or an image, or paste it into logs.

**Backup**, from your laptop over Tailscale. This works whether or not the service is running:

```sh
ssh root@portfolio 'docker compose -f /opt/portfolio/compose.prod.yaml run --rm --no-deps -T --entrypoint tar portfolio -C /data -cf - .' > portfolio-host-key.tar

# Check it: ssh-keygen refuses keys read from a pipe, so extract privately.
mkdir -m 700 host-key-check && tar -xf portfolio-host-key.tar -C host-key-check
ssh-keygen -lf host-key-check/ssh_host_ed25519_key   # matches the published fingerprint
rm -rf host-key-check
```

Store the tar in your password manager or an encrypted drive. Delete stray copies.

**Restore** to a VM whose stack has not started yet (see [Rebuild with an existing host key](#rebuild-with-an-existing-host-key)):

```sh
ssh root@portfolio 'docker compose -f /opt/portfolio/compose.prod.yaml run --rm --no-deps -T --entrypoint tar portfolio -C /data -xf -' < portfolio-host-key.tar
```

`compose run` pulls the image if needed, creates the volume with the right labels and runs as UID 10001 with the service's mounts, so ownership and mode survive.

## Publish the fingerprint

1. Run `/opt/portfolio/fingerprint.sh` (bootstrap also prints it). It reads the public key inside the container and checks it against what `127.0.0.1:2222` serves.
2. On the website, next to the connect command, show `ssh aidanjeon.com`, the fallback `ssh -p 2222 ssh.aidanjeon.com`, and `SHA256:... (ED25519)`. Optionally show the `known_hosts` line the script prints.
3. Set the `HOST_KEY_FINGERPRINT` repo variable to the same `SHA256:...` value so every rollout checks it.

## Release and rollback

| Action | How |
| --- | --- |
| Release | With CI green on `main`: `git tag -a v0.1.0 -m "v0.1.0" && git push origin v0.1.0` |
| What CI does | Builds linux/amd64, pushes `:v0.1.0` and `:latest`. If `DEPLOY_ENABLED` is `true`, joins the tailnet as `tag:ci`, then over Tailscale SSH as `deploy` runs `cd /opt/portfolio && TAG=v0.1.0 docker compose -f compose.prod.yaml pull && ... up -d`. Then `ssh-keyscan` on `PUBLIC_HOST` ports 2222 and 22 must return an ed25519 key (and the expected fingerprint, if set) |
| Rebuild/redeploy a tag | Actions → Deploy → Run workflow → existing tag. This also moves `:latest` to it |
| Manual deploy | `ssh root@portfolio` then `cd /opt/portfolio && TAG=<tag> docker compose -f compose.prod.yaml pull && TAG=<tag> docker compose -f compose.prod.yaml up -d` |
| Change `deploy/` files | Re-run `deploy/azure.sh`. It keeps the running tag |
| Rollback | `TAG=<previous> docker compose -f compose.prod.yaml up -d` (the old image is usually still local; `pull` first if not). Then reconnect and compare the fingerprint |
| After a rollback | Re-run the workflow for `<previous>` so `:latest` stops pointing at the bad release. A plain `up -d` without `TAG` uses `:latest` |

Rollback never touches volumes. Keep at least the previous image: don't `docker image prune -a` right after a release.

## Monitoring

| Check | Target | Expect |
| --- | --- | --- |
| HTTP (no redirect following) | `https://aidanjeon.com` | `308`, `location` starts with `https://www.aidanjeon.com` |
| TCP | `aidanjeon.com:22` | Open |
| TCP | `ssh.aidanjeon.com:2222` | Open |
| TLS expiry | `aidanjeon.com`, `ssh.aidanjeon.com` | More than 14 days left (Caddy renews at ~30) |
| Azure credit | Sponsorship balance, Cost analysis | No surprise charges; see [Cost](#cost) |
| CPU credits | VM → Metrics → "CPU Credits Remaining" | Not near zero. B-series VMs drop to baseline CPU when credits run out |

Use any external uptime service with free HTTP and TCP checks, alerting to email or phone. On the box:

```sh
docker compose -f /opt/portfolio/compose.prod.yaml ps          # health
docker compose -f /opt/portfolio/compose.prod.yaml logs --since 1h portfolio
docker stats --no-stream
free -m                                                        # 1 GiB RAM plus 1 GiB swap
```

The health check proves an SSH key exchange, not a full session. `restart: unless-stopped` restarts exited containers (including after reboots and unattended upgrades) but not merely unhealthy ones.

**Patching.** Mostly automatic:

- **Ubuntu security updates:** unattended-upgrades installs them daily and, when one needs a reboot (kernel, libc, OpenSSL), reboots at **04:00 Pacific** (`bootstrap.sh` sets the VM's time zone to `America/Los_Angeles` and writes `/etc/apt/apt.conf.d/52portfolio-unattended-upgrades`). Visitors connected at that moment are dropped. Docker, the containers (`restart: unless-stopped`), tailscaled and the rate limit all come back on boot.
- **Tailscale:** `tailscale set --auto-update` lets it upgrade its own package.
- **Docker Engine** comes from Docker's repository, which unattended-upgrades doesn't touch. Once a month, over Tailscale SSH, run `apt-get update && apt-get upgrade`. Re-running `azure.sh` does the same for docker and tailscale, because bootstrap's `apt-get install` upgrades installed packages. A docker-ce upgrade restarts dockerd, and `live-restore` keeps the containers and their visitors' sessions running through it.

After any manual upgrade or reboot, check the fingerprint.

## Risks

| Risk | Mitigation |
| --- | --- |
| Credit or 12-month free period runs out | The subscription is disabled and the VM stops. Budget alert, balance checks, and a decision before month 11 |
| Region policy or size restriction | `azure.sh` checks both and falls back to `westus2` |
| Corporate or campus networks block outbound 22 | Advertise `ssh -p 2222 ssh.aidanjeon.com` next to the main command |
| Host key loss | Offline backup, restored with `--no-start`. If truly lost: publish the new fingerprint with a dated note, and update `HOST_KEY_FINGERPRINT` |
| Bot traffic on 22 (scanners, brute force) | Anonymous sessions only run the TUI. Limits: 32 sessions, 3 per IP, 300 s idle, 1800 s max, 10 new connections/min per IP after a burst of 20 (`DOCKER-USER` hashlimit), CPU/memory/PID caps, bounded logs |
| Visitors behind a shared NAT (campus, office, carrier) | They share one public address, so 3 concurrent sessions and one rate bucket between them. Raise `--max-per-ip` if that bites |
| Real client IPs lost (Docker's `iptables` off, or a proxy in front) | Every visitor would count as one address. `daemon.json` pins `"iptables": true`; the launch checklist verifies real IPs |
| A port published by mistake | The NSG admits only TCP 22, 2222, 80 and 443, whatever compose publishes |
| Upgrades restart dockerd or reboot the VM | `live-restore` keeps containers up through a dockerd restart (bootstrap re-runs and manual upgrades upgrade docker-ce). Reboots happen only when an update requires one, at 04:00 Pacific, and everything starts on boot |
| IPv6 | Not offered: IPv4 VNet and IP, no AAAA. Adding it needs a dual-stack VNet and IP, and a check that the container sees real IPv6 peers, since Docker's default bridge shows them all as the gateway and `--max-per-ip` would count them together |
| Tailscale down or misconfigured | Run Command or the serial console; `az vm repair` if the VM agent is down too ([Break-glass](#break-glass)) |
| Auth key copy on the VM | The run-command script stays in a root-only agent directory. The key is one-off, tagged, and expires in a day |
| 1 GiB RAM, burstable CPU | Swap file; containers capped at 256 + 128 MiB and 0.5 CPU each. Watch CPU credits |
| ACME fails at cutover | Restart Caddy after each record resolves. Mind Let's Encrypt's failed-validation limit; don't restart in a loop |
| HSTS-holding browsers see a certificate error during the apex move | Move the apex at a quiet hour, restart Caddy as soon as it resolves. Caddy keeps Vercel's `max-age=63072000` |
| Container doesn't start after an app change | Roll back by `TAG`; check the app supports every flag in `compose.prod.yaml` |
| `deploy` user is root-equivalent (docker group) | Reachable only over the tailnet, by `tag:ci`, per the SSH policy |

## Launch checklist

- [ ] Release image `:<tag>` on GHCR, public, pullable without login; `--help` lists all compose flags
- [ ] `deploy/azure.sh` finished with `BOOTSTRAP_OK`; region and size noted
- [ ] NSG shows one allow rule (TCP 22, 2222, 80, 443 from Internet) besides Azure's defaults
- [ ] `ssh root@portfolio` reaches the host shell; `ssh.socket` and `ssh.service` masked
- [ ] Stack up; `portfolio` healthy; read-only, non-root, limits applied (`docker inspect`)
- [ ] Fingerprint recorded; host key backed up offline and test-restored (or the tar listed and checked)
- [ ] `ssh <ip>` and `ssh -p 2222 <ip>` work
- [ ] While connected from another network (e.g. a phone hotspot), the container sees your real public IP, not a `172.x` gateway: as root on the VM, `nsenter -t "$(docker inspect -f '{{.State.Pid}}' portfolio-portfolio-1)" -n ss -tn state established '( sport = :2222 )'` lists your phone network's address (compare `curl -s https://ifconfig.me` there)
- [ ] `iptables -L PORTFOLIO-RATE -nv` lists the hashlimit rule, and `systemctl is-enabled portfolio-ratelimit` says `enabled`
- [ ] Cost analysis a few days in shows only free meters (or cents); budget alert set
- [ ] Namecheap screenshot taken; TTL lowered a day ahead; CAA allows Let's Encrypt
- [ ] `ssh` A record → VM; certificate obtained; `ssh ssh.aidanjeon.com` works
- [ ] Apex A record → VM; MX, TXT and `www` unchanged; test email forwarded
- [ ] `curl -I https://aidanjeon.com` → 308 to www; `ssh aidanjeon.com` works from another network
- [ ] Fingerprint and both connect commands published on the website
- [ ] Uptime checks for HTTPS, tcp/22 and tcp/2222 alerting
- [ ] Repo variables set; `DEPLOY_ENABLED=true`; one tag deploy and one rollback rehearsed
- [ ] Restart and reboot keep the same fingerprint
- [ ] Checked in macOS Terminal, another terminal app, and Windows Terminal/OpenSSH

## Teardown

Set the apex A record back to `216.198.79.1` and delete the `ssh` record first. Then `az group delete -n portfolio` removes the VM, disk, network and static IP. A new deployment gets a new IP.

## Local development

### Native

```sh
cargo build --locked
cargo run --locked -- serve --bind 127.0.0.1:2222 --host-key data/ssh_host_ed25519_key
```

In another terminal:

```sh
ssh -p 2222 localhost
```

The first connection asks whether to trust the server's host key. Compare the displayed SHA256 fingerprint against the public key derived directly from the server's local key:

```sh
ssh-keygen -y -f data/ssh_host_ed25519_key | ssh-keygen -lf -
```

Accept only if it matches. Keep normal OpenSSH host verification enabled. A later host-key change deserves investigation; do not solve it by routinely disabling verification or clearing `known_hosts`.

The key is created on first start with private permissions. The application fails on an invalid existing key rather than silently replacing the server identity.

### Repeatable local verification

Python 3 and OpenSSH are enough; the smoke checks install nothing and contact only a listener they start on localhost. The port must be free.

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
python3 -m unittest discover -s tests -p 'test_*.py' -v
python3 scripts/smoke.py --ssh
```

The harness uses actual PTYs and OpenSSH connections. It records text screens, raw terminal output, and server logs in ignored `target/smoke/`. It tests independent visitors, terminal resizing, abrupt disconnects, clean quits, repeated connections, rejected requests, and a restart with the same host key. It trusts only the key generated by its own test server, using an isolated `known_hosts` file with strict checking. These are local checks; they do not prove public-network reachability or every terminal application's rendering.

### Local container

The root `compose.yaml` builds the image and maps the port to `127.0.0.1:2222` only. It uses the same hardening as production.

```sh
docker compose config --quiet
docker compose build
docker compose up -d
docker compose ps
ssh -p 2222 localhost
docker compose exec -T portfolio sh -c 'ssh-keygen -y -f /data/ssh_host_ed25519_key | ssh-keygen -lf -'
docker compose restart portfolio && ssh -p 2222 localhost   # no changed-key warning
```

`docker compose down` keeps the key volume; `down -v` deletes it. Its stop signal is SIGINT, matching the server's shutdown handler. [Docker documents these service settings](https://docs.docker.com/reference/compose-file/services/).

### Local verification record

An earlier local run (Docker Desktop, Linux/ARM64 image) passed `docker compose config --quiet`, the image build, and these runtime checks in an isolated Compose project:

- The SSH handshake health check became healthy.
- The process ran as UID/GID 10001, with a read-only root filesystem and all capabilities dropped. A write to the root filesystem failed as expected.
- The generated key belonged to UID/GID 10001 with mode 0600 on the persistent volume.
- Two real OpenSSH PTYs navigated independently. Project links, 80×24 to 40×16 resizing, `q`/`Ctrl+C`, and terminal restoration worked.
- Restarting the container preserved its public key and accepted a new connection with strict host-key verification.
- The localhost port binding, 256 MiB memory limit, half-CPU allocation, and 64-process limit were present in the running container.

Still unverified: Linux/AMD64 at runtime, the production compose stack, Caddy, image rollback, public-network access and production load.
