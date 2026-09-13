# collie-waker build / deploy helpers.
#
# Common usage:
#   make build                        # docker build, tag :dev
#   make push                         # docker push IMAGE=:dev
#   make lint                         # cargo check + cargo clippy + helm lint
#   make render                       # helm template dry-run to stdout
#   make install                      # helm install (requires --set overrides)
#   make package                      # helm package the chart into dist/

REGISTRY   ?= ghcr.io
IMAGE_USER ?= jonmast
IMAGE_NAME ?= collie-waker
IMAGE      ?= $(REGISTRY)/$(IMAGE_USER)/$(IMAGE_NAME)
TAG        ?= dev
PLATFORM   ?= linux/amd64
CHART      := charts/$(IMAGE_NAME)
DIST       := dist

CARGO     ?= cargo
HELM      ?= helm
DOCKER    ?= docker
BUILDX    ?= docker buildx

# Verbosity for cargo
CARGO_FLAGS ?=

# --- Docker ------------------------------------------------------------------

.PHONY: build
build: ## docker build (single-arch)
	$(DOCKER) build \
		--platform $(PLATFORM) \
		--tag $(IMAGE):$(TAG) \
		--tag $(IMAGE):latest \
		.

.PHONY: push
push: ## docker push $(TAG) and :latest
	$(DOCKER) push $(IMAGE):$(TAG)
	$(DOCKER) push $(IMAGE):latest

# --- Rust --------------------------------------------------------------------

.PHONY: check
check: ## cargo check
	$(CARGO) check $(CARGO_FLAGS) --all-targets

.PHONY: clippy
clippy: ## cargo clippy (deny warnings)
	$(CARGO) clippy $(CARGO_FLAGS) --all-targets -- -D warnings

.PHONY: test
test: ## cargo test
	$(CARGO) test $(CARGO_FLAGS) --all-targets

# --- Helm --------------------------------------------------------------------

.PHONY: helm-deps
helm-deps: ## fetch and unpack helm chart dependencies
	@if ! $(HELM) dependency list $(CHART) | tail -n +2 | grep -q 'ok[[:space:]]*$$'; then \
		$(HELM) dependency build $(CHART); \
	fi
	@cd $(CHART)/charts && for f in *.tgz; do \
		dir="$${f%-*.tgz}"; \
		if [ ! -d "$$dir" ]; then \
			tar -xzf "$$f"; \
		fi; \
	done

.PHONY: helm-lint
helm-lint: helm-deps ## helm lint the chart
	$(HELM) lint $(CHART) \
		--set waker.publicHost=collie.example.com \
		--set ssh.host=192.0.2.10 \
		--set ssh.user=collie \
		--set wol.macAddress=AA:BB:CC:DD:EE:FF

.PHONY: render
render: helm-deps ## helm template render (no install). Use --set to fill required values.
	$(HELM) template release $(CHART) \
		--set waker.publicHost=collie.example.com \
		--set ssh.host=192.0.2.10 \
		--set ssh.user=collie \
		--set wol.macAddress=AA:BB:CC:DD:EE:FF

.PHONY: install
install: helm-deps ## helm install. Pass extra flags via HELM_FLAGS=...
	$(HELM) install collie-waker $(CHART) $(HELM_FLAGS)

.PHONY: upgrade
upgrade: helm-deps ## helm upgrade. Pass extra flags via HELM_FLAGS=...
	$(HELM) upgrade collie-waker $(CHART) $(HELM_FLAGS)

.PHONY: uninstall
uninstall: ## helm uninstall
	$(HELM) uninstall collie-waker

.PHONY: package
package: helm-deps ## helm package the chart into dist/
	@mkdir -p $(DIST)
	$(HELM) package $(CHART) --destination $(DIST)

# --- Meta --------------------------------------------------------------------

.PHONY: lint
lint: check clippy helm-lint ## run every static check

.PHONY: help
help: ## show this help
	@awk 'BEGIN {FS = ":.*##"; printf "Targets:\n"} \
		/^[a-zA-Z_-]+:.*##/ { printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)

.DEFAULT_GOAL := help
