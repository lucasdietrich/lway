// Bad daemon with SINGLE fork (remains session leader - can acquire terminal!)
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <fcntl.h>

void daemonize_bad() {
    pid_t pid;

    // Only one fork
    pid = fork();
    if (pid < 0) exit(EXIT_FAILURE);
    if (pid > 0) exit(EXIT_SUCCESS);  // Parent exits

    // Create new session - becomes session leader and STAYS session leader!
    if (setsid() < 0) exit(EXIT_FAILURE);
    
    // NO SECOND FORK - this is the problem!

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
    printf("Starting bad daemon (PID: %d)\n", getpid());
    daemonize_bad();

    // Daemon work
    FILE *log = fopen("/tmp/daemon_bad.log", "w");
    if (log) {
        fprintf(log, "Bad daemon running with PID: %d\n", getpid());
        fprintf(log, "Session ID (SID): %d\n", getsid(0));
        fprintf(log, "Process Group ID (PGID): %d\n", getpgid(0));
        fprintf(log, "Parent PID: %d\n", getppid());
        fprintf(log, "IS SESSION LEADER: %s\n", 
                (getpid() == getsid(0)) ? "YES - BAD!" : "NO - GOOD!");
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
