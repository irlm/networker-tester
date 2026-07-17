FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY server.py .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

# --no-access-log: per-request stderr I/O is measurement noise no other
# language pays (audit F11); CI already runs with logging off.
CMD ["uvicorn", "server:app", \
     "--host", "0.0.0.0", \
     "--port", "8443", \
     "--no-access-log", \
     "--ssl-keyfile", "/opt/bench/key.pem", \
     "--ssl-certfile", "/opt/bench/cert.pem"]
