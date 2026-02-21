.PHONY: build test run stop

build:
	cd orchestrator && cargo build

test:
	cd orchestrator && cargo test

run:
	cd orchestrator && cargo run -- run ..

stop:
	cd orchestrator && cargo run -- stop ..
