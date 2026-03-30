FROM ruby:3.3-slim

WORKDIR /app

COPY Gemfile .
RUN bundle install --quiet

COPY config.ru puma.rb ./

EXPOSE 8443

ENV BENCH_CERT_DIR=/opt/bench

CMD ["bundle", "exec", "puma", "-C", "puma.rb", "config.ru"]
