build:
	cargo build

run: build
	RUST_LOG=trace RUST_BACKTRACE=1 ./target/debug/socr

lint:
	cargo clippy --tests --workspace -- -D warnings

test:
	cargo test --verbose -- --nocapture --skip integration ${name}

test_integration:
	cargo test --test '*' --verbose -- --nocapture --test-threads 1 ${name}
