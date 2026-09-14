#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <signal.h>

void sigint(int signum) {
    printf("Received SIGINT (%d)\n", signum);
}

#include <unistd.h>

int main(int argc, char *argv[])
{
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);

    signal(SIGINT, sigint);

    size_t alloc_mb = 0;
    int opt;
    while ((opt = getopt(argc, argv, "M:")) != -1) {
        if (opt == 'M')
            alloc_mb = (size_t)strtoul(optarg, NULL, 10);
    }

    struct timespec start;
    clock_gettime(CLOCK_MONOTONIC, &start);

    uint64_t i = 0;
    uint64_t mod = 0x10000000;
    int allocated = 0;
    for (;;) {
        if (i % mod == 0)
            printf("Hello %lu\n", i);

        if (!allocated && alloc_mb > 0) {
            struct timespec now;
            clock_gettime(CLOCK_MONOTONIC, &now);
            double elapsed = (now.tv_sec - start.tv_sec) + (now.tv_nsec - start.tv_nsec) / 1e9;
            if (elapsed >= 2.0) {
                size_t size = alloc_mb * 1024 * 1024;
                void *mem = malloc(size);
                if (mem == NULL) {
                    fprintf(stderr, "Failed to allocate %zu MB\n", alloc_mb);
                } else {
                    memset(mem, 0xAA, size); /* touch pages so they are actually committed */
                    printf("Allocated %zu MB\n", alloc_mb);
                }
                allocated = 1;
            }
        }

        i++;
    }
}