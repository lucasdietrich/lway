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

example EXAMPLE='pipe':
  cargo build --example {{EXAMPLE}}
  scp target/thumbv7neon-unknown-linux-gnueabihf/debug/examples/{{EXAMPLE}} {{target}}:~/

target := "amy"
deploy: build
  scp {{exe}} {{target}}:~/
  scp apps.yaml {{target}}:~/

app:
  bear -- $CC apps/app.c -o apps/app 
  scp apps/app {{target}}:~/