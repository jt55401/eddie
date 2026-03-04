FROM rust:1.87-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates ruby-full build-essential python3 perl \
 && rm -rf /var/lib/apt/lists/*

RUN gem install bundler jekyll --no-document

WORKDIR /repo
CMD ["bash", "integrations/jekyll/tests/docker/run-e2e.sh"]
