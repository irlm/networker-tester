#!/usr/bin/env bash
set -euo pipefail

# Deploy the two public LagHound SDK reference apps to the smallest Azure
# Container Apps Consumption allocation. Images must already be available from
# a public registry so this deployment does not need a paid Azure Container
# Registry. See README.md in this directory for build/push commands.

: "${CSHARP_IMAGE:?Set CSHARP_IMAGE to a public container image URI}"
: "${RUST_IMAGE:?Set RUST_IMAGE to a public container image URI}"
: "${LAGHOUND_TOKEN:?Set LAGHOUND_TOKEN to the public demo token (at least 16 bytes)}"

if [ "${#LAGHOUND_TOKEN}" -lt 16 ]; then
  echo "LAGHOUND_TOKEN must be at least 16 bytes" >&2
  exit 1
fi

if ! command -v az >/dev/null 2>&1; then
  echo "Azure CLI (az) is required" >&2
  exit 1
fi

AZURE_LOCATION="${AZURE_LOCATION:-eastus2}"
RESOURCE_GROUP="${RESOURCE_GROUP:-laghound-sdk-demos}"
CONTAINER_ENV="${CONTAINER_ENV:-laghound-sdk-demos}"
CSHARP_APP="${CSHARP_APP:-laghound-csharp-demo}"
RUST_APP="${RUST_APP:-laghound-rust-demo}"

az account show --output none
az extension add --name containerapp --upgrade --only-show-errors
az provider register --namespace Microsoft.App --wait

az group create \
  --name "$RESOURCE_GROUP" \
  --location "$AZURE_LOCATION" \
  --output none

if ! az containerapp env show --name "$CONTAINER_ENV" --resource-group "$RESOURCE_GROUP" --output none 2>/dev/null; then
  az containerapp env create \
    --name "$CONTAINER_ENV" \
    --resource-group "$RESOURCE_GROUP" \
    --location "$AZURE_LOCATION" \
    --logs-destination none \
    --output none
fi

deploy_app() {
  app_name="$1"
  image="$2"

  if az containerapp show --name "$app_name" --resource-group "$RESOURCE_GROUP" --output none 2>/dev/null; then
    az containerapp secret set \
      --name "$app_name" \
      --resource-group "$RESOURCE_GROUP" \
      --secrets "laghound-token=$LAGHOUND_TOKEN" \
      --output none

    az containerapp update \
      --name "$app_name" \
      --resource-group "$RESOURCE_GROUP" \
      --image "$image" \
      --cpu 0.25 \
      --memory 0.5Gi \
      --min-replicas 0 \
      --max-replicas 1 \
      --set-env-vars PORT=8080 LAGHOUND_PUBLIC_DEMO=1 LAGHOUND_TOKEN=secretref:laghound-token \
      --output none
  else
    az containerapp create \
      --name "$app_name" \
      --resource-group "$RESOURCE_GROUP" \
      --environment "$CONTAINER_ENV" \
      --image "$image" \
      --ingress external \
      --target-port 8080 \
      --transport auto \
      --cpu 0.25 \
      --memory 0.5Gi \
      --min-replicas 0 \
      --max-replicas 1 \
      --secrets "laghound-token=$LAGHOUND_TOKEN" \
      --env-vars PORT=8080 LAGHOUND_PUBLIC_DEMO=1 LAGHOUND_TOKEN=secretref:laghound-token \
      --output none
  fi
}

deploy_app "$CSHARP_APP" "$CSHARP_IMAGE"
deploy_app "$RUST_APP" "$RUST_IMAGE"

csharp_fqdn="$(az containerapp show --name "$CSHARP_APP" --resource-group "$RESOURCE_GROUP" --query properties.configuration.ingress.fqdn --output tsv)"
rust_fqdn="$(az containerapp show --name "$RUST_APP" --resource-group "$RESOURCE_GROUP" --query properties.configuration.ingress.fqdn --output tsv)"

echo
echo "C# demo:  https://$csharp_fqdn"
echo "Rust demo: https://$rust_fqdn"
echo
echo "Dashboard build variables:"
echo "VITE_LAGHOUND_CSHARP_DEMO_URL=https://$csharp_fqdn"
echo "VITE_LAGHOUND_RUST_DEMO_URL=https://$rust_fqdn"
echo "VITE_LAGHOUND_PUBLIC_DEMO_TOKEN=<same value as LAGHOUND_TOKEN>"
