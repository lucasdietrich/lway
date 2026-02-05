#include <unistd.h>
#include <stdio.h>
#include <fcntl.h>

int main() {
    // Open a file
    int fd = open("output.txt", O_WRONLY | O_CREAT | O_TRUNC, 0644);
    
    // Duplicate stdout (fd 1) to the file descriptor
    // Now writing to stdout will write to the file
    dup2(fd, STDOUT_FILENO);
    
    // This goes to the file instead of the terminal
    printf("This message goes to output.txt\n");
    
    close(fd);
    return 0;
}
