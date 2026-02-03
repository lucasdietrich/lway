# Justfile for managing build tasks
# Use `just <task>` to run a specific task
# Use `just --summary` to see available tasks
# QEMU
qemu *args: build
  ./scripts/run-qemu.sh {{exe}} {{args}}
qemu-debug *args: build
  ./scripts/run-qemu.sh --debug {{exe}} {{args}}

disassemble: build
  scripts/disassemble.sh {{exe}}
exe := "target/thumbv7neon-unknown-linux-gnueabihf/debug/lway"

build: debug

run:
  cargo run

release:
  cargo build --release

debug:
  cargo build

test:
  cargo test

clean:
  cargo clean

target := "amy"
deploy: build
  scp {{exe}} {{target}}:~/
