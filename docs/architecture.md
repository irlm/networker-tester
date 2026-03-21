# Architecture

This repository has two closely related runtime paths: the direct probe path and the dashboard
control-plane path.

## System Overview

```mermaid
flowchart LR
    subgraph ProbePath["Direct probe path"]
        T["networker-tester CLI"]
        E["networker-endpoint"]
        O["Artifacts: JSON, HTML, Excel, DB"]
        T -->|"TCP, HTTP1, HTTP2, HTTP3, UDP"| E
        T --> O
    end

    subgraph ControlPlane["Dashboard control plane"]
        B["Browser SPA"]
        D["networker-dashboard"]
        A["networker-agent"]
        TP["networker-tester subprocess"]
        P["PostgreSQL"]
        C["networker-common"]

        B -->|"HTTP and WebSocket"| D
        D -->|"UI responses and live updates"| B
        D -->|"Agent WebSocket"| A
        A -->|"Status and results"| D
        A --> TP
        D --> P
        D -.->|"shared messages"| C
        A -.->|"shared messages"| C
    end
```

## Main Components

| Path | Role |
|------|------|
| `crates/networker-tester` | Probe engine and CLI. Runs the protocol tests and writes artifacts. |
| `crates/networker-endpoint` | Target HTTP/HTTPS/UDP service used for controlled measurements. |
| `crates/networker-dashboard` | REST API, WebSocket hubs, auth, scheduling, deploy orchestration, static frontend hosting. |
| `crates/networker-agent` | Worker process that receives jobs and runs `networker-tester`. |
| `crates/networker-common` | Shared message and protocol types used by dashboard and agent. |
| `dashboard/` | React SPA for the browser UI. |

## Runtime Flows

### Direct CLI Flow

```mermaid
sequenceDiagram
    participant User
    participant Tester as networker-tester
    participant Endpoint as networker-endpoint
    participant Output as Artifacts

    User->>Tester: run CLI with target, modes, and output options
    Tester->>Endpoint: execute probes over TCP, HTTP, HTTP3, and UDP
    Endpoint-->>Tester: responses and timing signals
    Tester->>Output: write JSON, HTML, Excel, and DB output
    Tester-->>User: terminal summary and artifact locations
```

1. `networker-tester` targets one or more URLs or hosts.
2. It runs the selected probes against `networker-endpoint` or another compatible target.
3. It writes artifacts such as JSON, HTML, Excel, and optional DB output.

### Dashboard-Managed Flow

```mermaid
sequenceDiagram
    participant Browser
    participant Dashboard as networker-dashboard
    participant DB as PostgreSQL
    participant Agent as networker-agent
    participant Tester as networker-tester
    participant Endpoint as networker-endpoint

    Browser->>Dashboard: create job or view run
    Dashboard->>DB: persist job metadata
    Dashboard-->>Agent: dispatch job over agent WebSocket
    Agent->>Tester: spawn probe run
    Tester->>Endpoint: execute probes
    Endpoint-->>Tester: responses and telemetry
    Tester-->>Agent: attempts and artifacts
    Agent-->>Dashboard: live results + status
    Dashboard->>DB: persist runs and attempts
    Dashboard-->>Browser: stream live updates over browser WebSocket
```

1. A browser connects to the dashboard UI.
2. The React SPA talks to `networker-dashboard` over HTTP and WebSocket.
3. The dashboard dispatches work to one or more `networker-agent` workers.
4. Each agent runs `networker-tester` jobs locally and streams results back.
5. The dashboard persists state in PostgreSQL and fans live updates back to browsers.

## What Lives Where

```mermaid
flowchart TD
    Repo["networker-tester repo"]
    Repo --> Crates["crates/"]
    Repo --> Frontend["dashboard/"]
    Repo --> Docs["docs/"]
    Repo --> Samples["examples/configs/"]
    Repo --> Tests["tests/"]

    Crates --> Tester["networker-tester crate"]
    Crates --> Endpoint["networker-endpoint crate"]
    Crates --> Dashboard["networker-dashboard crate"]
    Crates --> Agent["networker-agent crate"]
    Crates --> Common["networker-common crate"]
```

## Reading Order For New Contributors

1. Read the root [`README.md`](../README.md) for the product overview and quick start.
2. Read [`installation.md`](installation.md) to build and run the core binaries.
3. Read [`probes.md`](probes.md) to understand which modes map to which measurements.
4. Read [`testing.md`](testing.md) for reproducible workflows and report interpretation.
5. Read [`deploy-config.md`](deploy-config.md) if you are working on installer-driven deployment.

## Where to Read Next

- [`installation.md`](installation.md)
- [`probes.md`](probes.md)
- [`deploy-config.md`](deploy-config.md)
- [`testing.md`](testing.md)
