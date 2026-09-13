// Proper daemon with DOUBLE fork (not a session leader)
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>

void daemonize_proper() {
    pid_t pid;

    // First fork
    pid = fork();
    if (pid < 0) exit(EXIT_FAILURE);
    if (pid > 0) exit(EXIT_SUCCESS);  // Parent exits

    // Create new session - becomes session leader
    if (setsid() < 0) exit(EXIT_FAILURE);

    // Second fork - NO LONGER session leader!
    pid = fork();
    if (pid < 0) exit(EXIT_FAILURE);
    if (pid > 0) exit(EXIT_SUCCESS);  // Parent exits

    umask(0);
    chdir("/");

    // Redirect
    int fd = open("/dev/null", O_RDWR);
    dup2(fd, 0);
    dup2(fd, 1);
    dup2(fd, 2);
    if (fd > 2) close(fd);
}

int main() {
    printf("Starting proper daemon (PID: %d)\n", getpid());
    daemonize_proper();

    // Daemon work - write to a log file to prove it's running
    FILE *log = fopen("/tmp/daemon_proper.log", "w");
    if (log) {
        fprintf(log, "Proper daemon running with PID: %d\n", getpid());
        fprintf(log, "Session ID (SID): %d\n", getsid(0));
        fprintf(log, "Process Group ID (PGID): %d\n", getpgid(0));
        fprintf(log, "Parent PID: %d\n", getppid());
        fflush(log);
        
        for (int i = 0; i < 600; i++) {
            fprintf(log, "Tick %d\n", i);
            fflush(log);
            sleep(1);
        }
        fclose(log);
    }
    
    return 0;
}
