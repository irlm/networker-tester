# networker-integration-tests

Private integration tests for [networker-tester](https://github.com/irlm/networker-tester).

These tests spin up real Azure/AWS VMs and run end-to-end installer and probe scenarios.
Unit tests for `install.sh` live in the public repo under `tests/`.

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
