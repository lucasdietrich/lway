#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <unistd.h>

void sigterm(int signum) {
    printf("Received SIGTERM signal: %d\n", signum);
} 

void info() {
    pid_t pid = getpid();
    gid_t gid = getgid();

    printf("PID: %d\n", pid);
    printf("GID: %d\n", gid);
}


int main() {
    // setpgid(0, 0);
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);

    int ret = fork();
    if (ret == -1) {
        perror("fork");
        return 1;
    } else if (ret == 0) {
        printf("Child process\n");
        printf("Child PID: %d\n", getpid());
    } else {
        printf("Parent process\n");
        printf("Parent PID: %d\n", getpid());

        // signal(SIGTERM, sigterm);

        // exit(0);
    }

    info();

    for (;;) {
        sleep(1);
    }
}