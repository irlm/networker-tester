FROM php:8.3-cli

# swoole's pecl build probes for brotli (libbrotlienc) and aborts configure
# when the dev package is missing (found by the wave-2 validation run).
RUN apt-get update \
    && apt-get install -y --no-install-recommends libbrotli-dev libssl-dev pkg-config \
    && pecl install swoole \
    && docker-php-ext-enable swoole \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY server.php .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

CMD ["php", "server.php"]
