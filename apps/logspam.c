/* Spams stdout/stderr with log lines, for exercising lway's log splicing. */
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t running = 1;

void sigint(int signum) {
    (void)signum;
    running = 0;
}

static void usage(const char *prog) {
    fprintf(stderr,
            "usage: %s [-s SIZE] [-i INTERVAL_MS] [-n COUNT] [-b BURST] [-c CHAR] [-o out|err|both] [-a]\n"
            "  -s SIZE          payload bytes per log line (default 64)\n"
            "  -i INTERVAL_MS   delay between bursts in milliseconds (default 100)\n"
            "  -n COUNT         total lines to emit, 0 for unlimited (default 0)\n"
            "  -b BURST         lines emitted per interval (default 1)\n"
            "  -c CHAR          fill byte for the payload (default 'x')\n"
            "  -o out|err|both  output stream to write to (default out)\n"
            "  -a               flush after every line\n",
            prog);
}

int main(int argc, char *argv[]) {
    size_t size = 64;
    long interval_ms = 100;
    uint64_t count = 0;
    uint64_t burst = 1;
    char fill = 'x';
    int to_stdout = 1;
    int to_stderr = 0;
    int flush_each = 0;

    int opt;
    while ((opt = getopt(argc, argv, "s:i:n:b:c:o:a")) != -1) {
        switch (opt) {
        case 's':
            size = (size_t)strtoul(optarg, NULL, 10);
            break;
        case 'i':
            interval_ms = strtol(optarg, NULL, 10);
            break;
        case 'n':
            count = strtoull(optarg, NULL, 10);
            break;
        case 'b':
            burst = strtoull(optarg, NULL, 10);
            break;
        case 'c':
            fill = optarg[0];
            break;
        case 'o':
            if (strcmp(optarg, "out") == 0) {
                to_stdout = 1;
                to_stderr = 0;
            } else if (strcmp(optarg, "err") == 0) {
                to_stdout = 0;
                to_stderr = 1;
            } else if (strcmp(optarg, "both") == 0) {
                to_stdout = 1;
                to_stderr = 1;
            } else {
                usage(argv[0]);
                return EXIT_FAILURE;
            }
            break;
        case 'a':
            flush_each = 1;
            break;
        default:
            usage(argv[0]);
            return EXIT_FAILURE;
        }
    }

    signal(SIGINT, sigint);
    signal(SIGTERM, sigint);

    char *payload = malloc(size + 1);
    if (payload == NULL) {
        fprintf(stderr, "failed to allocate %zu bytes\n", size);
        return EXIT_FAILURE;
    }
    memset(payload, fill, size);
    payload[size] = '\0';

    struct timespec sleep_time = {
        .tv_sec = interval_ms / 1000,
        .tv_nsec = (interval_ms % 1000) * 1000000L,
    };

    uint64_t emitted = 0;
    while (running && (count == 0 || emitted < count)) {
        for (uint64_t i = 0; i < burst && (count == 0 || emitted < count); i++, emitted++) {
            if (to_stdout) {
                printf("[%lu] pid=%d %s\n", emitted, getpid(), payload);
                if (flush_each)
                    fflush(stdout);
            }
            if (to_stderr) {
                fprintf(stderr, "[%lu] pid=%d %s\n", emitted, getpid(), payload);
                if (flush_each)
                    fflush(stderr);
            }
        }
        nanosleep(&sleep_time, NULL);
    }

    fprintf(stderr, "exiting after %lu lines\n", emitted);
    free(payload);
    return EXIT_SUCCESS;
}
