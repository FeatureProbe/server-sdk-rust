build_date = `date +%Y%m%d%H%M`
commit = `git rev-parse HEAD`
version = `git rev-parse --short HEAD`

.PHONY: release
clean:
	cargo clean
release:
	cargo build --release --verbose
release-test:
	cargo test --release --verbose && \
	cargo test --release --verbose --features async --no-default-features
test:
	cargo test --verbose && \
	cargo test --verbose --features use_tokio --features internal --features event_tokio --no-default-features

