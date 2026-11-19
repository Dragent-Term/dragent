# ── Build stage ───────────────────────────────────────────────────────────────
# Pinned Go matching go.mod. Static build so the final image needs no libc.
#
# Multi-arch: built via `docker buildx --platform linux/amd64,linux/arm64`, so
# one image manifest runs on BOTH amd64 (App Runner) and arm64 (EC2 t4g/Graviton).
# BUILDPLATFORM keeps the compiler running natively on the CI runner while Go
# cross-compiles to TARGETARCH for free (CGO is off) — no QEMU emulation of the
# compiler, so multi-arch builds stay fast.
FROM --platform=$BUILDPLATFORM golang:1.25-bookworm AS build

ARG TARGETOS
ARG TARGETARCH

WORKDIR /src

# Cache deps separately from source for fast rebuilds.
COPY go.mod go.sum ./
RUN go mod download

COPY . .

# CGO disabled → a fully static binary that runs on distroless/scratch.
# GOOS/GOARCH come from buildx's target platform (falls back to linux/amd64 for a
# plain `docker build`, preserving the old single-arch behaviour).
# -trimpath + ldflags strip paths and debug info for a smaller, reproducible build.
RUN CGO_ENABLED=0 GOOS=${TARGETOS:-linux} GOARCH=${TARGETARCH:-amd64} go build \
    -trimpath -ldflags="-s -w" \
    -o /out/server ./cmd/api

# ── Runtime stage ─────────────────────────────────────────────────────────────
# Distroless: no shell, no package manager — minimal attack surface. Runs as a
# non-root user by default (the :nonroot tag). static-debian12 is itself a
# multi-arch manifest, so buildx picks the matching arch automatically.
FROM gcr.io/distroless/static-debian12:nonroot

WORKDIR /app
COPY --from=build /out/server /app/server

# App Runner / the platform sets PORT; the server already honors it (default 8080).
EXPOSE 8080

# Non-root, static binary, no shell — production-safe defaults.
USER nonroot:nonroot
ENTRYPOINT ["/app/server"]
