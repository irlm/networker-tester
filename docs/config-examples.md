# Config Examples

The repository keeps sample JSON files in
[`examples/configs/`](../examples/configs/). Copy a file and edit the copy. Do
not edit the sample file in place.

## Tester Configs

- [`examples/configs/tester.example.json`](../examples/configs/tester.example.json)
  A minimal CLI config for direct `networker-tester --config ...` use.
- [`examples/configs/networker-cloud.example.json`](../examples/configs/networker-cloud.example.json)
  An example of the generated cloud-target format. The installer writes this
  format to a local `networker-cloud.json`.

Run:

```bash
./target/release/networker-tester --config examples/configs/tester.example.json
```

## Endpoint Configs

- [`examples/configs/endpoint.example.json`](../examples/configs/endpoint.example.json)
  The basic endpoint server ports and the log level.

Run:

```bash
./target/release/networker-endpoint --config examples/configs/endpoint.example.json
```

## Deploy Configs

- [`examples/configs/deploy.example.json`](../examples/configs/deploy.example.json)
  A minimal deploy file for a local tester and one LAN endpoint.
- [`examples/configs/deploy-lan.json`](../examples/configs/deploy-lan.json)
  A multi-endpoint LAN deployment with a remote tester host.
- [`examples/configs/deploy-multi-cloud.json`](../examples/configs/deploy-multi-cloud.json)
  A side-by-side Azure, AWS, and GCP endpoint deployment.
- [`examples/configs/deploy-test-3cloud.json`](../examples/configs/deploy-test-3cloud.json)
  A three-cloud comparison with a local tester and a broader mode set.
- [`examples/configs/deploy-6ep-bench.json`](../examples/configs/deploy-6ep-bench.json)
  A larger benchmark matrix across six endpoints.

Run:

```bash
bash install.sh --deploy examples/configs/deploy.example.json
```

## Notes

- The installer generates `networker-cloud.json` as an output artifact. It writes
  this file to the current working directory or to the remote tester home
  directory during a deployment.
- The `networker-cloud.example.json` file in the repository is a reference format
  only.
- CLI flags override the values from the tester and endpoint config files.
