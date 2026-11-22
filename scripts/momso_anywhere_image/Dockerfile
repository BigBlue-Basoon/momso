FROM python:3.12-slim
WORKDIR /app
RUN pip install --no-cache-dir google-auth==2.52.0 requests==2.33.1
COPY scripts/momso_anywhere_gateway.py scripts/momso_anywhere_auth.py scripts/momso_anywhere_login.mjs scripts/momso_anywhere_public_assets.py ./
COPY scripts/momso_wake/main.py ./momso_wake/main.py
COPY public/ ./public/
USER 65532:65532
CMD ["python", "momso_anywhere_gateway.py"]
