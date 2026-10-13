# Makefile for StarMade Shipyard Helper

.PHONY: all build test cli clean help

GRADLE ?= $(shell if [ -f ./gradlew ]; then echo "./gradlew"; else echo "gradle"; fi)
# Directory holding BlockConfig.xml and BlockTypes.properties, for the offline harness
CATALOG ?= $(HOME)/.smtest2/vol/data/config

all: build

# Build the shaded mod jar into build/libs/
build:
	$(GRADLE) shadowJar

# Run JVM unit tests
test:
	$(GRADLE) test

# Chat with the agent against an in-memory design (no game needed)
cli:
	$(GRADLE) -q --console=plain cli --args="--catalog $(CATALOG)"

clean:
	rm -rf build

help:
	@echo "make build  - build the mod jar"
	@echo "make test   - run unit tests"
	@echo "make cli    - offline agent harness (CATALOG=<dir with BlockConfig.xml>)"
	@echo "make clean  - remove build outputs"
