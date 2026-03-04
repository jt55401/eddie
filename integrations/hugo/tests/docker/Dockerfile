FROM rust:1.87-bookworm

RUN apt-get update \
 && apt-get install -y --no-install-recommends git curl ca-certificates perl \
 && rm -rf /var/lib/apt/lists/*

RUN HUGO_VERSION=0.126.1 \
 && curl -fsSL -o /tmp/hugo.tar.gz "https://github.com/gohugoio/hugo/releases/download/v${HUGO_VERSION}/hugo_extended_${HUGO_VERSION}_linux-amd64.tar.gz" \
 && tar -xzf /tmp/hugo.tar.gz -C /usr/local/bin hugo \
 && rm -f /tmp/hugo.tar.gz

WORKDIR /repo
CMD ["bash", "integrations/hugo/tests/docker/run-e2e.sh"]
