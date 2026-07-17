# nginx baseline image — API-SPEC.md transport subset (see nginx.conf).
#
# nginx:1.27 (mainline, nginx.org packaging) ships http_v2 + http_v3, matching
# what the orchestrator / install.sh install on benchmark VMs.
#
# A self-signed fallback cert is baked at build time so the image runs
# standalone and `nginx -t` can gate the build. Real deployments mount the
# shared benchmark certs as individual files:
#   -v cert.pem:/opt/bench/cert.pem:ro -v key.pem:/opt/bench/key.pem:ro
# (Do NOT mount a volume over all of /opt/bench — it would hide the
# pre-generated /opt/bench/download payload files.)
#
# The shared bench-data.json mount used by the app-language images is N/A
# here: nginx serves no /api/* workloads (API-SPEC.md §9).
FROM nginx:1.27-alpine

RUN apk add --no-cache openssl \
 && mkdir -p /opt/bench/download \
 && openssl req -x509 -newkey rsa:2048 -keyout /opt/bench/key.pem \
      -out /opt/bench/cert.pem -days 3650 -nodes -subj '/CN=bench' 2>/dev/null

COPY generate-download-files.sh /usr/local/bin/generate-download-files.sh
RUN sh /usr/local/bin/generate-download-files.sh /opt/bench/download

COPY nginx.conf /etc/nginx/nginx.conf
# Config syntax gate: fail the build, not the container start.
RUN nginx -t

COPY docker-entrypoint-bench.sh /usr/local/bin/docker-entrypoint-bench.sh
RUN chmod +x /usr/local/bin/docker-entrypoint-bench.sh /usr/local/bin/generate-download-files.sh

EXPOSE 8443/tcp 8443/udp

ENTRYPOINT ["/usr/local/bin/docker-entrypoint-bench.sh"]
CMD ["nginx", "-g", "daemon off;"]
