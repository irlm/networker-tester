FROM ruby:3.3-slim

WORKDIR /app

COPY Gemfile .
# puma builds a native extension; ruby:3.3-slim ships no compiler toolchain,
# which made `bundle install` exit 5 and killed the whole validation matrix
# for 14 straight weeks (audit V1). libssl-dev is REQUIRED at gem-build time:
# without it puma's MiniSSL ext silently compiles out and `bind ssl://` dies
# at boot with "Puma compiled without SSL support" (found by the wave-2
# validation run). Purge only the compiler afterward -- libssl runtime stays.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential libssl-dev \
    && bundle install --quiet \
    && apt-get purge -y --auto-remove build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY config.ru puma.rb ./

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

CMD ["bundle", "exec", "puma", "-C", "puma.rb", "config.ru"]
