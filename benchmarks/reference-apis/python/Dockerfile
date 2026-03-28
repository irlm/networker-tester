FROM python:3.12-slim

WORKDIR /app

COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY server.py .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

CMD ["uvicorn", "server:app", \
     "--host", "0.0.0.0", \
     "--port", "8443", \
     "--ssl-keyfile", "/opt/bench/key.pem", \
     "--ssl-certfile", "/opt/bench/cert.pem"]
