FROM rust:1.87-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates python3 python3-pip perl \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /repo
CMD ["bash", "integrations/mkdocs/tests/docker/run-e2e.sh"]
