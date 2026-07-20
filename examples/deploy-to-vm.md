# Deploy the multi-language harness to one cloud VM

Goal: run all five LagHound language samples on a single Azure / AWS / GCP VM so
a LagHound fleet can probe every language from one host. ~5 minutes.

## 1. Provision a VM

Any small Linux VM works (2 vCPU / 4 GB is plenty for all five samples):

| Cloud | Example |
|-------|---------|
| Azure | `az vm create -g rg -n laghound-demo --image Ubuntu2404 --size Standard_B2s --generate-ssh-keys` |
| AWS   | `t3.small`, Ubuntu 24.04 AMI |
| GCP   | `e2-small`, `ubuntu-2404-lts` |

## 2. Open ports 8081-8085 (inbound TCP)

Only needed if the fleet probes the samples directly (skip if you reverse-proxy — step 5).

| Cloud | Command |
|-------|---------|
| Azure | `az vm open-port -g rg -n laghound-demo --port 8081-8085 --priority 1001` |
| AWS   | Add an inbound rule to the security group: TCP `8081-8085` from the fleet's CIDR |
| GCP   | `gcloud compute firewall-rules create laghound --allow tcp:8081-8085 --source-ranges <fleet-cidr>` |

Prefer to scope the source range to the LagHound fleet egress IPs rather than `0.0.0.0/0`.

## 3. Install Docker

```bash
ssh azureuser@<vm-ip>
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER" && exec sg docker newgrp   # or log out/in
```

## 4. Clone and start

```bash
git clone https://github.com/irlm/networker-tester.git
cd networker-tester/examples

# Set a real token in production (contract requires >= 16 bytes):
echo "LAGHOUND_TOKEN=$(openssl rand -hex 24)" > .env

docker compose up --build -d      # detached
docker compose ps                 # all five should show (healthy)
./probe-all.sh                    # smoke test: 5 PASS
```

The five endpoints are now at `http://<vm-ip>:8081..8085/laghound/echo`. Point a
LagHound target at them with `--bearer-token <token>` (the value in `.env`).

## 5. (Optional) Reverse-proxy instead of opening five ports

Put nginx in front and expose one 443 with path prefixes — no need to open
8081-8085 to the internet:

```nginx
# /etc/nginx/sites-available/laghound-demo
server {
  listen 443 ssl;
  server_name demo.example.com;
  # ... ssl_certificate / ssl_certificate_key ...

  location /csharp/ { proxy_pass http://127.0.0.1:8081/; }
  location /js/     { proxy_pass http://127.0.0.1:8082/; }
  location /python/ { proxy_pass http://127.0.0.1:8083/; }
  location /rust/   { proxy_pass http://127.0.0.1:8084/; }
  location /go/     { proxy_pass http://127.0.0.1:8085/; }
}
```

Then targets probe e.g. `https://demo.example.com/go/laghound/echo`. Keep the
compose `ports:` bound to `127.0.0.1` only if you go this route (edit the
mappings to `"127.0.0.1:8085:8085"`).

## 6. Operate

```bash
docker compose logs -f laghound-go     # tail one service
docker compose restart laghound-rust   # bounce one
docker compose down                    # stop all (restart: unless-stopped survives reboots otherwise)
```
