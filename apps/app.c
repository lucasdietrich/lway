#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <stdint.h>

static int sigint_count = 0;

void sigint(int signum) {
    printf("Caught signal %d (SIGINT)\n", signum);
    sigint_count++;

    if (sigint_count >= 2) {
        printf("Exiting after %d SIGINTs\n", sigint_count);
        exit(EXIT_FAILURE);
    }
}

int main(int argc, char *argv[]) {
    uint32_t i = 0;
    uint32_t max_iterations = 0; // 0 means infinite
    
    // Parse command line argument
    if (argc > 1) {
        max_iterations = (uint32_t)atoi(argv[1]);
    }
    
    printf("Hello, World!\n");
    if (max_iterations == 0) {
        printf("Running infinite loop (max_iterations=0)\n");
    } else {
        printf("Running %u iterations\n", max_iterations);
    }

    // Ctrl +C
    signal(SIGINT, sigint);

    for (;;) {
        printf("Tick %u\n", i);
        sleep(1);
        i++;
        
        // Exit if we've reached max_iterations (unless it's 0 for infinite)
        if (max_iterations > 0 && i >= max_iterations) {
            printf("Completed %u iterations. Exiting.\n", max_iterations);
            break;
        }
    }
    
    // Return ok if max_iterations is even otherwise return failure
    if (max_iterations & 1) {
        return EXIT_FAILURE;
    } else {
        return EXIT_SUCCESS;
    }
}