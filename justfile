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

test-native:
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

deploy-release: release deploy-config
  scp target/aarch64-unknown-linux-gnu/release/lway {{target}}:~/

app:
  . ./.env && bear -- $CC apps/app.c -o apps/app
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/app {{target}}:/var/amy/

ctest: deploy-config
  . ./.env && bear -- $CC apps/test.c -o apps/test
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/test {{target}}:/var/amy/

writer:
  . ./.env && bear -- $CC apps/writer.c -o apps/writer
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/writer {{target}}:/var/amy/

ch:
  . ./.env && bear -- $CC apps/ch.c -o apps/ch
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/ch {{target}}:/var/amy/

apps APP='app':
  . ./.env && bear -- $CC apps/{{APP}}.c -o apps/{{APP}}
  ssh {{target}} "mkdir -p /var/amy"
  scp apps/{{APP}} {{target}}:/var/amy/