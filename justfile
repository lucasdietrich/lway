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
exe := "target/aarch64-unknown-linux-gnu/debug/lway"

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
  scp target/aarch64-unknown-linux-gnu/debug/examples/{{EXAMPLE}} {{target}}:~/

target := "mpx.usb"
deploy: build
  scp {{exe}} {{target}}:~/
  scp apps.yaml {{target}}:~/

app:
  bear -- $CC apps/app.c -o apps/app 
  scp apps/app {{target}}:~/