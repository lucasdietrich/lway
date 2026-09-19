#define _GNU_SOURCE
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <stdio.h>

#include <sys/mman.h>
#include <fcntl.h>

void hexdump(const void *data, size_t size)
{
    const uint8_t *bytes = (const uint8_t *)data;
    for (size_t i = 0; i < size; i++) {
        if (i % 32 == 0) {
            printf("%08zx  ", i);
        }
        printf("%02x ", bytes[i]);
        if (i % 32 == 31 || i == size - 1) {
            printf("\n");
        }
    }
}

int main()
{
    size_t size = 16384;

    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);

    int fd = memfd_create("log", MFD_ALLOW_SEALING);
    perror("memfd_create");

    ftruncate(fd, size); /* set its size first */

    /* lock the size once, before any mapping is made */
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW) < 0) {
        perror("fcntl F_ADD_SEALS");
    }

    void *addr = mmap(NULL, size, PROT_READ | PROT_WRITE,
                    MAP_PRIVATE, fd, 0);
    if (addr == MAP_FAILED) {
        perror("mmap");
        return 1;
    }

    memset(addr, 0xAA, size);
    
    hexdump(addr, size);


    // uint8_t buffer[16384];
    // for (int i = 0; i < sizeof(buffer); i++)
    //     buffer[i] = 0xAA;

    // for (size_t i = 0; i < 8 * 128 * 64; i++) {
    //     ssize_t ret = write(fd, buffer, sizeof(buffer));
    //     if (ret < 0) {
    //         perror("write");
    //         continue;
    //     }

    //     off_t pos = lseek(fd, 0, SEEK_CUR);
    //     printf("wrote %zd bytes, now at offset %lld\n", ret, (long long)pos);
    // }

    // /* verify total size */
    // off_t total = lseek(fd, 0, SEEK_END);
    // printf("total file size: %lld\n", (long long)total);

    // /* read back first 16 bytes from the start */
    // uint8_t readbuf[16];
    // ssize_t n = pread(fd, readbuf, sizeof(readbuf), 0);
    // printf("read back %zd bytes: %02x %02x %02x ...\n", n, readbuf[0], readbuf[1], readbuf[2]);

    printf("done\n");

    for (;;) {
        sleep(1);
    }

    close(fd);
    
    return 0;
}