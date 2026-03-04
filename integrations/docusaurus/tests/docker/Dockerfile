FROM rust:1.87-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates python3 perl \
 && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - \
 && apt-get update \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /repo
CMD ["bash", "integrations/docusaurus/tests/docker/run-e2e.sh"]
