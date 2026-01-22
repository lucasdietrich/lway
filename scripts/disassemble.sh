#!/bin/bash

set -eu

if [ $# -ne 1 ]; then
    echo "Usage: $0 <elf-file>"
    exit 1
fi

ELF="$1"

if [ ! -f "$ELF" ]; then
    echo "Error: File '$ELF' not found"
    exit 1
fi

# Toolchain prefix (if using cross-compilation)
PREFIX="${CROSS_COMPILE:-}"

OBJDUMP="${PREFIX}objdump"
READELF="${PREFIX}readelf"
NM="${PREFIX}nm"
SIZE="${PREFIX}size"
RUSTFILT="rustfilt"

# Determine base path for outputs
DIR="$(dirname "$ELF")"
BASE="$(basename "$ELF")"
BASE_NO_EXT="${BASE%.*}"
OUT_PREFIX="${DIR}/${BASE_NO_EXT}"

echo "Analyzing ELF: $ELF"
echo "Toolchain prefix: ${PREFIX:-(native)}"
echo "Output prefix: $OUT_PREFIX"
echo

# Helper: run command if tool exists
run_if_exists() {
    TOOL="$1"
    OUT_FILE="$2"
    shift 2
    if command -v "$TOOL" >/dev/null 2>&1; then
        echo "Running: $TOOL $*"
        "$TOOL" "$@" > "$OUT_FILE" || echo "Warning: $TOOL failed for $*"
    else
        echo "Warning: $TOOL not found, skipping $OUT_FILE"
    fi
}

# Objdump commands
run_if_exists "$OBJDUMP" "${OUT_PREFIX}.objdump_src.s" -S "$ELF"
run_if_exists "$OBJDUMP" "${OUT_PREFIX}.objdump.s" -d "$ELF"
run_if_exists "$OBJDUMP" "${OUT_PREFIX}.objdump_all.s" -D "$ELF"
run_if_exists "$OBJDUMP" "${OUT_PREFIX}.objdump_sections.s" -h "$ELF"

# Readelf full dump
run_if_exists "$READELF" "${OUT_PREFIX}.readelf.txt" -a "$ELF"

# Readelf section hex dumps
for section in .data .bss .noinit .text; do
    OUT="${OUT_PREFIX}.section_${section#.}.txt"
    if command -v "$READELF" >/dev/null 2>&1; then
        echo "Dumping section: $section"
        "$READELF" -x "$section" "$ELF" > "$OUT" 2>/dev/null || echo "Warning: Section $section not found"
    else
        echo "Warning: $READELF not found, skipping $OUT"
    fi
done

# nm and optionally demangle with rustfilt
NM_RAW="${OUT_PREFIX}.nm.txt"
NM_DEMANGLED="${OUT_PREFIX}.nm.demangled.txt"
if command -v "$NM" >/dev/null 2>&1; then
    echo "Generating symbol table with $NM"
    "$NM" --print-size --size-sort --radix=x "$ELF" > "$NM_RAW"
    if command -v "$RUSTFILT" >/dev/null 2>&1; then
        echo "Demangling Rust symbols with $RUSTFILT"
        "$RUSTFILT" < "$NM_RAW" > "$NM_DEMANGLED"
    else
        echo "Warning: $RUSTFILT not found, skipping Rust symbol demangling"
    fi
else
    echo "Warning: $NM not found, skipping symbol dump"
fi

# Size
run_if_exists "$SIZE" "${OUT_PREFIX}.size.txt" -B "$ELF"

# SONAME and shared libraries
INFO_FILE="${OUT_PREFIX}.dyninfo.txt"
if command -v "$READELF" >/dev/null 2>&1; then
    SONAME=$($READELF -d "$ELF" 2>/dev/null | awk '/SONAME/ {print $NF}' | tr -d '[]' || true)
    NEEDED_LIBS=$($READELF -d "$ELF" 2>/dev/null | awk '/NEEDED/ {print $NF}' | tr -d '[]' || true)

    {
        [ -n "$SONAME" ] && echo "SONAME: $SONAME"
        echo "NEEDED libraries:"
        [ -n "$NEEDED_LIBS" ] && echo "$NEEDED_LIBS" || echo "  (none or not a dynamic binary)"
    } > "$INFO_FILE"
else
    echo "Warning: $READELF not found, skipping $INFO_FILE"
fi

echo
echo "Analysis complete. Output files:"
ls -1 "${OUT_PREFIX}".* 2>/dev/null || echo "No output generated."

