# LagHound JS/Node sample — bare node:http, zero runtime deps.
# Build context MUST be the repo root: `docker build -f examples/js.Dockerfile .`
# The sample runs the SDK's TypeScript source directly (Node >= 22.6 strips
# types), so no build step is required — just copy the SDK and run.
FROM node:22-slim AS runtime
WORKDIR /sdk

# Copy the whole JS SDK tree: the example loads ../src/index.ts at runtime.
COPY sdk/js ./

WORKDIR /sdk/example

# Container listens on 8082 internally; compose maps it to the host port.
# LAGHOUND_TOKEN comes from the environment.
ENV PORT=8082
EXPOSE 8082
CMD ["node", "server.mjs"]
