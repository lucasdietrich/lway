#!/bin/bash

echo "=== Compiling daemons ==="
gcc -o daemon_proper daemon_proper.c
gcc -o daemon_bad daemon_bad.c

echo ""
echo "=== Starting proper daemon (double fork) ==="
./daemon_proper
sleep 1

echo ""
echo "=== Starting bad daemon (single fork) ==="
./daemon_bad
sleep 1

echo ""
echo "=== Process information ==="
echo ""

# Find PIDs
PROPER_PID=$(pgrep -f daemon_proper)
BAD_PID=$(pgrep -f daemon_bad)

echo "Proper daemon PID: $PROPER_PID"
echo "Bad daemon PID: $BAD_PID"
echo ""

if [ -n "$PROPER_PID" ]; then
    echo "--- Proper Daemon (double fork) ---"
    ps -o pid,ppid,pgid,sid,tty,stat,cmd -p $PROPER_PID
    echo ""
    echo "Session info for proper daemon:"
    echo "  PID: $PROPER_PID"
    echo "  SID: $(ps -o sid= -p $PROPER_PID | tr -d ' ')"
    echo "  Is session leader? $([ $PROPER_PID -eq $(ps -o sid= -p $PROPER_PID | tr -d ' ') ] && echo 'YES (BAD)' || echo 'NO (GOOD)')"
    echo ""
fi

if [ -n "$BAD_PID" ]; then
    echo "--- Bad Daemon (single fork) ---"
    ps -o pid,ppid,pgid,sid,tty,stat,cmd -p $BAD_PID
    echo ""
    echo "Session info for bad daemon:"
    echo "  PID: $BAD_PID"
    echo "  SID: $(ps -o sid= -p $BAD_PID | tr -d ' ')"
    echo "  Is session leader? $([ $BAD_PID -eq $(ps -o sid= -p $BAD_PID | tr -d ' ') ] && echo 'YES (BAD)' || echo 'NO (GOOD)')"
    echo ""
fi

echo ""
echo "=== Log files ==="
echo "--- Proper daemon log ---"
cat /tmp/daemon_proper.log 2>/dev/null | head -10
echo ""
echo "--- Bad daemon log ---"
cat /tmp/daemon_bad.log 2>/dev/null | head -10
echo ""

echo "=== Key Difference ==="
echo "The proper daemon (PID != SID) cannot acquire a controlling terminal"
echo "The bad daemon (PID == SID) CAN acquire a controlling terminal if it opens one!"
echo ""
echo "To see live processes, use: htop -F daemon"
echo "Or: ps -eo pid,ppid,pgid,sid,tty,stat,cmd | grep daemon"
echo ""
echo "To stop them: kill $PROPER_PID $BAD_PID"
