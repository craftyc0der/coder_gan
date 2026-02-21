.PHONY: build test run stop kill

build:
	cd orchestrator && cargo build

test:
	cd orchestrator && cargo test

run:
	cd orchestrator && cargo run -- run ..

stop:
	cd orchestrator && cargo run -- stop ..

kill:
	@pids=$$(ps -eo pid,args | grep -E 'target/debug/orchestrator|cargo run -- run \.\.|orchestrator run' | grep -v grep | awk '{print $$1}'); \
	if [ -n "$$pids" ]; then \
		echo "Killing orchestrator processes: $$pids"; \
		kill $$pids; \
	else \
		echo "No orchestrator processes found."; \
	fi
