param(
    [switch]$Delete,
    [switch]$Watch
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path

function Write-Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Write-Ok($msg)   { Write-Host "    $msg" -ForegroundColor Green }
function Write-Err($msg)  { Write-Host "    ERROR: $msg" -ForegroundColor Red }

if ($Delete) {
    Write-Step "Tearing down pulsar-relay..."
    kubectl delete -f "$ScriptDir\ingress.yaml"    --ignore-not-found
    kubectl delete -f "$ScriptDir\service.yaml"    --ignore-not-found
    kubectl delete -f "$ScriptDir\hpa.yaml"        --ignore-not-found
    kubectl delete -f "$ScriptDir\deployment.yaml" --ignore-not-found
    Write-Ok "Done."
    exit 0
}

Write-Step "Deploying pulsar-relay to Kubernetes..."

Write-Step "Applying deployment + config..."
kubectl apply -f "$ScriptDir\deployment.yaml"
if (-not $?) { Write-Err "deployment.yaml failed"; exit 1 }

Write-Step "Applying services..."
kubectl apply -f "$ScriptDir\service.yaml"
if (-not $?) { Write-Err "service.yaml failed"; exit 1 }

Write-Step "Applying ingress..."
kubectl apply -f "$ScriptDir\ingress.yaml"
if (-not $?) { Write-Err "ingress.yaml failed"; exit 1 }

if (Test-Path "$ScriptDir\hpa.yaml") {
    Write-Step "Applying HPA..."
    kubectl apply -f "$ScriptDir\hpa.yaml"
}

Write-Step "Waiting for rollout..."
kubectl rollout status deployment/pulsar-relay --timeout=120s
if (-not $?) { Write-Err "Rollout failed or timed out"; exit 1 }

Write-Ok "Rollout complete."
Write-Step "Current state:"
kubectl get deployment,svc,ingress -l app=pulsar-relay

if ($Watch) {
    Write-Step "Watching pods..."
    kubectl get pods -l app=pulsar-relay -w
}
