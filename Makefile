.PHONY: profile install-flamegraph flamegraph baseline

profile:
	bash scripts/profile.sh

install-flamegraph:
	cargo install flamegraph

flamegraph:
	cargo flamegraph --bin dora -- \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/dora-profile-fixture \
		--lang rust --no-color --quiet

baseline:
	@echo "measuring baseline on fixture repo..."
	@time ./target/release/dora \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/dora-profile-fixture \
		--lang rust --no-color --quiet \
		2>/dev/null | wc -l
