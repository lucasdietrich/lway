#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <stdint.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <string.h>

static int sigint_count = 0;

void sigint(int signum) {
    printf("Caught signal %d (SIGINT)\n", signum);
    sigint_count++;

    if (sigint_count >= 2) {
        printf("Exiting after %d SIGINTs\n", sigint_count);
        exit(EXIT_FAILURE);
    }
}

void daemonize() {
    pid_t pid;

    // First fork
    pid = fork();
    if (pid < 0) {
        exit(EXIT_FAILURE);
    }
    if (pid > 0) {
        // Parent exits
        exit(EXIT_SUCCESS);
    }

    // Create new session
    if (setsid() < 0) {
        exit(EXIT_FAILURE);
    }

    // Second fork
    pid = fork();
    if (pid < 0) {
        exit(EXIT_FAILURE);
    }
    if (pid > 0) {
        // Parent exits
        exit(EXIT_SUCCESS);
    }

    // Set umask
    umask(0);

    // Change working directory to root
    chdir("/");

    // Close all file descriptors
    for (int fd = sysconf(_SC_OPEN_MAX); fd >= 0; fd--) {
        close(fd);
    }

    // Reopen stdin, stdout, stderr to /dev/null
    int fd = open("/dev/null", O_RDWR);
    if (fd != -1) {
        dup2(fd, STDIN_FILENO);
        dup2(fd, STDOUT_FILENO);
        dup2(fd, STDERR_FILENO);
        if (fd > 2) {
            close(fd);
        }
    }
}

int main(int argc, char *argv[]) {
    // set flush on for stdout and stderr
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);

    uint32_t i = 0;
    uint32_t max_iterations = 0; // 0 means infinite
    int daemon_mode = 0;
    
    // Parse command line arguments
    for (int arg_idx = 1; arg_idx < argc; arg_idx++) {
        if (strcmp(argv[arg_idx], "-d") == 0) {
            daemon_mode = 1;
        } else {
            max_iterations = (uint32_t)atoi(argv[arg_idx]);
        }
    }
    
    printf("Hello, World!\n");
    fprintf(stderr, "This is an error message\n");
    if (max_iterations == 0) {
        printf("Running infinite loop (max_iterations=0)\n");
    } else {
        printf("Running %u iterations\n", max_iterations);
    }

    if (daemon_mode) {
        printf("Daemonizing in 2 seconds...\n");
        sleep(2);
        daemonize();
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