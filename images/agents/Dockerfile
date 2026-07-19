ARG NODE_IMAGE=node:24-bookworm-slim
FROM ${NODE_IMAGE}

ARG AGENT_PACKAGE
ARG AGENT_VERSION
RUN test -n "${AGENT_PACKAGE}" && test -n "${AGENT_VERSION}" \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git openssh-client ripgrep \
    && rm -rf /var/lib/apt/lists/* \
    && npm install --global --prefix /opt/agent "${AGENT_PACKAGE}@${AGENT_VERSION}" \
    && npm cache clean --force \
    && useradd --create-home --uid 10001 --shell /bin/bash agent \
    && mkdir -p /workspace \
    && chown agent:agent /workspace

ENV PATH="/opt/agent/bin:${PATH}"
ENV HOME=/home/agent
USER agent
WORKDIR /workspace
CMD ["/bin/sh"]
