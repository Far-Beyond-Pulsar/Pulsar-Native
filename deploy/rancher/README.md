# Pulsar Relay — Rancher Deployment Guide

Deploy the Pulsar relay server on **Rancher** (or any Kubernetes cluster) using the manifests in this directory. The relay handles WebSocket notification delivery, QUIC peer-to-peer connections, and UDP hole-punching for multiplayer sessions.

---

## Prerequisites

- A **Kubernetes cluster** managed by Rancher (v2.7+)
- **Rancher UI access** (or `kubectl` if you prefer the CLI)
- A **container registry** where the `pulsar-relay` image is published
  (e.g., Docker Hub, GitHub Container Registry, or your private registry)

---

## ⚙️ Before You Deploy — Configure Secrets

**You must generate secrets** before deploying. Do not use the default values.

### 1. JWT Secret

Generate a strong random secret:

```bash
# Linux / macOS
openssl rand -base64 32
```

```powershell
# Windows PowerShell
[Convert]::ToBase64String((1..32 | ForEach-Object { Get-Random -Max 256 }))
```

### 2. Ed25519 Server Key (optional but recommended)

Persisting this key keeps session tokens valid across restarts. If you skip it, a new key is generated every time the pod restarts, invalidating all active sessions.

```bash
openssl rand -hex 32
```

### 3. Update `secret.yaml`

Replace the placeholder values in `deploy/rancher/secret.yaml`:

```yaml
stringData:
  jwt-secret: "your-generated-base64-secret"
  server-ed25519-key: "your-generated-64-char-hex-key"
```

---

## 🚀 Deploy via Rancher UI (No kubectl Required)

### Step 1 — Import the YAML

1. Open your **Rancher UI** and navigate to the cluster you want to deploy to.
2. Click the **⨁ Import YAML** button in the top-right toolbar (or go to **Resources > Workloads > Import YAML**).
3. A dialog opens — paste the full contents of **each file** in order:

   | Order | File | Purpose |
   |---|---|---|
   | 1 | `namespace.yaml` | Creates the `pulsar-relay` namespace |
   | 2 | `configmap.yaml` | Non-sensitive env vars (log level, bind addresses) |
   | 3 | `secret.yaml` | ⚠️ Sensitive — JWT secret, Ed25519 key |
   | 4 | `deployment.yaml` | The relay pod + headless service |
   | 5 | `service.yaml` | LoadBalancer + NetworkPolicy |

4. Click **Import** after each file. Watch for green "Active" status.

### Step 2 — Verify the Deployment

1. In the left nav, go to **Workloads > Deployments**.
2. You should see `pulsar-relay` with **1/1** pods running.
3. Click the deployment name to view pod logs — you should see:

   ```
   INFO pulsar_relay: 🚀 Starting all services...
   INFO pulsar_relay: 🌐 Starting HTTP server on 0.0.0.0:8080...
   INFO pulsar_relay: ⚡ Starting QUIC relay on 0.0.0.0:8443...
   INFO pulsar_relay: 🌐 HTTP server ready - accepting connections
   ```

### Step 3 — Check the Service Endpoint

1. Go to **Service Discovery > Services**.
2. Find `pulsar-relay`. The **Endpoints** column shows the external IP or hostname
   (this is where Pulsar clients will connect).
3. Test that the relay is responding:

   ```bash
   curl http://<EXTERNAL-IP>:8080/health/liveness
   ```

   Expected response: `{"status":"healthy"}`

---

## 🚀 Deploy via CLI (kubectl)

If you have `kubectl` configured with your cluster:

```bash
# Update the image name in deployment.yaml first, then:
kubectl apply -k deploy/rancher/
```

---

## 🔧 Configuration Reference

### Environment Variables

Set these via `configmap.yaml` or `secret.yaml`:

| Variable | Default | Description |
|---|---|---|
| `PULSAR_HTTP_BIND` | `0.0.0.0:8080` | HTTP admin API bind address |
| `PULSAR_QUIC_BIND` | `0.0.0.0:8443` | QUIC relay bind address |
| `PULSAR_UDP_BIND` | `0.0.0.0:7000` | UDP hole-punching bind address |
| `PULSAR_LOG_LEVEL` | `info` | Log level (trace/debug/info/warn/error) |
| `PULSAR_JWT_SECRET` | `change-this-secret-in-production` | ⚠️ JWT signing key |
| `PULSAR_SERVER_ED25519_KEY` | ephemeral (auto-generated) | Persistent Ed25519 key (hex, 64 chars) |
| `PULSAR_DATABASE_URL` | (none) | PostgreSQL connection string (optional) |
| `PULSAR_STORAGE_DIR` | (none) | Local snapshot storage directory (optional) |
| `PULSAR_TLS_CERT` | self-signed | TLS certificate path for QUIC |
| `PULSAR_TLS_KEY` | self-signed | TLS key path for QUIC |

### Ports

| Port | Protocol | Purpose |
|---|---|---|
| `8080` | TCP | HTTP API + WebSocket notifications + health checks |
| `8443` | UDP | QUIC relay for P2P connections |
| `7000` | UDP | UDP hole-punching (NAT traversal) |

### Health Checks

| Endpoint | Purpose |
|---|---|
| `GET /health/liveness` | Pod is alive (used by Kubernetes liveness probe) |
| `GET /health/readiness` | Pod is ready to accept traffic (used by readiness probe) |
| `GET /health` | Full health (DB, TLS, relay connections) |

---

## 📊 Monitoring (Rancher)

1. Go to the **pulsar-relay** deployment details page.
2. Click the **Metrics** tab to view CPU / memory usage.
3. The deployment includes liveness + readiness probes — Rancher will show
   **green** for healthy pods and **red** for failed probes.
4. For Prometheus metrics, configure a ServiceMonitor in Rancher to scrape
   `http://pulsar-relay:8080/metrics`.

---

## 🔁 Scaling

The default is **1 replica**. To scale:

- **Rancher UI**: Deployment details → **⋮ > Scale** → enter the number of replicas.
- **CLI**: `kubectl scale deployment/pulsar-relay -n pulsar-relay --replicas=3`

> **Note:** QUIC connections are stateful. For multi-replica setups, you may need
> session affinity (`service.spec.sessionAffinity: ClientIP`) or a Redis-backed
> session store (coming in a future release).

---

## 🐛 Troubleshooting

### Pod crashes immediately

Check the logs: **Workloads > Deployments > pulsar-relay > ⋮ > View Logs**.

Common causes:
- **Missing JWT secret**: The `secret.yaml` wasn't applied, or the values are empty.
- **Port conflict**: Another service is binding the same ports in the same namespace.
- **Image not found**: The image name in `deployment.yaml` doesn't exist in your registry.

### Health check fails

Run from a pod in the same cluster:

```bash
kubectl run -it --rm test-pod --image=curlimages/curl --restart=Never -- \
  curl -s http://pulsar-relay.pulsar-relay.svc:8080/health/liveness
```

Expected: `{"status":"healthy"}`

### Clients can't connect from outside the cluster

1. Verify the LoadBalancer has an external IP or hostname:
   **Service Discovery > Services > pulsar-relay > Endpoints**.
2. Check firewall/security group rules — the LoadBalancer must allow inbound
   traffic on TCP 8080, UDP 8443, and UDP 7000.
3. Try connecting from outside:

   ```bash
   curl http://<EXTERNAL-IP>:8080/health/liveness
   ```

---

## 📁 File Reference

| File | What it creates |
|---|---|
| `namespace.yaml` | `Namespace/pulsar-relay` |
| `configmap.yaml` | `ConfigMap/pulsar-relay-config` |
| `secret.yaml` | `Secret/pulsar-relay-secrets` |
| `deployment.yaml` | `Deployment/pulsar-relay` + Headless Service |
| `service.yaml` | `Service/pulsar-relay` (LoadBalancer) + `NetworkPolicy` |
| `kustomization.yaml` | Kustomize entry point (`kubectl apply -k deploy/rancher/`) |
