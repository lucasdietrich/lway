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
  cargo test --target x86_64-unknown-linux-gnu

clean:
  cargo clean

example EXAMPLE='pipe':
  cargo build --example {{EXAMPLE}}
  scp target/aarch64-unknown-linux-gnu/debug/examples/{{EXAMPLE}} {{target}}:~/

target := "mpx.usb"
deploy-config:
  scp lway.yaml {{target}}:~/

deploy: build deploy-config
  scp {{exe}} {{target}}:~/

app:
  bear -- $CC apps/app.c -o apps/app 
  scp apps/app {{target}}:~/

ctest: deploy-config
  . ./.env && $CC apps/test.c -o apps/test
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/test {{target}}:/var/amy/