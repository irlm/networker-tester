# LagHound Python sample — stdlib wsgiref + the laghound SDK (zero third-party deps).
# Build context MUST be the repo root: `docker build -f examples/python.Dockerfile .`
FROM python:3.12-slim AS runtime
WORKDIR /app

# Copy the SDK package source and the sample app. The SDK is pure stdlib, so we
# just put src/ on PYTHONPATH rather than pip-installing (keeps the image tiny).
COPY sdk/python/src ./src
COPY sdk/python/example ./example

ENV PYTHONPATH=/app/src
ENV PYTHONUNBUFFERED=1

# Container listens on 8083 internally; compose maps it to the host port.
# LAGHOUND_TOKEN comes from the environment.
ENV PORT=8083
EXPOSE 8083
WORKDIR /app/example
CMD ["python3", "app.py"]
