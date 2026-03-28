FROM node:22-slim

WORKDIR /app

# Copy source (no npm install needed — zero dependencies)
COPY server.js package.json ./

# Default cert location; mount or override with BENCH_CERT_DIR
ENV BENCH_CERT_DIR=/opt/bench
ENV PORT=8443

EXPOSE 8443

CMD ["node", "server.js"]
