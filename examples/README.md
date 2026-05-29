# Submux examples

Observability assets for running submux against a Grafana/Prometheus stack
(e.g. the `lgtm-autostart` compose stack).

```
examples/
├── grafana/
│   └── submux-overview.json          # Grafana dashboard (import or auto-provisioned)
└── observability/
    ├── compose.yaml                  # one-command stack: collector + Prometheus + Grafana
    ├── otel-collector.yaml           # OTLP receive → Prometheus remote-write
    ├── prometheus-compose.yml        # Prometheus config used by compose
    ├── prometheus.yml                # standalone scrape config (alternative)
    └── grafana/provisioning/         # auto-provisioned datasource + dashboard
```

## 0. Quick start (Docker Compose)

Bring up the whole stack — OTel Collector, Prometheus, and Grafana with the
dashboard and datasource pre-provisioned:

```bash
docker compose -f examples/observability/compose.yaml up -d   # podman compose also works
```

Run submux pointed at the collector, then open Grafana:

```bash
SUBMUX_OTLP_ENDPOINT=http://localhost:4318 submux
# Grafana → http://localhost:3000 → "Submux — Gateway Overview"
```

If you already run your own LGTM stack, skip this and use section 1 instead.

## 1. Get submux metrics into Prometheus

Submux uses the OpenTelemetry SDK with two readers off one instrument set:

- **OTLP push** to a collector — enabled by setting `SUBMUX_OTLP_ENDPOINT`.
- **Prometheus scrape** at `GET /metrics` — always on.

### Option A — OTLP push (recommended; matches the lgtm-autostart stack)

Point submux at the collector's OTLP/HTTP receiver and run it:

```bash
SUBMUX_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_METRIC_EXPORT_INTERVAL=15000 \
  submux
```

Metrics flow submux → collector `:4318` → `prometheusremotewrite` → Prometheus.
No scrape job needed. Confirm with:
`curl -s 'http://localhost:9090/api/v1/query?query=submux_requests_total'`.

### Option B — Prometheus scrapes `/metrics` directly

If you'd rather scrape, mount
[`observability/prometheus.yml`](observability/prometheus.yml) into the
Prometheus service (it already runs with
`--config.file=/etc/prometheus/prometheus.yml`):

```yaml
  prometheus:
    volumes:
      - /ABS/PATH/TO/submux/examples/observability/prometheus.yml:/etc/prometheus/prometheus.yml
    # Linux only (Docker Desktop resolves host.docker.internal automatically):
    # extra_hosts: ["host.docker.internal:host-gateway"]
```

Both paths produce the same metric names, so the dashboard works with either.

## 1a. Per-consumer attribution

The calling client identifies itself with a stable
`X-Proxy-User-Id: <whoami>_<uuid>` header; submux stamps it as the `consumer`
label on every metric. Generate the id once per machine and point Claude Code at
submux:

```bash
# one-time per machine — generate and persist the id
echo "$(whoami)_$(uuidgen)" > ~/.submux_consumer_id

export ANTHROPIC_BASE_URL=http://your-submux:8080
export ANTHROPIC_CUSTOM_HEADERS="X-Proxy-User-Id: $(cat ~/.submux_consumer_id)"
claude
```

(Or persist it under `"env"` in `~/.claude/settings.json`.) Requests without —
or with a malformed — header are bucketed as `consumer="unknown"`.

## 2. Import the dashboard

In Grafana: **Dashboards → New → Import → Upload JSON file**, pick
[`grafana/submux-overview.json`](grafana/submux-overview.json), and select your
Prometheus datasource when prompted (`DS_PROMETHEUS`).

The dashboard shows request rate / success ratio / latency quantiles (by
protocol & model), error breakdown by status, per-account quota utilization,
cooldown state, in-flight requests, and credential-refresh attempts.

## Metric reference

| Metric | Type | Labels |
|---|---|---|
| `submux_requests_total` | counter | `protocol`, `model_group`, `status`, `consumer` |
| `submux_request_duration_seconds` | histogram | `protocol`, `model_group`, `consumer` |
| `submux_tokens_total` | counter | `consumer`, `protocol`, `direction` (`input`/`output`), `model` |
| `submux_account_quota_utilization` | gauge | `account_id`, `window` (`5h`/`7d`) |
| `submux_account_cooldown_active` | gauge | `account_id` |
| `submux_refresh_attempts_total` | counter | `account_id`, `result` |

> `consumer` is the **inbound** identity (`X-Proxy-User-Id`, the calling
> machine/user); `account_id` is the **upstream** subscription that served the
> request. Token metering covers the `/v1/messages` and `/codex/*` streaming
> paths.
