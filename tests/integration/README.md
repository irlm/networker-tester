# networker-integration-tests

Private integration tests for [networker-tester](https://github.com/irlm/networker-tester).

These tests spin up real Azure/AWS VMs and run end-to-end installer and probe scenarios.
Unit tests for `install.sh` live in the public repo under `tests/`.

## CI status: manual-only, but no longer unwatched

These 42 tests create billable cloud resources, so **CI never executes them** —
run them yourself with the commands below. What CI *does* do, on every PR that
touches the installer, is parse every suite (`bats --count`) and check that the
`install.sh` component names they invoke still exist. That is the cheap half of
the problem: between 2026-03 and 2026-08 nothing in the repo referenced these
files at all, so they could have stopped parsing and nobody would have found
out until someone ran them by hand (audit P1-15).

The Azure suites are the ones that could be automated further — `AZURE_CREDENTIALS`
already exists as a repo secret for deploys. The AWS suites cannot: there are no
AWS credentials in CI, and they are local-only by design.

Overlapping coverage that *does* run automatically: the weekly `Prod soak check`
canary provisions real Azure VMs through the control plane, including a
multi-cell proxy matrix, so the Azure deploy path is not unexercised.

## Structure

```
tests/
  integration/          ← this submodule
    azure/              — Azure VM end-to-end tests
    aws/                — AWS EC2 end-to-end tests
    helpers/            — shared helpers (SSH wrappers, VM lifecycle, assertions)
    run.sh              — entrypoint: ./run.sh [azure|aws|all]
```

## Requirements

- `bats-core` (`brew install bats-core`)
- Azure CLI logged in (`az login`) — for Azure tests
- AWS CLI configured (`aws configure`) — for AWS tests
- SSH agent running with key forwarded

## Running

```bash
# From the public repo root (after `git submodule update --init`):
cd tests/integration
./run.sh azure    # Azure end-to-end
./run.sh aws      # AWS end-to-end
./run.sh all      # both
```
