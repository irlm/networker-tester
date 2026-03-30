FROM php:8.3-cli

RUN pecl install swoole && docker-php-ext-enable swoole

WORKDIR /app

COPY server.php .

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

CMD ["php", "server.php"]
