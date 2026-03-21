# Architecture

This repository has two closely related runtime paths: the direct probe path and the dashboard
control-plane path.

## System Overview

```mermaid
flowchart LR
    subgraph ProbePath["Direct probe path"]
        T["networker-tester CLI<br/>crates/networker-tester"]
        E["networker-endpoint<br/>crates/networker-endpoint"]
        O["Artifacts<br/>JSON / HTML / Excel / DB"]
        T -->|"TCP / HTTP1 / HTTP2 / HTTP3 / UDP"| E
        T --> O
    end

    subgraph ControlPlane["Dashboard control plane"]
        B["Browser SPA<br/>dashboard/"]
        D["networker-dashboard<br/>crates/networker-dashboard"]
        A["networker-agent<br/>crates/networker-agent"]
        TP["networker-tester subprocess"]
        P["PostgreSQL"]
        C["networker-common<br/>shared messages"]

        B <-->|"HTTP + WebSocket"| D
        D <-->|"Agent WebSocket"| A
        A --> TP
        D --> P
        D --- C
        A --- C
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
    Tester->>Endpoint: execute probes over TCP / HTTP / HTTP3 / UDP
    Endpoint-->>Tester: responses and timing signals
    Tester->>Output: write JSON / HTML / Excel / DB output
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

    Browser->>Dashboard: create job / view run
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

    Crates --> Tester["networker-tester"]
    Crates --> Endpoint["networker-endpoint"]
    Crates --> Dashboard["networker-dashboard"]
    Crates --> Agent["networker-agent"]
    Crates --> Common["networker-common"]
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
