# Config Examples

Checked-in sample JSON files live in [`examples/configs/`](../examples/configs/).
Use them as copy-and-edit starting points rather than editing them in place.

## Tester Configs

- [`examples/configs/tester.example.json`](../examples/configs/tester.example.json)
  Minimal CLI config for direct `networker-tester --config ...` usage.
- [`examples/configs/networker-cloud.example.json`](../examples/configs/networker-cloud.example.json)
  Example of the generated cloud-target format that the installer writes to a local
  `networker-cloud.json`.

Run:

```bash
./target/release/networker-tester --config examples/configs/tester.example.json
```

## Endpoint Configs

- [`examples/configs/endpoint.example.json`](../examples/configs/endpoint.example.json)
  Basic endpoint server ports and log level.

Run:

```bash
./target/release/networker-endpoint --config examples/configs/endpoint.example.json
```

## Deploy Configs

- [`examples/configs/deploy.example.json`](../examples/configs/deploy.example.json)
  Minimal deploy file for a local tester and one LAN endpoint.
- [`examples/configs/deploy-lan.json`](../examples/configs/deploy-lan.json)
  Multi-endpoint LAN deployment with a remote tester host.
- [`examples/configs/deploy-multi-cloud.json`](../examples/configs/deploy-multi-cloud.json)
  Side-by-side Azure, AWS, and GCP endpoint deployment.
- [`examples/configs/deploy-test-3cloud.json`](../examples/configs/deploy-test-3cloud.json)
  Three-cloud comparison with a local tester and a broader mode set.
- [`examples/configs/deploy-6ep-bench.json`](../examples/configs/deploy-6ep-bench.json)
  Larger benchmark matrix across six endpoints.

Run:

```bash
bash install.sh --deploy examples/configs/deploy.example.json
```

## Notes

- The installer-generated `networker-cloud.json` is an output artifact written to the current
  working directory or remote tester home directory during deployment flows.
- The checked-in `networker-cloud.example.json` exists only as a reference format.
- CLI flags override values loaded from tester and endpoint config files.
