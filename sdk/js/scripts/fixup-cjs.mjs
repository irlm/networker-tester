// Stamp dist/cjs as CommonJS so require() resolves it despite the package
// being "type": "module".
import { writeFileSync, mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const dir = fileURLToPath(new URL("../dist/cjs/", import.meta.url));
mkdirSync(dir, { recursive: true });
writeFileSync(dir + "package.json", JSON.stringify({ type: "commonjs" }) + "\n");
