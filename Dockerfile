# Rova Docker image
# Usage:
#   docker build -t rova .
#   docker run -it --rm rova --help
#   docker run -it --rm -v ~/.config/rova:/root/.config/rova rova chat

FROM python:3.12-slim-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
    bubblewrap \
    nsjail \
    xclip \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY pyproject.toml README.md ./
COPY rova/ ./rova/

RUN pip install --no-cache-dir .

ENTRYPOINT ["rova"]
CMD ["chat"]
