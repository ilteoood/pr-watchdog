FROM alpine:latest AS builder
ARG TARGETARCH
WORKDIR /builder
COPY . .
RUN ./scripts/binary.sh $TARGETARCH && \
    echo "nobody:x:65534:65534:Nobody:/:" > /etc_passwd

FROM scratch
COPY --from=builder --chmod=755 /builder/pr-watchdog ./pr-watchdog
COPY --from=builder "/etc_passwd" "/etc/passwd"
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /usr/local/ssl/ca-certificates.crt
USER nobody

ENV RUST_LOG=info
ENV GITHUB_TOKEN=your-github-token
ENV WATCHED_REPOS=owner/repo
ENV CRON_PATTERN="0 */5 8-18 * * Mon-Fri *"

CMD ["./pr-watchdog"]
