#!/bin/bash
set -euo pipefail

# When cargo comes with the Yocto SDK, and the script is called from `cargo run`,
# with `runner` being properly set in `.cargo/config.toml`, cargo is messing
# up the by setting the LD_LIBRARY_PATH.
# This is a workaround to unset it. Additionally, this script must be called using bash. 
unset LD_LIBRARY_PATH

# check whether the Yocto SDK is set up (SDKTARGETSYSROOT)
if [ -z "${SDKTARGETSYSROOT:-}" ]; then
  echo "Error: SDKTARGETSYSROOT is not set. Please source the Yocto SDK environment setup script first."
  exit 1
fi

# Check for --debug flag and remove it from arguments
if [[ "${1:-}" == "--debug" ]]; then
  QEMU_ARG_GDB="-g 1234"
  shift
fi

qemu-arm \
-L "$SDKTARGETSYSROOT" \
  ${QEMU_ARG_GDB:-} \
  $@

