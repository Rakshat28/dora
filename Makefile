.PHONY: profile install-flamegraph flamegraph baseline

profile:
	bash scripts/profile.sh

install-flamegraph:
	cargo install flamegraph

flamegraph:
	cargo flamegraph --bin ast-search -- \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/ast-search-profile-fixture \
		--lang rust --no-color --quiet

baseline:
	@echo "measuring baseline on fixture repo..."
	@time ./target/release/ast-search \
		-q '(function_item name: (identifier) @fn_name)' \
		-p /tmp/ast-search-profile-fixture \
		--lang rust --no-color --quiet \
		2>/dev/null | wc -l
