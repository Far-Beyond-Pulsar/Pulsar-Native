#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cyan='\033[0;36m'; green='\033[0;32m'; red='\033[0;31m'; reset='\033[0m'
step()  { echo -e "\n${cyan}==> $*${reset}"; }
ok()    { echo -e "    ${green}$*${reset}"; }
err()   { echo -e "    ${red}ERROR: $*${reset}"; exit 1; }

DELETE=0
WATCH=0
for arg in "$@"; do
    case $arg in
        --delete|-d) DELETE=1 ;;
        --watch|-w)  WATCH=1  ;;
    esac
done

if [[ $DELETE -eq 1 ]]; then
    step "Tearing down pulsar-relay..."
    kubectl delete -f "$SCRIPT_DIR/ingress.yaml"    --ignore-not-found
    kubectl delete -f "$SCRIPT_DIR/service.yaml"    --ignore-not-found
    kubectl delete -f "$SCRIPT_DIR/hpa.yaml"        --ignore-not-found
    kubectl delete -f "$SCRIPT_DIR/deployment.yaml" --ignore-not-found
    ok "Done."
    exit 0
fi

step "Deploying pulsar-relay to Kubernetes..."

step "Applying deployment + config..."
kubectl apply -f "$SCRIPT_DIR/deployment.yaml" || err "deployment.yaml failed"

step "Applying services..."
kubectl apply -f "$SCRIPT_DIR/service.yaml" || err "service.yaml failed"

step "Applying ingress..."
kubectl apply -f "$SCRIPT_DIR/ingress.yaml" || err "ingress.yaml failed"

if [[ -f "$SCRIPT_DIR/hpa.yaml" ]]; then
    step "Applying HPA..."
    kubectl apply -f "$SCRIPT_DIR/hpa.yaml"
fi

step "Waiting for rollout..."
kubectl rollout status deployment/pulsar-relay --timeout=120s || err "Rollout failed or timed out"

ok "Rollout complete."
step "Current state:"
kubectl get deployment,svc,ingress -l app=pulsar-relay

if [[ $WATCH -eq 1 ]]; then
    step "Watching pods..."
    kubectl get pods -l app=pulsar-relay -w
fi
