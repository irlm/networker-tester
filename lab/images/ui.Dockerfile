# syntax=docker/dockerfile:1.7
# ─── Optional UI: dashboard SPA (Vite build) behind nginx ────────────────────
ARG NODE_IMAGE=node:22-alpine
FROM ${NODE_IMAGE} AS build
WORKDIR /app
COPY dashboard/package.json dashboard/package-lock.json ./
RUN --mount=type=cache,target=/root/.npm npm ci --no-audit --no-fund
COPY dashboard/ ./
COPY shared /shared
RUN npm run build

FROM nginx:1.27-alpine
COPY --from=build /app/dist /usr/share/nginx/html
COPY lab/images/ui/nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
