.PHONY: profile install-flamegraph flamegraph baseline

profile:
	bash scripts/profile.sh

install-flamegraph:
	cargo install flamegraph

flamegraph:
	cargo flamegraph --bin doora -- \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/doora-profile-fixture \
		--lang rust --no-color --quiet

baseline:
	@echo "measuring baseline on fixture repo..."
	@time ./target/release/doora \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/doora-profile-fixture \
		--lang rust --no-color --quiet \
		2>/dev/null | wc -l
