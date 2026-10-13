FROM python:3.12-slim

# Orion runs with zero third-party Python dependencies (pure stdlib).
LABEL org.opencontainers.image.title="Orion Assistant"
LABEL org.opencontainers.image.description="General-purpose expert AI assistant"

WORKDIR /app
COPY orion/ ./orion/
COPY tests/ ./tests/
COPY README.md .

# Non-root user
RUN useradd -m orion && mkdir -p /home/orion/.orion && chown -R orion:orion /app /home/orion/.orion
USER orion

# API by default; CLI = `docker run … orion-cli "question"`
ENV ORION_DB_PATH=/home/orion/.orion/assistant.db
EXPOSE 8765

CMD ["python3", "-m", "orion.api", "8765"]
