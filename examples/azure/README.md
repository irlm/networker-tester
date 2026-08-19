# Azure SDK reference demos

Deploy the C# and Rust sample apps as two public Azure Container Apps in one
shared Consumption environment. This is the smallest practical Azure shape for
real HTTP services that must run arbitrary .NET and Rust containers:

- `0.25` vCPU and `0.5Gi` memory per replica (the Consumption minimum)
- `min-replicas 0`, `max-replicas 1` (scale to zero between visits)
- no Log Analytics workspace
- public registry images, avoiding a dedicated Azure Container Registry
- transfer routes disabled and tighter rate limits on the public deployment

Azure's Consumption plan includes a monthly free grant and does not charge app
usage while an app is scaled to zero. Light demo traffic can fit inside that
grant, but this is not a promise of a zero bill: registry, network egress, DNS,
or traffic above the grant can still cost money. See the official
[Container Apps pricing](https://azure.microsoft.com/pricing/details/container-apps/)
and [scaling documentation](https://learn.microsoft.com/azure/container-apps/scale-app).

## 1. Build and publish the images

The Docker build context must be the repository root because each sample uses
its sibling SDK project. Use immutable tags rather than `latest`.

```bash
export IMAGE_OWNER="ghcr.io/<github-owner>"
export IMAGE_TAG="$(git rev-parse --short HEAD)"

docker login ghcr.io
docker buildx build --platform linux/amd64 \
  --file examples/csharp.Dockerfile \
  --tag "$IMAGE_OWNER/laghound-csharp-demo:$IMAGE_TAG" \
  --push .
docker buildx build --platform linux/amd64 \
  --file examples/rust.Dockerfile \
  --tag "$IMAGE_OWNER/laghound-rust-demo:$IMAGE_TAG" \
  --push .
```

Make both GHCR packages public so Azure can pull them without stored registry
credentials. Docker Hub or any other public OCI registry works too.

## 2. Deploy

Use a public demo token. It is intentionally shared with dashboard users who
choose **Use example**, so do not reuse a production secret. Public-demo mode
disables upload/download routes and lowers request limits.

```bash
export CSHARP_IMAGE="$IMAGE_OWNER/laghound-csharp-demo:$IMAGE_TAG"
export RUST_IMAGE="$IMAGE_OWNER/laghound-rust-demo:$IMAGE_TAG"
export LAGHOUND_TOKEN="replace-with-public-demo-token"

./examples/azure/deploy.sh
```

Optional deployment settings:

| Variable | Default |
|---|---|
| `AZURE_LOCATION` | `eastus2` |
| `RESOURCE_GROUP` | `laghound-sdk-demos` |
| `CONTAINER_ENV` | `laghound-sdk-demos` |
| `CSHARP_APP` | `laghound-csharp-demo` |
| `RUST_APP` | `laghound-rust-demo` |

The script is idempotent for normal image/token updates. It prints both Azure
FQDNs when deployment completes.

## Local container smoke test

Run the same container port and public-demo settings before pushing anything to
Azure. Docker must be running locally:

```bash
./examples/azure/run-local.sh
```

The harness builds both images, starts them with `docker run`, checks each
landing page, `/work`, token-gated `/laghound/health`, and confirms that the
public profile returns `404` for download. It removes only its two named test
containers when it exits. Override `CSHARP_PORT`, `RUST_PORT`, or
`LAGHOUND_TOKEN` if those defaults are already in use.

## 3. Put the live examples in the dashboard

Provide these at dashboard build time, using the FQDNs printed by the script:

```bash
export VITE_LAGHOUND_CSHARP_DEMO_URL="https://<csharp-fqdn>"
export VITE_LAGHOUND_RUST_DEMO_URL="https://<rust-fqdn>"
export VITE_LAGHOUND_PUBLIC_DEMO_TOKEN="$LAGHOUND_TOKEN"
```

When the URLs are absent, the SDK Endpoints page honestly shows the examples as
**Deploy ready** and links to their source. When configured, it shows **Live on
Azure**, opens each running page, and lets an operator prefill the registration
form with the example URL, route, and public token.

## Cold-start tradeoff

Scale-to-zero is the cheapest setting, but the first request after an idle
period has a cold start. Set `--min-replicas 1` if the examples must respond
immediately; Azure then bills idle allocation. For reference demos, the short
cold start is usually the better tradeoff.
