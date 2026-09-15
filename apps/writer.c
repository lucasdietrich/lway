/* Continuously writes to a file, for exercising lway's IO/log stats. */
#include <errno.h>
#include <fcntl.h>
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
            "usage: %s <file> [-s SIZE] [-i INTERVAL_MS] [-c CHAR] [-a]\n"
            "  -s SIZE          bytes written per iteration (default 64)\n"
            "  -i INTERVAL_MS   delay between writes in milliseconds (default 1000)\n"
            "  -c CHAR          fill byte for the payload (default 'A')\n"
            "  -a               append to the file instead of truncating it\n",
            prog);
}

int main(int argc, char *argv[]) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);
    signal(SIGINT, sigint);
    signal(SIGTERM, sigint);

    if (argc < 2) {
        usage(argv[0]);
        return EXIT_FAILURE;
    }
    const char *path = argv[1];

    size_t size = 64;
    long interval_ms = 1000;
    char fill = 'A';
    int append = 0;

    /* getopt starts scanning at argv[1] by default; skip our positional file arg. */
    optind = 2;
    int opt;
    while ((opt = getopt(argc, argv, "s:i:c:a")) != -1) {
        switch (opt) {
        case 's':
            size = (size_t)strtoul(optarg, NULL, 10);
            break;
        case 'i':
            interval_ms = strtol(optarg, NULL, 10);
            break;
        case 'c':
            fill = optarg[0];
            break;
        case 'a':
            append = 1;
            break;
        default:
            usage(argv[0]);
            return EXIT_FAILURE;
        }
    }

    int flags = O_WRONLY | O_CREAT | (append ? O_APPEND : O_TRUNC);
    int fd = open(path, flags, 0644);
    if (fd < 0) {
        fprintf(stderr, "failed to open %s: %s\n", path, strerror(errno));
        return EXIT_FAILURE;
    }

    char *buf = malloc(size);
    if (buf == NULL) {
        fprintf(stderr, "failed to allocate %zu bytes\n", size);
        return EXIT_FAILURE;
    }
    memset(buf, fill, size);

    struct timespec sleep_time = {
        .tv_sec = interval_ms / 1000,
        .tv_nsec = (interval_ms % 1000) * 1000000L,
    };

    uint64_t iteration = 0;
    uint64_t total_bytes = 0;
    while (running) {
        printf("writing iteration %lu\n", iteration);
        ssize_t written = write(fd, buf, size);
        if (written < 0) {
            fprintf(stderr, "write failed: %s\n", strerror(errno));
            break;
        }
        total_bytes += (uint64_t)written;
        iteration++;
        if (iteration % 16 == 0)
            printf("wrote %lu bytes so far (%lu iterations)\n", total_bytes, iteration);

        fsync(fd);
        nanosleep(&sleep_time, NULL);
    }

    printf("exiting after %lu bytes written\n", total_bytes);
    free(buf);
    close(fd);
    return EXIT_SUCCESS;
}
